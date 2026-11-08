//! Generic, provider-neutral auth types.
//!
//! This module owns the trait contracts (`AuthLease`, `HttpAuthorizer`),
//! error shapes, metadata shapes, and status projection. Concrete provider
//! runtimes live in `crates/meerkat-client/src/providers/*`. `meerkat-core` stays
//! generic — no provider-specific fields or logic land here.

pub mod binding_use;
pub mod error;
pub mod http_egress;
pub mod lease;
pub mod lifecycle;
pub mod mcp_attachment_identity_key;
pub mod mcp_service_secret;
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
pub use http_egress::HttpEgressRefusal;
pub use lease::{
    AuthConstraints, AuthLease, AuthRefreshReason, HttpAuthorizationContent,
    HttpAuthorizationReceipt, HttpAuthorizationRequest, HttpAuthorizationResponse,
    HttpAuthorizationResponseAction, HttpAuthorizer, HttpRequestInitiator, ResolvedAuthEnvelope,
    ResolvedAuthKind,
};
#[cfg(not(target_arch = "wasm32"))]
pub use lifecycle::{
    AuthLoginLifecycleGuard, AuthLoginLifecycleRetention, AuthStatusRehydrateError,
    CredentialStatusRead, acquire_auth_login_lifecycle_guard,
    acquire_auth_login_lifecycle_guard_for_unattended,
    clear_tokens_and_publish_lifecycle_released_coordinated,
    clear_tokens_and_publish_lifecycle_released_coordinated_for_identity,
    clear_tokens_and_publish_lifecycle_released_coordinated_for_mode,
    observe_marked_tokens_for_status_for_identity, rehydrate_durable_predecessor_for_mutation,
    rehydrate_durable_predecessor_for_mutation_for_identity,
    rehydrate_durable_predecessor_for_mutation_with_access,
    rehydrate_loaded_marked_tokens_with_guard, rehydrate_marked_tokens_for_status,
    rehydrate_marked_tokens_for_status_for_identity,
    rehydrate_marked_tokens_for_status_for_identity_with_guard,
    rehydrate_marked_tokens_for_status_with_guard, retain_auth_login_lifecycle_exclusion,
    try_acquire_auth_login_lifecycle_guard,
};
#[cfg(not(target_arch = "wasm32"))]
pub use lifecycle::{
    BoundedLifecyclePublicationError, publish_token_lifecycle_acquired_for_identity_within,
};
pub use lifecycle::{
    PublishedAuthStatus, TokenLifecycleClearError, lease_snapshot_expires_at_datetime,
    mark_tokens_lifecycle_published_for_transition,
    oauth_status_projection_snapshot_from_newer_marker, persisted_auth_mode_is_directly_creatable,
    persisted_auth_mode_uses_oauth_login_lifecycle, persisted_token_expires_at_epoch_secs,
    project_published_auth_status, publish_token_lifecycle_acquired,
    publish_token_lifecycle_acquired_for_identity, publish_token_lifecycle_released,
    publish_token_lifecycle_released_for_identity, restore_token_lifecycle_snapshot,
    tokens_lifecycle_publication, tokens_lifecycle_publication_with_explicit_expiry,
    tokens_lifecycle_published, tokens_lifecycle_published_generation,
};
#[cfg(not(target_arch = "wasm32"))]
pub use mcp_attachment_identity_key::resolve_or_create_mcp_attachment_identity_key;
pub use mcp_attachment_identity_key::{
    McpAttachmentIdentityKey, McpAttachmentIdentityKeyError, load_mcp_attachment_identity_key,
};
pub use mcp_service_secret::{
    McpServiceSecretPresence, McpServiceSecretRef, McpServiceSecretRefusal, McpServiceSecretValue,
    McpServiceSecretValues, clear_mcp_service_secret, load_mcp_service_secret,
    observe_mcp_service_secret, save_mcp_service_secret,
};
pub use metadata::{
    AnthropicAuthMetadata, AnthropicRouteHints, AuthMetadata, AuthMetadataDefaults, AuthRouteHints,
    GoogleAuthMetadata, GoogleRouteHints, OpenAiAuthMetadata, OpenAiRouteHints,
    ProviderAuthMetadata,
};
pub use principal::{
    ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalContractError, PrincipalId,
    PrincipalKind, PrincipalQualification, PrincipalRef, TrustDomainId, VisibilityClass,
    can_observe_visibility, metadata_grants_no_visibility,
};
pub use status::{AuthErrorSummary, AuthStatus, AuthStatusPhase, CredentialObservation};
pub use token_store::{
    ConnectorClientSecretKey, CredentialMutationError, CredentialMutationFn,
    CredentialMutationOutcome, CredentialSlotRefusal, PersistedAuthMode, PersistedTokens,
    ProviderAuthPersistence, ProviderAuthPersistenceId, RefreshCoordinator, RefreshError,
    RefreshFailureDisposition, RefreshFailureObservation, RefreshFn, TokenKey, TokenStore,
    TokenStoreAccess, TokenStoreError, TokenStoreObservation, TokenStoreRefusal,
};
