//! Shared resolver helpers used by provider runtimes.
//!
//! Plan §6.11 closure: resolvers return credential material directly
//! (a `String` secret). Simple-secret paths cover `InlineSecret` /
//! `Env` / `ExternalResolver` (via the typed
//! `ResolvedAuthEnvelope::InlineSecret` variant — dogma §5 closure
//! replaced the `"__secret__"` synthetic-header-key convention) for
//! `api_key` / `static_bearer` auth methods. The external-authorizer
//! resolver surfaces upstream AuthError by calling the resolver handle
//! but the real `Arc<dyn HttpAuthorizer>` is constructed by the
//! provider runtime at build time (cloud-specific clients depend on
//! SDKs that can't serde-roundtrip).

use meerkat_core::{AuthError, CredentialSourceSpec, ResolvedAuthEnvelope};

use crate::runtime::errors::ProviderAuthError;
use crate::runtime::registry::ResolverEnvironment;

/// Resolve a [`CredentialSourceSpec`] into a single secret string. Used
/// by api_key / static_bearer auth methods. Returns the resolved secret
/// directly; the provider runtime wraps it via
/// `StaticLease::inline_secret` for transport to `build_client`.
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
        #[cfg(not(target_arch = "wasm32"))]
        CredentialSourceSpec::Command {
            program,
            args,
            cwd,
            env: cmd_env,
            timeout_ms,
            refresh_interval_ms,
        } => {
            use crate::auth_store::{CommandCredentialRunner, CommandCredentialSpec};
            let spec = CommandCredentialSpec {
                program: program.clone(),
                args: args.clone(),
                cwd: cwd.clone(),
                env: cmd_env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                timeout_ms: *timeout_ms,
                refresh_interval_ms: *refresh_interval_ms,
            };
            let runner = CommandCredentialRunner::new(spec);
            let tokens = runner
                .resolve()
                .await
                .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?;
            tokens.primary_secret.ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "command returned no primary_secret in its persisted tokens payload".into(),
                )
            })
        }
        #[cfg(target_arch = "wasm32")]
        CredentialSourceSpec::Command { .. } => Err(ProviderAuthError::SourceResolutionFailed(
            "CredentialSourceSpec::Command requires a subprocess runner; \
             not available on the wasm32 target"
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

/// Extract a simple secret from a resolved envelope. Dogma §5:
/// `ResolvedAuthEnvelope::InlineSecret` is the typed canonical
/// variant. The legacy `StaticHeaders` shape is still accepted
/// transitively — a single header maps to the secret — but external
/// resolvers should produce the typed variant directly.
fn extract_secret_from_envelope(
    envelope: ResolvedAuthEnvelope,
) -> Result<String, ProviderAuthError> {
    match envelope {
        ResolvedAuthEnvelope::InlineSecret { secret, .. } => Ok(secret),
        ResolvedAuthEnvelope::StaticHeaders { headers, .. } => {
            if let [(_, value)] = headers.as_slice() {
                Ok(value.clone())
            } else {
                Err(ProviderAuthError::SourceResolutionFailed(
                    "external resolver returned StaticHeaders with \
                     multiple entries; api_key/static_bearer path \
                     requires InlineSecret or a single-entry \
                     StaticHeaders envelope"
                        .into(),
                ))
            }
        }
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
    fn extract_secret_inline_variant() {
        let env = ResolvedAuthEnvelope::InlineSecret {
            secret: "sk-x".into(),
            metadata: Default::default(),
            expires_at: None,
        };
        assert_eq!(extract_secret_from_envelope(env).unwrap(), "sk-x");
    }

    #[test]
    fn extract_secret_single_header_legacy() {
        // Back-compat: a single-header StaticHeaders envelope is still
        // accepted. Multi-header envelopes (which previously carried
        // the `__secret__` key alongside other wire headers) are an
        // error — external resolvers should use the typed InlineSecret
        // variant.
        let env = ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![("Authorization".into(), "Bearer sk-y".into())],
            metadata: Default::default(),
            expires_at: None,
        };
        assert_eq!(extract_secret_from_envelope(env).unwrap(), "Bearer sk-y");
    }

    #[test]
    fn extract_secret_multi_header_errors() {
        let env = ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![
                ("Authorization".into(), "Bearer x".into()),
                ("X-Provider-Id".into(), "acct".into()),
            ],
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
