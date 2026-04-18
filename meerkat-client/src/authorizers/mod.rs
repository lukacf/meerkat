//! Dynamic `HttpAuthorizer` implementations for cloud backends:
//! AWS (SigV4 for Bedrock), Google (ADC + metadata), Azure AD
//! (client-credentials OAuth2).
//!
//! Each authorizer acquires and caches a credential/token and adds the
//! appropriate `Authorization` (and service-specific) headers on every
//! call to [`meerkat_core::HttpAuthorizer::authorize`].

use std::sync::Arc;

/// Shared closure type for env-variable lookup. Used by authorizers that
/// want to remain hermetic in tests by taking a closure rather than
/// reading `std::env::var` directly. The process-env implementation is
/// `Arc::new(|k| std::env::var(k).ok())`.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[cfg(feature = "aws-auth")]
pub mod aws;
#[cfg(feature = "azure-auth")]
pub mod azure;
#[cfg(feature = "google-oauth")]
pub mod google;
pub mod static_bearer;

#[cfg(feature = "aws-auth")]
pub use aws::{AwsAuthError, AwsCredentialProvider, AwsStsAuthorizer};
#[cfg(feature = "azure-auth")]
pub use azure::{AzureAdAuthorizer, AzureAuthError, AzureClientCredentials};
#[cfg(feature = "google-oauth")]
pub use google::{GoogleAuthAuthorizer, GoogleAuthChain, GoogleAuthError};
pub use static_bearer::StaticBearerAuthorizer;
