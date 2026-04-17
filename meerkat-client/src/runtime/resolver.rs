//! Shared resolver helpers used by provider runtimes.
//!
//! Phase 2 only covers the simple-secret shape: `InlineSecret` / `Env` /
//! `ExternalResolver` (via the `"__secret__"` synthetic header key) for
//! `api_key`/`static_bearer` auth methods, plus the external-authorizer
//! path. Managed-store and platform-default sources surface as
//! `InteractiveLoginRequired` — Phase 3 adds the managed store.

use meerkat_core::{AuthError, CredentialSourceSpec, ResolvedAuthEnvelope};

use crate::runtime::binding::ShimCredential;
use crate::runtime::errors::ProviderAuthError;
use crate::runtime::registry::ResolverEnvironment;

/// Resolve a [`CredentialSourceSpec`] into a single secret string. Used by
/// api_key / static_bearer auth methods.
pub async fn resolve_simple_secret(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &crate::runtime::binding::ValidatedBinding,
) -> Result<ShimCredential, ProviderAuthError> {
    match source {
        CredentialSourceSpec::InlineSecret { secret } => Ok(ShimCredential::Secret(secret.clone())),
        CredentialSourceSpec::Env { env: var } => match (env.env_lookup)(var) {
            Some(value) => Ok(ShimCredential::Secret(value)),
            None => Err(ProviderAuthError::Auth(AuthError::MissingSecret)),
        },
        CredentialSourceSpec::ExternalResolver { handle } => {
            let resolver = env
                .external_resolvers
                .get(handle)
                .ok_or_else(|| ProviderAuthError::ExternalResolverMissing(handle.clone()))?;
            let envelope = resolver.resolve(binding).await?;
            extract_secret_from_envelope(envelope)
        }
        CredentialSourceSpec::ManagedStore { .. } | CredentialSourceSpec::PlatformDefault => {
            Err(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))
        }
    }
}

/// External auth for `external_authorizer` method — does not require a raw
/// secret; produces `ShimCredential::Authorizer` so `build_client` can
/// reject it with the typed Phase-3-deferred error.
pub async fn resolve_external_authorizer(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &crate::runtime::binding::ValidatedBinding,
) -> Result<ShimCredential, ProviderAuthError> {
    let CredentialSourceSpec::ExternalResolver { handle } = source else {
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "external_authorizer auth requires CredentialSourceSpec::ExternalResolver, \
             got {source:?}",
        )));
    };
    let resolver = env
        .external_resolvers
        .get(handle)
        .ok_or_else(|| ProviderAuthError::ExternalResolverMissing(handle.clone()))?;
    // Call the resolver to surface any upstream AuthError, but the shim
    // treats the credential as an opaque Authorizer marker.
    let _envelope = resolver.resolve(binding).await?;
    Ok(ShimCredential::Authorizer)
}

/// Phase-2 convention: a resolver's `StaticHeaders` envelope carries a
/// `("__secret__", value)` pair when the caller expects a simple secret.
/// Real provider header emission is a Phase 3 concern.
fn extract_secret_from_envelope(
    envelope: ResolvedAuthEnvelope,
) -> Result<ShimCredential, ProviderAuthError> {
    match envelope {
        ResolvedAuthEnvelope::StaticHeaders { headers, .. } => headers
            .into_iter()
            .find_map(|(k, v)| if k == "__secret__" { Some(v) } else { None })
            .map(ShimCredential::Secret)
            .ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "external resolver produced StaticHeaders without a '__secret__' key; \
                     Phase 2 shim cannot extract credential material"
                        .into(),
                )
            }),
        ResolvedAuthEnvelope::DynamicAuthorizer { .. } => Ok(ShimCredential::Authorizer),
        ResolvedAuthEnvelope::None { .. } => Ok(ShimCredential::None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn extract_secret_happy_path() {
        let env = ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![("__secret__".into(), "sk-x".into())],
            metadata: Default::default(),
            expires_at: None,
        };
        let cred = extract_secret_from_envelope(env).unwrap();
        assert_eq!(cred, ShimCredential::Secret("sk-x".into()));
    }

    #[test]
    fn extract_secret_missing_key_errors() {
        let env = ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![("Authorization".into(), "Bearer x".into())],
            metadata: Default::default(),
            expires_at: None,
        };
        let err = extract_secret_from_envelope(env).unwrap_err();
        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
    }

    #[test]
    fn extract_dynamic_envelope_returns_authorizer() {
        let env = ResolvedAuthEnvelope::DynamicAuthorizer {
            metadata: Default::default(),
            expires_at: None,
        };
        let cred = extract_secret_from_envelope(env).unwrap();
        assert_eq!(cred, ShimCredential::Authorizer);
    }

    #[test]
    fn extract_none_envelope_returns_none() {
        let env = ResolvedAuthEnvelope::None {
            metadata: Default::default(),
        };
        let cred = extract_secret_from_envelope(env).unwrap();
        assert_eq!(cred, ShimCredential::None);
    }
}
