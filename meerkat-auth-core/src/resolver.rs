//! Shared resolver helpers used by provider runtimes.
//!
//! Plan §6.11 closure: resolvers return typed credential material that
//! provider runtimes wrap into real leases. Simple-secret paths cover
//! `InlineSecret` / `Env` / `ExternalResolver` (only via the typed
//! `ResolvedAuthEnvelope::InlineSecret` variant — dogma §5 closure
//! rejects the `"__secret__"` synthetic-header-key convention) for
//! `api_key` / `static_bearer` auth methods. External-authorizer
//! resolution now returns typed auth material rather than `()`, so
//! provider runtimes no longer end in placeholder empty leases.

use std::sync::Arc;

use async_trait::async_trait;
#[cfg(not(target_arch = "wasm32"))]
use chrono::{DateTime, Utc};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, TokenKey, lease_snapshot_expires_at_datetime,
};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeasePhase, AuthLeaseSnapshot, LeaseKey,
};
use meerkat_core::{
    AnthropicAuthMetadata, AuthError, AuthLease, AuthMetadata, AuthMetadataDefaults,
    AuthRouteHints, CredentialSourceSpec, GoogleAuthMetadata, HttpAuthorizationRequest,
    HttpAuthorizer, OpenAiAuthMetadata, ProviderAuthMetadata, ResolvedAuthEnvelope,
};

use meerkat_llm_core::provider_runtime::binding::{DynamicLease, StaticLease, ValidatedBinding};
use meerkat_llm_core::provider_runtime::errors::ProviderAuthError;
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;

/// Resolve a [`CredentialSourceSpec`] into a single secret string. Used
/// by api_key / static_bearer auth methods. Returns the resolved secret
/// directly; the provider runtime wraps it via
/// `StaticLease::inline_secret` for transport to `build_client`.
pub async fn resolve_simple_secret(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
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
        CredentialSourceSpec::ManagedStore => resolve_managed_store_secret(env, binding).await,
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
        CredentialSourceSpec::PlatformDefault => {
            Err(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))
        }
    }
}

async fn resolve_managed_store_secret(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
) -> Result<String, ProviderAuthError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let store = env
            .token_store
            .as_ref()
            .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
        let key = TokenKey::from_connection_ref(&binding.connection_ref);
        let tokens = store
            .load(&key)
            .await
            .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
            .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
        let expected = managed_store_auth_mode(&binding.auth_profile.auth_method)?;
        if tokens.auth_mode != expected {
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "managed_store credential mode {:?} does not match binding auth_method '{}'",
                tokens.auth_mode, binding.auth_profile.auth_method,
            )));
        }
        tokens.primary_secret.ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "managed_store credential has no primary_secret".into(),
            )
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (env, binding);
        Err(ProviderAuthError::SourceResolutionFailed(
            "CredentialSourceSpec::ManagedStore requires a host TokenStore; \
             not available on the wasm32 target"
                .into(),
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn managed_store_auth_mode(auth_method: &str) -> Result<PersistedAuthMode, ProviderAuthError> {
    match auth_method {
        "api_key" | "api_key_express" | "foundry_api_key" => Ok(PersistedAuthMode::ApiKey),
        "static_bearer" | "bearer_api_key" | "bedrock_bearer" => {
            Ok(PersistedAuthMode::StaticBearer)
        }
        other => Err(ProviderAuthError::SourceResolutionFailed(format!(
            "auth_method '{other}' cannot resolve simple credentials from managed_store"
        ))),
    }
}

/// Static header injector used when an external resolver returns
/// `ResolvedAuthEnvelope::StaticHeaders`.
pub struct StaticHeadersAuthorizer {
    headers: Vec<(String, String)>,
    label: String,
}

impl StaticHeadersAuthorizer {
    pub fn new(headers: Vec<(String, String)>, label: impl Into<String>) -> Self {
        Self {
            headers,
            label: label.into(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl HttpAuthorizer for StaticHeadersAuthorizer {
    async fn authorize(&self, req: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError> {
        req.headers.extend(self.headers.iter().cloned());
        Ok(())
    }

    fn label(&self) -> &str {
        &self.label
    }
}

/// Merge auth-profile metadata defaults into a resolved metadata block,
/// then enforce the binding's metadata requirements.
pub fn finalize_auth_metadata(
    binding: &ValidatedBinding,
    metadata: AuthMetadata,
) -> Result<AuthMetadata, ProviderAuthError> {
    let defaults = &binding.auth_profile.metadata_defaults;
    if !binding.policy.allow_auth_override {
        if let (Some(default_workspace), Some(resolved_workspace)) = (
            defaults.workspace_id.as_deref(),
            metadata.workspace_id.as_deref(),
        ) && default_workspace != resolved_workspace
        {
            return Err(ProviderAuthError::Auth(AuthError::WorkspaceMismatch));
        }
        if let (Some(default_org), Some(resolved_org)) = (
            defaults.organization_id.as_deref(),
            metadata.organization_id.as_deref(),
        ) && default_org != resolved_org
        {
            return Err(ProviderAuthError::Auth(AuthError::WorkspaceMismatch));
        }
    }

    let metadata = merge_auth_metadata_defaults(defaults, metadata);
    enforce_metadata_requirements(binding, &metadata)?;
    Ok(metadata)
}

/// Return true when the binding allows silent refresh/token renewal.
pub fn refresh_allowed(binding: &ValidatedBinding) -> bool {
    binding.auth_profile.constraints.allow_refresh
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedOauthRefreshLifecycle {
    Untracked,
    AuthMachine(AuthLeaseSnapshot),
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedOauthAccess {
    Cached {
        tokens: PersistedTokens,
        expires_at: Option<DateTime<Utc>>,
    },
    Refresh {
        lifecycle: ManagedOauthRefreshLifecycle,
    },
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedOauthRefreshCompletion {
    pub expires_at: Option<DateTime<Utc>>,
    lease_snapshot: Option<AuthLeaseSnapshot>,
}

#[cfg(not(target_arch = "wasm32"))]
const MANAGED_OAUTH_REFRESH_WAIT_POLL_MS: u64 = 10;
#[cfg(not(target_arch = "wasm32"))]
const MANAGED_OAUTH_REFRESH_WAIT_TIMEOUT_SECS: u64 = 30;

#[cfg(not(target_arch = "wasm32"))]
pub async fn resolve_managed_oauth_access(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    persisted: &PersistedTokens,
) -> Result<ManagedOauthAccess, ProviderAuthError> {
    let Some(handle) = env.auth_lease_handle.as_ref() else {
        return resolve_untracked_managed_oauth_access(env, binding, persisted);
    };

    let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
    let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
    let refresh_wait_deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(MANAGED_OAUTH_REFRESH_WAIT_TIMEOUT_SECS);

    loop {
        let mut snapshot = handle.snapshot(&lease_key);
        if snapshot.phase.is_none() {
            let observed = reload_managed_oauth_tokens(env, binding, &token_key).await?;
            let Some(_) = acquire_managed_oauth_lease_if_snapshot(
                handle.as_ref(),
                &lease_key,
                &snapshot,
                managed_oauth_token_expires_at_epoch_secs(&observed),
            )?
            else {
                continue;
            };
            snapshot = handle.snapshot(&lease_key);
        }

        match snapshot.phase {
            Some(AuthLeasePhase::Valid)
                if snapshot_expires_at_is_fresh(env, snapshot.expires_at) =>
            {
                let Some(observed) = load_managed_oauth_tokens(env, &token_key).await? else {
                    if mark_managed_oauth_reauth_required_for_snapshot(
                        handle.as_ref(),
                        &lease_key,
                        &snapshot,
                    )? {
                        return Err(ProviderAuthError::Auth(AuthError::Expired));
                    }
                    continue;
                };
                if handle.snapshot(&lease_key) != snapshot {
                    continue;
                }
                let token_matches_lease =
                    managed_oauth_tokens_match_lease_snapshot(&observed, &snapshot);
                if token_matches_lease && observed.primary_secret.is_some() {
                    return Ok(ManagedOauthAccess::Cached {
                        tokens: observed,
                        expires_at: lease_snapshot_expires_at_datetime(&snapshot),
                    });
                }
                if !refresh_allowed(binding) {
                    if mark_managed_oauth_reauth_required_for_snapshot(
                        handle.as_ref(),
                        &lease_key,
                        &snapshot,
                    )? {
                        return Err(ProviderAuthError::Auth(AuthError::Expired));
                    }
                    continue;
                }
                match begin_managed_oauth_refresh(handle.as_ref(), &lease_key, &snapshot) {
                    Ok(Some(lifecycle)) => {
                        return Ok(ManagedOauthAccess::Refresh { lifecycle });
                    }
                    Ok(None) => continue,
                    Err(err) => match handle.snapshot(&lease_key).phase {
                        Some(AuthLeasePhase::Refreshing) => {
                            wait_for_managed_oauth_owner_refresh(
                                handle.as_ref(),
                                &lease_key,
                                refresh_wait_deadline,
                            )
                            .await?;
                        }
                        Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => continue,
                        Some(AuthLeasePhase::ReauthRequired) => {
                            return Err(ProviderAuthError::Auth(AuthError::Expired));
                        }
                        _ => {
                            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                                "AuthMachine lifecycle begin_refresh failed: {err}"
                            )));
                        }
                    },
                }
            }
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                if !refresh_allowed(binding) {
                    if mark_managed_oauth_reauth_required_for_snapshot(
                        handle.as_ref(),
                        &lease_key,
                        &snapshot,
                    )? {
                        return Err(ProviderAuthError::Auth(AuthError::Expired));
                    }
                    continue;
                }
                match begin_managed_oauth_refresh(handle.as_ref(), &lease_key, &snapshot) {
                    Ok(Some(lifecycle)) => {
                        return Ok(ManagedOauthAccess::Refresh { lifecycle });
                    }
                    Ok(None) => continue,
                    Err(err) => match handle.snapshot(&lease_key).phase {
                        Some(AuthLeasePhase::Refreshing) => {
                            wait_for_managed_oauth_owner_refresh(
                                handle.as_ref(),
                                &lease_key,
                                refresh_wait_deadline,
                            )
                            .await?;
                        }
                        Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => continue,
                        Some(AuthLeasePhase::ReauthRequired) => {
                            return Err(ProviderAuthError::Auth(AuthError::Expired));
                        }
                        _ => {
                            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                                "AuthMachine lifecycle begin_refresh failed: {err}"
                            )));
                        }
                    },
                }
            }
            Some(AuthLeasePhase::Refreshing) => {
                if !refresh_allowed(binding) {
                    return Err(ProviderAuthError::Auth(AuthError::Expired));
                }
                wait_for_managed_oauth_owner_refresh(
                    handle.as_ref(),
                    &lease_key,
                    refresh_wait_deadline,
                )
                .await?;
            }
            Some(AuthLeasePhase::ReauthRequired | AuthLeasePhase::Released) | None => {
                return Err(ProviderAuthError::Auth(AuthError::Expired));
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn acquire_managed_oauth_lease_if_snapshot(
    handle: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &LeaseKey,
    expected: &meerkat_core::handles::AuthLeaseSnapshot,
    expires_at: u64,
) -> Result<Option<meerkat_core::handles::AuthLeaseTransition>, ProviderAuthError> {
    handle
        .acquire_lease_if_snapshot(lease_key, expected, expires_at)
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle conditional acquire failed: {e}"
            ))
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn begin_managed_oauth_refresh(
    handle: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &LeaseKey,
    from_snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
) -> Result<Option<ManagedOauthRefreshLifecycle>, ProviderAuthError> {
    let Some(transition) = handle
        .begin_refresh_if_snapshot(lease_key, from_snapshot)
        .map_err(|err| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle begin_refresh failed: {err}"
            ))
        })?
    else {
        return Ok(None);
    };
    Ok(Some(ManagedOauthRefreshLifecycle::AuthMachine(
        AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Refreshing),
            expires_at: from_snapshot.expires_at,
            generation: transition.generation,
        },
    )))
}

#[cfg(not(target_arch = "wasm32"))]
type ManagedOauthRefreshOwner = (
    Arc<dyn meerkat_core::handles::AuthLeaseHandle>,
    LeaseKey,
    AuthLeaseSnapshot,
);

#[cfg(not(target_arch = "wasm32"))]
fn managed_oauth_refresh_owner_guard(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    lifecycle: &ManagedOauthRefreshLifecycle,
) -> Result<Option<ManagedOauthRefreshOwner>, ProviderAuthError> {
    let ManagedOauthRefreshLifecycle::AuthMachine(refreshing_snapshot) = lifecycle else {
        return Ok(None);
    };
    let handle = env.auth_lease_handle.as_ref().ok_or_else(|| {
        ProviderAuthError::SourceResolutionFailed(
            "AuthMachine refresh lifecycle missing auth lease handle".into(),
        )
    })?;
    let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
    Ok(Some((
        Arc::clone(handle),
        lease_key,
        refreshing_snapshot.clone(),
    )))
}

#[cfg(not(target_arch = "wasm32"))]
async fn wait_for_managed_oauth_owner_refresh(
    handle: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &LeaseKey,
    deadline: tokio::time::Instant,
) -> Result<(), ProviderAuthError> {
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(ProviderAuthError::Auth(AuthError::RefreshFailed(format!(
                "managed OAuth auth lease {lease_key} remained refreshing for {MANAGED_OAUTH_REFRESH_WAIT_TIMEOUT_SECS}s"
            ))));
        }

        tokio::time::sleep(std::time::Duration::from_millis(
            MANAGED_OAUTH_REFRESH_WAIT_POLL_MS,
        ))
        .await;

        match handle.snapshot(lease_key).phase {
            Some(AuthLeasePhase::Refreshing) => continue,
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                return Ok(());
            }
            Some(AuthLeasePhase::ReauthRequired) => {
                return Err(ProviderAuthError::Auth(AuthError::Expired));
            }
            Some(AuthLeasePhase::Released) | None => {
                return Err(ProviderAuthError::Auth(AuthError::Expired));
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn load_managed_oauth_tokens(
    env: &ResolverEnvironment,
    token_key: &TokenKey,
) -> Result<Option<PersistedTokens>, ProviderAuthError> {
    let store = env.token_store.as_ref().ok_or_else(|| {
        ProviderAuthError::SourceResolutionFailed(
            "managed OAuth owner refresh completed but TokenStore is unavailable".into(),
        )
    })?;
    store
        .load(token_key)
        .await
        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
async fn reload_managed_oauth_tokens(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    token_key: &TokenKey,
) -> Result<PersistedTokens, ProviderAuthError> {
    load_managed_oauth_tokens(env, token_key)
        .await?
        .ok_or_else(|| interactive_login_error(binding))
}

#[cfg(not(target_arch = "wasm32"))]
fn mark_managed_oauth_reauth_required_for_snapshot(
    handle: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &LeaseKey,
    expected: &meerkat_core::handles::AuthLeaseSnapshot,
) -> Result<bool, ProviderAuthError> {
    handle
        .mark_reauth_required_if_snapshot(lease_key, expected)
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle conditional mark_reauth_required failed: {e}"
            ))
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn mark_managed_oauth_refresh_failed_for_snapshot(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    expected: &meerkat_core::handles::AuthLeaseSnapshot,
    permanent: bool,
) -> Result<bool, ProviderAuthError> {
    let Some(handle) = env.auth_lease_handle.as_ref() else {
        return Ok(false);
    };
    let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
    handle
        .refresh_failed_if_snapshot(&lease_key, expected, permanent)
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle conditional refresh_failed failed: {e}"
            ))
        })
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn save_and_complete_managed_oauth_refresh(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    token_key: &TokenKey,
    lifecycle: ManagedOauthRefreshLifecycle,
    refreshed: &PersistedTokens,
) -> Result<ManagedOauthRefreshCompletion, ProviderAuthError> {
    let store = match env.token_store.as_ref() {
        Some(store) => store,
        None => {
            if let ManagedOauthRefreshLifecycle::AuthMachine(refreshing_snapshot) = &lifecycle {
                let _ = mark_managed_oauth_refresh_failed_for_snapshot(
                    env,
                    binding,
                    refreshing_snapshot,
                    false,
                );
            }
            return Err(ProviderAuthError::SourceResolutionFailed(
                "managed OAuth refresh completed but TokenStore is unavailable".into(),
            ));
        }
    };

    let Some((handle, lease_key, refreshing_snapshot)) =
        managed_oauth_refresh_owner_guard(env, binding, &lifecycle)?
    else {
        store
            .save(token_key, refreshed)
            .await
            .map_err(|save_error| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "TokenStore save failed for untracked managed OAuth refresh: {save_error}"
                ))
            })?;
        return Ok(ManagedOauthRefreshCompletion {
            expires_at: refreshed.expires_at,
            lease_snapshot: None,
        });
    };

    if handle.snapshot(&lease_key) != refreshing_snapshot {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "AuthMachine lifecycle changed before managed OAuth refreshed tokens could be saved"
                .into(),
        ));
    }

    if let Err(save_error) = store.save(token_key, refreshed).await {
        mark_managed_oauth_refresh_failed_for_snapshot(env, binding, &refreshing_snapshot, false)
            .map_err(|mark_error| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "TokenStore save failed before AuthMachine refresh completion: {save_error}; \
                     AuthMachine conditional refresh-failure rollback failed: {mark_error}"
            ))
        })?;
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "TokenStore save failed before AuthMachine refresh completion: {save_error}"
        )));
    }

    if handle.snapshot(&lease_key) != refreshing_snapshot {
        let cleared =
            clear_managed_oauth_refreshed_tokens_if_current(store, token_key, refreshed).await?;
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "AuthMachine lifecycle changed while managed OAuth refreshed tokens were saved; \
             stale_token_cleared={cleared}"
        )));
    }

    let refreshed_expires_at = managed_oauth_token_expires_at_epoch_secs(refreshed);
    let transition = match handle.complete_refresh_if_snapshot(
        &lease_key,
        &refreshing_snapshot,
        refreshed_expires_at,
        epoch_secs((env.now)()),
    ) {
        Ok(Some(transition)) => transition,
        Ok(None) => {
            let cleared =
                clear_managed_oauth_refreshed_tokens_if_current(store, token_key, refreshed)
                    .await?;
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle changed before managed OAuth refresh completion; \
                 stale_token_cleared={cleared}"
            )));
        }
        Err(err) => {
            let cleared =
                clear_managed_oauth_refreshed_tokens_if_current(store, token_key, refreshed)
                    .await?;
            let rollback_marked = mark_managed_oauth_refresh_failed_for_snapshot(
                env,
                binding,
                &refreshing_snapshot,
                false,
            )?;
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle complete_refresh failed after managed OAuth refreshed tokens \
                 were saved; stale_token_cleared={cleared}; rollback_marked={rollback_marked}: {err}"
            )));
        }
    };

    let snapshot = AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::Valid),
        expires_at: lease_expires_at_snapshot_arg(refreshed_expires_at),
        generation: transition.generation,
    };
    Ok(ManagedOauthRefreshCompletion {
        expires_at: lease_snapshot_expires_at_datetime(&snapshot).or(refreshed.expires_at),
        lease_snapshot: Some(snapshot),
    })
}

#[cfg(not(target_arch = "wasm32"))]
async fn clear_managed_oauth_refreshed_tokens_if_current(
    store: &Arc<dyn meerkat_core::auth::TokenStore>,
    token_key: &TokenKey,
    refreshed: &PersistedTokens,
) -> Result<bool, ProviderAuthError> {
    store
        .clear_if_current(token_key, refreshed)
        .await
        .map_err(|clear_error| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "stale managed OAuth TokenStore cleanup failed: {clear_error}"
            ))
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn lease_expires_at_snapshot_arg(expires_at: u64) -> Option<u64> {
    (expires_at != u64::MAX).then_some(expires_at)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn fail_managed_oauth_refresh(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    lifecycle: ManagedOauthRefreshLifecycle,
    permanent: bool,
) -> Result<(), ProviderAuthError> {
    let ManagedOauthRefreshLifecycle::AuthMachine(refreshing_snapshot) = lifecycle else {
        return Ok(());
    };
    mark_managed_oauth_refresh_failed_for_snapshot(env, binding, &refreshing_snapshot, permanent)?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn oauth_refresh_error_text_is_permanent(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("status=401")
        || error.contains("status=403")
        || (error.contains("status=400")
            && body_mentions_any(
                &error,
                &[
                    "invalid_grant",
                    "invalid_client",
                    "unauthorized_client",
                    "invalid_scope",
                    "access_denied",
                    "permission_denied",
                ],
            ))
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_untracked_managed_oauth_access(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    persisted: &PersistedTokens,
) -> Result<ManagedOauthAccess, ProviderAuthError> {
    if persisted
        .expires_at
        .is_none_or(|exp| exp - (env.now)() > chrono::Duration::seconds(60))
    {
        persisted
            .primary_secret
            .as_ref()
            .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
        return Ok(ManagedOauthAccess::Cached {
            tokens: persisted.clone(),
            expires_at: persisted.expires_at,
        });
    }
    if !refresh_allowed(binding) {
        return Err(ProviderAuthError::Auth(AuthError::Expired));
    }
    Ok(ManagedOauthAccess::Refresh {
        lifecycle: ManagedOauthRefreshLifecycle::Untracked,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn snapshot_expires_at_is_fresh(env: &ResolverEnvironment, expires_at: Option<u64>) -> bool {
    let Some(expires_at) = expires_at else {
        return true;
    };
    let expires_at = i64::try_from(expires_at).unwrap_or(i64::MAX);
    expires_at.saturating_sub((env.now)().timestamp()) > AUTH_LEASE_TTL_REFRESH_WINDOW_SECS as i64
}

#[cfg(not(target_arch = "wasm32"))]
fn epoch_secs(ts: DateTime<Utc>) -> u64 {
    ts.timestamp().max(0) as u64
}

#[cfg(not(target_arch = "wasm32"))]
fn managed_oauth_token_expires_at_epoch_secs(tokens: &PersistedTokens) -> u64 {
    tokens.expires_at.map(epoch_secs).unwrap_or(u64::MAX)
}

#[cfg(not(target_arch = "wasm32"))]
fn managed_oauth_tokens_match_lease_snapshot(
    tokens: &PersistedTokens,
    snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
) -> bool {
    let snapshot_expires_at = snapshot.expires_at.unwrap_or(u64::MAX);
    managed_oauth_token_expires_at_epoch_secs(tokens) == snapshot_expires_at
}

#[cfg(not(target_arch = "wasm32"))]
fn body_mentions_any(body: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| body.contains(needle))
}

/// Return the auth error to surface when runtime resolution would need an
/// interactive login that the binding does not permit.
pub fn interactive_login_error(binding: &ValidatedBinding) -> ProviderAuthError {
    if binding.auth_profile.constraints.allow_interactive_login {
        ProviderAuthError::Auth(AuthError::InteractiveLoginRequired)
    } else {
        ProviderAuthError::Auth(AuthError::MissingSecret)
    }
}

/// Materialize a real lease from a typed external-auth envelope.
pub fn materialize_external_auth_lease(
    binding: &ValidatedBinding,
    envelope: ResolvedAuthEnvelope,
    source_label: impl Into<String>,
) -> Result<Arc<dyn AuthLease>, ProviderAuthError> {
    let source_label = source_label.into();
    match envelope {
        ResolvedAuthEnvelope::InlineSecret {
            secret,
            metadata,
            expires_at,
        } => {
            let metadata = finalize_auth_metadata(binding, metadata)?;
            Ok(Arc::new(StaticLease::inline_secret(
                secret,
                metadata,
                expires_at,
                source_label,
            )))
        }
        ResolvedAuthEnvelope::StaticHeaders {
            headers,
            metadata,
            expires_at,
        } => {
            let metadata = finalize_auth_metadata(binding, metadata)?;
            let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(StaticHeadersAuthorizer::new(
                headers,
                format!("{source_label}:static_headers"),
            ));
            Ok(Arc::new(DynamicLease::new(
                authorizer,
                metadata,
                expires_at,
                source_label,
            )))
        }
        ResolvedAuthEnvelope::DynamicAuthorizer { .. } => {
            Err(ProviderAuthError::Auth(AuthError::HostOwnedUnavailable))
        }
        ResolvedAuthEnvelope::None { .. } => Err(ProviderAuthError::Auth(AuthError::MissingSecret)),
    }
}

/// External auth for `external_authorizer` method. Calls the host-
/// registered resolver and materializes the returned typed auth
/// material into a real lease so provider runtimes do not end in
/// placeholder empty leases.
pub async fn resolve_external_authorizer(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
) -> Result<Arc<dyn AuthLease>, ProviderAuthError> {
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
    let envelope = resolver.resolve(binding).await?;
    materialize_external_auth_lease(
        binding,
        envelope,
        // Wave-c C-1 follow-up: `ConnectionRef` has no `Display` impl by
        // wave-b design (the opaque `realm:binding` string form was
        // deleted so no code path silently ferries the join through the
        // runtime). Project realm/binding explicitly at this log/ident
        // site.
        format!(
            "external:{}:{}:{}",
            binding.connection_ref.realm.as_str(),
            binding.connection_ref.binding.as_str(),
            binding.auth_profile.id,
        ),
    )
}

/// Extract a simple secret from a resolved envelope. Dogma §5:
/// `ResolvedAuthEnvelope::InlineSecret` is the typed canonical
/// variant. `StaticHeaders` is intentionally rejected on
/// api_key/static_bearer paths so header material cannot become an
/// implicit secret shape.
fn extract_secret_from_envelope(
    envelope: ResolvedAuthEnvelope,
) -> Result<String, ProviderAuthError> {
    match envelope {
        ResolvedAuthEnvelope::InlineSecret { secret, .. } => Ok(secret),
        ResolvedAuthEnvelope::StaticHeaders { .. } => {
            Err(ProviderAuthError::SourceResolutionFailed(
                "external resolver returned StaticHeaders envelope; \
                 api_key/static_bearer path requires InlineSecret, \
                 or use external_authorizer for header material"
                    .into(),
            ))
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

fn merge_auth_metadata_defaults(
    defaults: &AuthMetadataDefaults,
    mut metadata: AuthMetadata,
) -> AuthMetadata {
    if metadata.organization_id.is_none() {
        metadata.organization_id = defaults.organization_id.clone();
    }
    if metadata.workspace_id.is_none() {
        metadata.workspace_id = defaults.workspace_id.clone();
    }
    if matches!(metadata.route_hints, AuthRouteHints::None) {
        metadata.route_hints = defaults.route_hints.clone();
    }
    metadata.provider_metadata = merge_provider_metadata(
        defaults.provider_metadata.clone(),
        metadata.provider_metadata,
    );
    metadata
}

fn merge_provider_metadata(
    defaults: Option<ProviderAuthMetadata>,
    resolved: Option<ProviderAuthMetadata>,
) -> Option<ProviderAuthMetadata> {
    match (defaults, resolved) {
        (None, other) | (other, None) => other,
        (
            Some(ProviderAuthMetadata::OpenAi(defaults)),
            Some(ProviderAuthMetadata::OpenAi(resolved)),
        ) => Some(ProviderAuthMetadata::OpenAi(OpenAiAuthMetadata {
            plan_type: resolved.plan_type.or(defaults.plan_type),
            user_id: resolved.user_id.or(defaults.user_id),
            account_id: resolved.account_id.or(defaults.account_id),
            is_fedramp: resolved.is_fedramp.or(defaults.is_fedramp),
            email: resolved.email.or(defaults.email),
        })),
        (
            Some(ProviderAuthMetadata::Anthropic(defaults)),
            Some(ProviderAuthMetadata::Anthropic(resolved)),
        ) => Some(ProviderAuthMetadata::Anthropic(AnthropicAuthMetadata {
            subscription_tier: resolved.subscription_tier.or(defaults.subscription_tier),
            aws_region: resolved.aws_region.or(defaults.aws_region),
            vertex_project_id: resolved.vertex_project_id.or(defaults.vertex_project_id),
            vertex_region: resolved.vertex_region.or(defaults.vertex_region),
            foundry_deployment: resolved.foundry_deployment.or(defaults.foundry_deployment),
        })),
        (
            Some(ProviderAuthMetadata::Google(defaults)),
            Some(ProviderAuthMetadata::Google(resolved)),
        ) => Some(ProviderAuthMetadata::Google(GoogleAuthMetadata {
            account_email: resolved.account_email.or(defaults.account_email),
            project_id: resolved.project_id.or(defaults.project_id),
            region: resolved.region.or(defaults.region),
            code_assist_tier: resolved.code_assist_tier.or(defaults.code_assist_tier),
        })),
        (_, resolved) => resolved,
    }
}

fn enforce_metadata_requirements(
    binding: &ValidatedBinding,
    metadata: &AuthMetadata,
) -> Result<(), ProviderAuthError> {
    if (binding.policy.require_metadata_account
        || binding.auth_profile.constraints.require_account_id)
        && metadata.account_id.is_none()
    {
        return Err(ProviderAuthError::Auth(AuthError::MissingRequiredMetadata(
            "account_id".into(),
        )));
    }

    if (binding.policy.require_metadata_workspace
        || binding.auth_profile.constraints.require_workspace_id)
        && metadata.workspace_id.is_none()
    {
        return Err(ProviderAuthError::Auth(AuthError::MissingRequiredMetadata(
            "workspace_id".into(),
        )));
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use crate::EphemeralTokenStore;
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat_core::auth::{PersistedTokens, TokenKey, TokenStore};
    use meerkat_core::{AuthProfile, AuthRouteHints, BindingPolicy, ConnectionRef, Provider};
    use meerkat_llm_core::provider_runtime::binding::{
        NormalizedAuthMethod, NormalizedBackendKind, ValidatedBinding,
    };

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
    fn extract_secret_static_headers_errors_for_simple_secret() {
        // Simple-secret resolvers fail closed: StaticHeaders are only
        // valid for external_authorizer leases, not as an implicit
        // api_key/static_bearer secret shape.
        let env = ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![("Authorization".into(), "Bearer sk-y".into())],
            metadata: Default::default(),
            expires_at: None,
        };
        let err = extract_secret_from_envelope(env).unwrap_err();
        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
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

    fn binding() -> ValidatedBinding {
        ValidatedBinding {
            connection_ref: ConnectionRef {
                realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
                binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
                profile: None,
            },
            provider: Provider::Gemini,
            backend: NormalizedBackendKind::Google(
                meerkat_core::provider_matrix::google::GoogleBackendKind::GoogleGenAi,
            ),
            auth: NormalizedAuthMethod::Google(
                meerkat_core::provider_matrix::google::GoogleAuthMethod::ExternalAuthorizer,
            ),
            backend_profile: Arc::new(meerkat_core::BackendProfile {
                id: "backend".into(),
                provider: Provider::Gemini,
                backend_kind: "google_genai".into(),
                base_url: None,
                options: serde_json::Value::Null,
            }),
            auth_profile: Arc::new(AuthProfile {
                id: "auth".into(),
                provider: Provider::Gemini,
                auth_method: "external_authorizer".into(),
                source: CredentialSourceSpec::ExternalResolver {
                    handle: "host".into(),
                },
                constraints: Default::default(),
                metadata_defaults: meerkat_core::AuthMetadataDefaults {
                    organization_id: Some("org-default".into()),
                    workspace_id: Some("ws-default".into()),
                    route_hints: AuthRouteHints::Google(Box::default()),
                    provider_metadata: Some(ProviderAuthMetadata::Google(GoogleAuthMetadata {
                        project_id: Some("proj-default".into()),
                        ..Default::default()
                    })),
                },
            }),
            policy: BindingPolicy::default(),
        }
    }

    fn simple_secret_binding(source: CredentialSourceSpec, auth_method: &str) -> ValidatedBinding {
        let mut binding = binding();
        binding.auth = NormalizedAuthMethod::Google(
            meerkat_core::provider_matrix::google::GoogleAuthMethod::ApiKey,
        );
        binding.auth_profile = Arc::new(AuthProfile {
            id: "managed".into(),
            provider: Provider::Gemini,
            auth_method: auth_method.into(),
            source,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        });
        binding
    }

    struct StaticEnvelopeResolver(ResolvedAuthEnvelope);

    #[async_trait::async_trait]
    impl meerkat_llm_core::provider_runtime::registry::ExternalAuthResolverHandle
        for StaticEnvelopeResolver
    {
        async fn resolve(
            &self,
            _binding: &ValidatedBinding,
        ) -> Result<ResolvedAuthEnvelope, AuthError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn simple_secret_external_static_headers_fails_closed() {
        let binding = simple_secret_binding(
            CredentialSourceSpec::ExternalResolver {
                handle: "host".into(),
            },
            "api_key",
        );
        let env = ResolverEnvironment::testing().with_external_resolver(
            "host",
            Arc::new(StaticEnvelopeResolver(
                ResolvedAuthEnvelope::StaticHeaders {
                    headers: vec![("Authorization".into(), "Bearer sk-y".into())],
                    metadata: Default::default(),
                    expires_at: None,
                },
            )),
        );

        let err = resolve_simple_secret(&binding.auth_profile.source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_source_reads_binding_scoped_token_store() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_connection_ref(&binding.connection_ref);
        store
            .save(&key, &PersistedTokens::api_key("sk-managed"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing().with_token_store(store);

        let secret = resolve_simple_secret(&binding.auth_profile.source, &env, &binding)
            .await
            .unwrap();

        assert_eq!(secret, "sk-managed");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_source_rejects_wrong_token_mode() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_connection_ref(&binding.connection_ref);
        store
            .save(&key, &PersistedTokens::static_bearer("bearer"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing().with_token_store(store);

        let err = resolve_simple_secret(&binding.auth_profile.source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
    }

    #[test]
    fn finalize_auth_metadata_merges_defaults() {
        let binding = binding();
        let metadata = finalize_auth_metadata(
            &binding,
            AuthMetadata {
                account_id: Some("acct-1".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(metadata.organization_id.as_deref(), Some("org-default"));
        assert_eq!(metadata.workspace_id.as_deref(), Some("ws-default"));
        assert!(matches!(metadata.route_hints, AuthRouteHints::Google(_)));
        match metadata.provider_metadata {
            Some(ProviderAuthMetadata::Google(google)) => {
                assert_eq!(google.project_id.as_deref(), Some("proj-default"));
            }
            other => panic!("unexpected provider metadata: {other:?}"),
        }
    }

    #[test]
    fn materialize_external_auth_headers_becomes_dynamic_lease() {
        let binding = binding();
        let lease = materialize_external_auth_lease(
            &binding,
            ResolvedAuthEnvelope::StaticHeaders {
                headers: vec![("Authorization".into(), "Bearer abc".into())],
                metadata: AuthMetadata {
                    account_id: Some("acct-1".into()),
                    ..Default::default()
                },
                expires_at: None,
            },
            "test",
        )
        .unwrap();
        assert!(matches!(
            lease.kind(),
            meerkat_core::ResolvedAuthKind::DynamicAuthorizer(_)
        ));
    }
}
