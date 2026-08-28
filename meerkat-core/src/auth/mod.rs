//! Generic, provider-neutral auth types.
//!
//! This module owns the trait contracts (`AuthLease`, `HttpAuthorizer`),
//! error shapes, metadata shapes, and status projection. Concrete provider
//! runtimes live in `meerkat-client/src/providers/*`. `meerkat-core` stays
//! generic — no provider-specific fields or logic land here.

pub mod binding_use;
pub mod error;
pub mod lease;
pub mod lifecycle;
pub mod metadata;
pub mod principal;
pub mod status;
pub mod token_store;

pub use binding_use::{
    AuthBindingUseDecision, AuthBindingUseDenial, AuthBindingUseGateError, AuthBindingUseRequest,
    AuthBindingUseWitness, authorize_explicit_auth_binding_use,
    authorize_then_materialize_auth_binding,
};
pub use error::{AuthError, AuthErrorKind};
pub use lease::{
    AuthConstraints, AuthLease, AuthRefreshReason, HttpAuthorizationContent,
    HttpAuthorizationReceipt, HttpAuthorizationRequest, HttpAuthorizationResponse,
    HttpAuthorizationResponseAction, HttpAuthorizer, ResolvedAuthEnvelope, ResolvedAuthKind,
};
#[cfg(not(target_arch = "wasm32"))]
pub use lifecycle::{
    AuthLoginLifecycleGuard, AuthStatusRehydrateError, acquire_auth_login_lifecycle_guard,
    clear_tokens_and_publish_lifecycle_released_coordinated,
    clear_tokens_and_publish_lifecycle_released_coordinated_for_identity,
    rehydrate_durable_predecessor_for_mutation,
    rehydrate_durable_predecessor_for_mutation_for_identity, rehydrate_marked_tokens_for_status,
    rehydrate_marked_tokens_for_status_for_identity,
};
pub use lifecycle::{
    PublishedAuthStatus, TokenLifecycleClearError, clear_tokens_and_publish_lifecycle_released,
    clear_tokens_and_publish_lifecycle_released_for_identity, lease_snapshot_expires_at_datetime,
    mark_tokens_lifecycle_published_for_transition,
    oauth_status_projection_snapshot_from_newer_marker, persisted_auth_mode_is_directly_creatable,
    persisted_auth_mode_uses_oauth_login_lifecycle, persisted_token_expires_at_epoch_secs,
    project_published_auth_status, publish_token_lifecycle_acquired,
    publish_token_lifecycle_acquired_for_identity, publish_token_lifecycle_released,
    publish_token_lifecycle_released_for_identity, restore_token_lifecycle_snapshot,
    tokens_lifecycle_publication, tokens_lifecycle_publication_with_explicit_expiry,
    tokens_lifecycle_published, tokens_lifecycle_published_generation,
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
    CredentialMutationError, CredentialMutationFn, CredentialMutationOutcome, PersistedAuthMode,
    PersistedTokens, ProviderAuthPersistence, ProviderAuthPersistenceId, RefreshCoordinator,
    RefreshError, RefreshFailureDisposition, RefreshFailureObservation, RefreshFn, TokenKey,
    TokenStore, TokenStoreError,
};
