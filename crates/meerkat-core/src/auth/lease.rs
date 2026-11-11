//! Auth lease, credential-material kinds, authorizer trait, and refresh semantics.
//!
//! `meerkat-core` owns the trait shape; concrete lease implementations
//! (`StaticLease`, `DynamicLease`) live in `crates/meerkat-client/src/runtime/binding.rs`.
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
#[derive(Clone, Serialize, Deserialize)]
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

/// Redacts the secret and header values.
impl std::fmt::Debug for ResolvedAuthEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InlineSecret {
                metadata,
                expires_at,
                ..
            } => f
                .debug_struct("InlineSecret")
                .field("secret", &crate::redact::REDACTED)
                .field("metadata", metadata)
                .field("expires_at", expires_at)
                .finish(),
            Self::StaticHeaders {
                headers,
                metadata,
                expires_at,
            } => f
                .debug_struct("StaticHeaders")
                .field(
                    "headers",
                    &crate::redact::RedactedValues::of(headers.iter().map(|(name, _)| name)),
                )
                .field("metadata", metadata)
                .field("expires_at", expires_at)
                .finish(),
            Self::DynamicAuthorizer {
                metadata,
                expires_at,
            } => f
                .debug_struct("DynamicAuthorizer")
                .field("metadata", metadata)
                .field("expires_at", expires_at)
                .finish(),
            Self::None { metadata } => f.debug_struct("None").field("metadata", metadata).finish(),
        }
    }
}

/// Minimal request view passed to a dynamic authorizer.
pub struct HttpAuthorizationRequest<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a mut Vec<(String, String)>,
}

pub struct HttpAuthorizationResponse<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub status: u16,
}

/// Opaque correlation receipt for one concrete authorization attempt.
///
/// Provider clients return this receipt to the same authorizer when observing
/// the HTTP response. Dynamic authorizers can then invalidate only credential
/// material that actually signed the failed attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpAuthorizationReceipt(Option<u64>);

impl HttpAuthorizationReceipt {
    pub const fn untracked() -> Self {
        Self(None)
    }

    pub const fn tracked(generation: u64) -> Self {
        Self(Some(generation))
    }

    pub const fn generation(self) -> Option<u64> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpAuthorizationResponseAction {
    Propagate,
    RetryWithFreshAuthorization,
}

/// Typed content facts an authorizer may use to add protocol-owned request
/// headers without inspecting a provider's serialized JSON body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HttpAuthorizationContent {
    pub has_images: bool,
    /// Who the request continues: the user, or the agent's own work.
    pub initiator: HttpRequestInitiator,
}

/// Who a provider request continues.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HttpRequestInitiator {
    /// A new step of the conversation: the request's last input is a user
    /// message, or it has no input yet.
    #[default]
    User,
    /// The agent continuing its own work.
    Agent,
}

/// The secret values an authorizer recently signed requests with, for
/// [`HttpAuthorizer::visit_signed_credentials`]. An authorizer whose
/// credential rotates (a refreshed access token) records each value as it
/// signs with it. The last [`Self::HISTORY`] distinct values are kept, so a
/// response to a request signed just before a rotation still loses the
/// value it repeats. `Debug` shows no value.
#[derive(Default)]
pub struct SignedCredentials {
    values: parking_lot::Mutex<std::collections::VecDeque<String>>,
}

impl SignedCredentials {
    /// How many distinct values are kept.
    pub const HISTORY: usize = 4;

    pub fn new() -> Self {
        Self::default()
    }

    /// Keep `value`, a secret a request was just signed with.
    pub fn record(&self, value: &str) {
        if value.is_empty() {
            return;
        }
        let mut values = self.values.lock();
        if values.back().is_some_and(|last| last == value) {
            return;
        }
        values.retain(|known| known != value);
        values.push_back(value.to_string());
        while values.len() > Self::HISTORY {
            values.pop_front();
        }
    }

    /// Call `visit` with every kept value, oldest first.
    pub fn visit(&self, visit: &mut dyn FnMut(&str)) {
        for value in self.values.lock().iter() {
            visit(value);
        }
    }
}

impl std::fmt::Debug for SignedCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedCredentials").finish_non_exhaustive()
    }
}

/// Dynamic authorizer trait. Used when auth artifacts need to be computed
/// per request (ADC, refreshed OAuth bearer, etc.).
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait HttpAuthorizer: Send + Sync {
    /// Refresh any derived credential/routing metadata required before a
    /// provider selects its concrete request endpoint.
    async fn prepare_request(&self) -> Result<(), AuthError> {
        Ok(())
    }

    async fn authorize(&self, req: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError>;
    async fn authorize_with_receipt(
        &self,
        req: &mut HttpAuthorizationRequest<'_>,
    ) -> Result<HttpAuthorizationReceipt, AuthError> {
        self.authorize(req).await?;
        Ok(HttpAuthorizationReceipt::untracked())
    }
    async fn observe_response(
        &self,
        _response: &HttpAuthorizationResponse<'_>,
    ) -> Result<HttpAuthorizationResponseAction, AuthError> {
        Ok(HttpAuthorizationResponseAction::Propagate)
    }
    async fn observe_response_with_receipt(
        &self,
        _receipt: HttpAuthorizationReceipt,
        response: &HttpAuthorizationResponse<'_>,
    ) -> Result<HttpAuthorizationResponseAction, AuthError> {
        self.observe_response(response).await
    }

    /// Call `visit` with every secret value this authorizer put on a
    /// request, as sent: a bearer token, an API key, or a whole secret
    /// header value (`Basic <pair>`). An authorizer whose credential rotates
    /// keeps the values it signed with in a [`SignedCredentials`] and visits
    /// those. The default visits nothing.
    ///
    /// The default [`Self::redact_response_diagnostic`] removes each visited
    /// value from what a provider client reads from a response, so an
    /// authorizer that signs with a secret implements this.
    fn visit_signed_credentials(&self, _visit: &mut dyn FnMut(&str)) {}

    /// Remove the credential material this authorizer signed requests with
    /// from `diagnostic`: a response body, a message decoded from one, or a
    /// field a provider client logs. A proxy, gateway or provider can echo a
    /// request's headers in an error, the bearer included, so provider
    /// clients pass what they read from a response through this before they
    /// parse, log or return it; successful content is never passed. The
    /// default removes every value [`Self::visit_signed_credentials`]
    /// visits, by exact match, and then credential shapes
    /// ([`crate::redact::redact_credentials`]), so an authorizer that visits
    /// nothing still loses a credential a response repeats in a known shape.
    fn redact_response_diagnostic(&self, diagnostic: &mut String) {
        crate::redact::redact_credentials(diagnostic, |visit| {
            self.visit_signed_credentials(visit);
        });
    }

    fn label(&self) -> &str;

    fn append_content_headers(
        &self,
        _content: HttpAuthorizationContent,
        _headers: &mut Vec<(String, String)>,
    ) -> Result<(), AuthError> {
        Ok(())
    }

    /// Process-local persistence authority that scopes any derived credential
    /// cache owned by this authorizer.
    fn persistence_authority_id(&self) -> Option<super::ProviderAuthPersistenceId> {
        None
    }

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
    fn resolved_auth_envelope_debug_redacts_secret_and_header_values() {
        const SECRET: &str = "sk-live-secret-value";
        let envelopes = [
            ResolvedAuthEnvelope::InlineSecret {
                secret: SECRET.into(),
                metadata: AuthMetadata::default(),
                expires_at: None,
            },
            ResolvedAuthEnvelope::StaticHeaders {
                headers: vec![("x-api-key".into(), SECRET.into())],
                metadata: AuthMetadata::default(),
                expires_at: None,
            },
        ];
        let rendered = format!("{envelopes:?} {envelopes:#?}");
        assert!(!rendered.contains(SECRET), "secret leaked: {rendered}");
        assert!(rendered.contains("x-api-key"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    /// An authorizer that implements only the required methods still loses
    /// an authorization header a response repeats: the default redaction
    /// removes credential shapes, and keeps the text around them.
    #[test]
    fn default_redaction_removes_an_echoed_authorization_header() {
        struct RequiredMethodsOnly;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HttpAuthorizer for RequiredMethodsOnly {
            async fn authorize(
                &self,
                _req: &mut HttpAuthorizationRequest<'_>,
            ) -> Result<(), AuthError> {
                Ok(())
            }

            fn label(&self) -> &'static str {
                "required-methods-only"
            }
        }

        let credential = format!("madeup{}", "bearer0123".repeat(3));
        let mut diagnostic = format!(
            r#"{{"error":{{"message":"upstream echo: rejected request with header authorization: Bearer {credential}","type":"invalid_request_error"}}}}"#
        );
        RequiredMethodsOnly.redact_response_diagnostic(&mut diagnostic);
        assert_eq!(
            diagnostic,
            r#"{"error":{"message":"upstream echo: rejected request with header authorization: Bearer [REDACTED]","type":"invalid_request_error"}}"#
        );

        // An ordinary provider error that only talks about credentials is
        // returned as it was.
        let ordinary = r#"{"error":{"message":"You didn't provide an API key. You need to provide your API key in an Authorization header using Bearer auth (i.e. Authorization: Bearer YOUR_KEY). See https://docs.upstream.invalid/v1/models?page=2","type":"invalid_request_error","param":null,"code":null}}"#;
        let mut diagnostic = ordinary.to_string();
        RequiredMethodsOnly.redact_response_diagnostic(&mut diagnostic);
        assert_eq!(diagnostic, ordinary);
    }

    /// The default redaction removes what `visit_signed_credentials`
    /// visits: as sent, without its scheme, and a value a rotation replaced.
    #[test]
    fn default_redaction_removes_the_visited_credentials() {
        struct Rotating {
            signed: SignedCredentials,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HttpAuthorizer for Rotating {
            async fn authorize(
                &self,
                _req: &mut HttpAuthorizationRequest<'_>,
            ) -> Result<(), AuthError> {
                Ok(())
            }

            fn visit_signed_credentials(&self, visit: &mut dyn FnMut(&str)) {
                self.signed.visit(visit);
            }

            fn label(&self) -> &'static str {
                "rotating"
            }
        }

        let earlier = "madeupearliertoken".repeat(2);
        let current = "madeupcurrenttoken".repeat(2);
        let authorizer = Rotating {
            signed: SignedCredentials::new(),
        };
        authorizer.signed.record(&earlier);
        authorizer.signed.record(&current);
        let mut diagnostic = format!(
            "upstream echo: authorization: Bearer {earlier}, then credential {current} in http://upstream.invalid/?session={current}"
        );
        authorizer.redact_response_diagnostic(&mut diagnostic);
        assert_eq!(
            diagnostic,
            "upstream echo: authorization: Bearer [REDACTED], then credential [REDACTED] in http://upstream.invalid/?session=[REDACTED]"
        );
    }

    /// The history keeps the last distinct values, drops the oldest past
    /// its bound, ignores an empty value, and prints none of them.
    #[test]
    fn signed_credentials_keep_a_bounded_history_and_print_nothing() {
        let signed = SignedCredentials::new();
        let visited = |signed: &SignedCredentials| {
            let mut values = Vec::new();
            signed.visit(&mut |value| values.push(value.to_string()));
            values
        };
        signed.record("");
        assert!(visited(&signed).is_empty());
        for value in ["value-1", "value-2", "value-2", "value-1", "value-3"] {
            signed.record(value);
        }
        assert_eq!(visited(&signed), ["value-2", "value-1", "value-3"]);
        for value in ["value-4", "value-5"] {
            signed.record(value);
        }
        assert_eq!(visited(&signed).len(), SignedCredentials::HISTORY);
        assert_eq!(
            visited(&signed),
            ["value-1", "value-3", "value-4", "value-5"]
        );
        let rendered = format!("{signed:?} {signed:#?}");
        assert!(!rendered.contains("value-"), "{rendered}");
    }

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
