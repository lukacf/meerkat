//! Shared resolver helpers used by provider runtimes.
//!
//! Plan §6.11 closure: resolvers return credential material directly
//! (a `String` secret or a resolved `Arc<dyn HttpAuthorizer>` via the
//! external-resolver envelope's `__secret__` convention) instead of the
//! plan §6.11 deletion. Simple-secret paths cover
//! `InlineSecret` / `Env` / `ExternalResolver` (with `"__secret__"` key)
//! for `api_key` / `static_bearer` auth methods. The external-authorizer
//! resolver surfaces upstream AuthError by calling the resolver handle
//! but the real `Arc<dyn HttpAuthorizer>` is constructed by the
//! provider runtime at build time (cloud-specific clients depend on
//! SDKs that can't serde-roundtrip).

use meerkat_core::{AuthError, CredentialSourceSpec, ResolvedAuthEnvelope};

use crate::runtime::errors::ProviderAuthError;
use crate::runtime::registry::ResolverEnvironment;

/// Resolve a [`CredentialSourceSpec`] into a single secret string. Used
/// by api_key / static_bearer auth methods. Returns the resolved secret
/// directly; the provider runtime wraps it in a `StaticLease` with the
/// `__secret__` synthetic header for transport to `build_client`.
pub async fn resolve_simple_secret(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &crate::runtime::binding::ValidatedBinding,
) -> Result<String, ProviderAuthError> {
    match source {
        CredentialSourceSpec::InlineSecret { secret } => Ok(secret.clone()),
        CredentialSourceSpec::Env { env: var, fallback } => {
            // Single canonical owner of env-var credential resolution
            // policy (dogma §1). For each var name (primary + ordered
            // fallback chain), `RKAT_<VAR>` overrides `<VAR>`. The
            // factory body no longer encodes this policy inline.
            let candidates =
                std::iter::once(var.as_str()).chain(fallback.iter().map(String::as_str));
            for candidate in candidates {
                let rkat_override = if candidate.starts_with("RKAT_") {
                    None
                } else {
                    (env.env_lookup)(&format!("RKAT_{candidate}"))
                };
                if let Some(value) = rkat_override.or_else(|| (env.env_lookup)(candidate)) {
                    return Ok(value);
                }
            }
            Err(ProviderAuthError::Auth(AuthError::MissingSecret))
        }
        CredentialSourceSpec::ExternalResolver { handle } => {
            let resolver = env
                .external_resolvers
                .get(handle)
                .ok_or_else(|| ProviderAuthError::ExternalResolverMissing(handle.clone()))?;
            let envelope = resolver.resolve(binding).await?;
            extract_secret_from_envelope(envelope)
        }
        CredentialSourceSpec::Command { .. } => Err(ProviderAuthError::SourceResolutionFailed(
            "CredentialSourceSpec::Command requires a provider-specific runner; \
             not reachable from the simple-secret resolver"
                .into(),
        )),
        CredentialSourceSpec::FileDescriptor { .. } => {
            Err(ProviderAuthError::SourceResolutionFailed(
                "CredentialSourceSpec::FileDescriptor requires a host-scoped reader; \
                 not reachable from the simple-secret resolver"
                    .into(),
            ))
        }
        CredentialSourceSpec::ManagedStore { .. } | CredentialSourceSpec::PlatformDefault => {
            Err(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))
        }
    }
}

/// External auth for `external_authorizer` method. Calls the host-
/// registered resolver to surface any upstream `AuthError` before the
/// provider runtime constructs its concrete `HttpAuthorizer`. Returns
/// `Ok(())` on success — the provider runtime is responsible for
/// wiring the backend-specific authorizer (Bedrock SigV4, Vertex
/// GoogleAuth, Foundry AzureAd, etc.) into a `DynamicLease`.
pub async fn resolve_external_authorizer(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &crate::runtime::binding::ValidatedBinding,
) -> Result<(), ProviderAuthError> {
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
    let _envelope = resolver.resolve(binding).await?;
    Ok(())
}

/// Extract a simple secret from a `StaticHeaders` envelope using the
/// `"__secret__"` synthetic header convention. Errors on any other
/// envelope shape.
fn extract_secret_from_envelope(
    envelope: ResolvedAuthEnvelope,
) -> Result<String, ProviderAuthError> {
    match envelope {
        ResolvedAuthEnvelope::StaticHeaders { headers, .. } => headers
            .into_iter()
            .find_map(|(k, v)| if k == "__secret__" { Some(v) } else { None })
            .ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "external resolver produced StaticHeaders without a '__secret__' key; \
                     api_key/static_bearer path cannot extract credential material"
                        .into(),
                )
            }),
        ResolvedAuthEnvelope::DynamicAuthorizer { .. } => {
            Err(ProviderAuthError::SourceResolutionFailed(
                "external resolver returned DynamicAuthorizer envelope; \
                 use external_authorizer auth method instead"
                    .into(),
            ))
        }
        ResolvedAuthEnvelope::None { .. } => Err(ProviderAuthError::Auth(AuthError::MissingSecret)),
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
        assert_eq!(extract_secret_from_envelope(env).unwrap(), "sk-x");
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
    fn extract_dynamic_envelope_errors() {
        let env = ResolvedAuthEnvelope::DynamicAuthorizer {
            metadata: Default::default(),
            expires_at: None,
        };
        let err = extract_secret_from_envelope(env).unwrap_err();
        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
    }

    #[test]
    fn extract_none_envelope_errors() {
        let env = ResolvedAuthEnvelope::None {
            metadata: Default::default(),
        };
        let err = extract_secret_from_envelope(env).unwrap_err();
        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::MissingSecret)
        ));
    }
}
