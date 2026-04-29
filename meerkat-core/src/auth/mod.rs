//! Generic, provider-neutral auth types.
//!
//! This module owns the trait contracts (`AuthLease`, `HttpAuthorizer`),
//! error shapes, metadata shapes, and status projection. Concrete provider
//! runtimes live in `meerkat-client/src/providers/*`. `meerkat-core` stays
//! generic — no provider-specific fields or logic land here.

pub mod error;
pub mod lease;
pub mod lifecycle;
pub mod metadata;
pub mod principal;
pub mod status;
pub mod token_store;

pub use error::{AuthError, AuthErrorKind};
pub use lease::{
    AuthConstraints, AuthLease, AuthRefreshReason, HttpAuthorizationRequest, HttpAuthorizer,
    ResolvedAuthEnvelope, ResolvedAuthKind,
};
pub use lifecycle::{
    PublishedAuthStatus, TokenLifecycleClearError, clear_tokens_and_publish_lifecycle_released,
    lease_snapshot_expires_at_datetime, persisted_token_expires_at_epoch_secs,
    project_published_auth_status, publish_token_lifecycle_acquired,
    publish_token_lifecycle_released,
};
pub use metadata::{
    AnthropicAuthMetadata, AnthropicRouteHints, AuthMetadata, AuthMetadataDefaults, AuthRouteHints,
    GoogleAuthMetadata, GoogleRouteHints, OpenAiAuthMetadata, OpenAiRouteHints,
    ProviderAuthMetadata,
};
pub use principal::{
    ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalContractError, PrincipalId,
    PrincipalKind, PrincipalRef, VisibilityClass, can_observe_visibility,
    metadata_grants_no_visibility,
};
pub use status::{AuthErrorSummary, AuthStatus, AuthStatusPhase};
pub use token_store::{
    PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError, RefreshFn, TokenKey,
    TokenStore, TokenStoreError,
};
