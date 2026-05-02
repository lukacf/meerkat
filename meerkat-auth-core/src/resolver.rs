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
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;

use async_trait::async_trait;
#[cfg(not(target_arch = "wasm32"))]
use chrono::{DateTime, Utc};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, TokenKey, lease_snapshot_expires_at_datetime,
};
use meerkat_core::handles::AuthLeaseSnapshot;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::handles::{AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeasePhase, LeaseKey};
use meerkat_core::{
    AnthropicAuthMetadata, AuthError, AuthLease, AuthMetadata, AuthMetadataDefaults,
    AuthRouteHints, CredentialSourceSpec, GoogleAuthMetadata, HttpAuthorizationRequest,
    HttpAuthorizer, OpenAiAuthMetadata, ProviderAuthMetadata, ResolvedAuthEnvelope,
};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::{AuthRefreshReason, ResolvedAuthKind};

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
    Ok(
        resolve_simple_secret_with_auth_context(source, env, binding)
            .await?
            .secret,
    )
}

/// Resolved simple-secret material plus the AuthMachine snapshot that
/// authorized it, when the secret came from lease-bound durable storage.
pub struct ResolvedSimpleSecret {
    pub secret: String,
    pub auth_lease_snapshot: Option<AuthLeaseSnapshot>,
}

pub async fn resolve_simple_secret_with_auth_context(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
) -> Result<ResolvedSimpleSecret, ProviderAuthError> {
    match source {
        CredentialSourceSpec::InlineSecret { secret } => Ok(ResolvedSimpleSecret {
            secret: secret.clone(),
            auth_lease_snapshot: None,
        }),
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
                    return Ok(ResolvedSimpleSecret {
                        secret: value,
                        auth_lease_snapshot: None,
                    });
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
            Ok(ResolvedSimpleSecret {
                secret: extract_secret_from_envelope(envelope)?,
                auth_lease_snapshot: None,
            })
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
            let secret = tokens.primary_secret.ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "command returned no primary_secret in its persisted tokens payload".into(),
                )
            })?;
            Ok(ResolvedSimpleSecret {
                secret,
                auth_lease_snapshot: None,
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
) -> Result<ResolvedSimpleSecret, ProviderAuthError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let expected = managed_store_auth_mode(&binding.auth_profile.auth_method)?;
        let resolved = resolve_lease_bound_stored_tokens(env, binding, expected).await?;
        let secret = resolved.tokens.primary_secret.ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "managed_store credential has no primary_secret".into(),
            )
        })?;
        Ok(ResolvedSimpleSecret {
            secret,
            auth_lease_snapshot: Some(resolved.lease_snapshot),
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
pub struct ManagedStoreTokens {
    pub store: Arc<dyn TokenStore>,
    pub key: TokenKey,
    pub tokens: PersistedTokens,
    pub lifecycle_snapshot: Option<meerkat_core::handles::AuthLeaseSnapshot>,
    pub lifecycle: ManagedStoreLifecycle,
    #[doc(hidden)]
    pub lifecycle_guard: Option<meerkat_core::AuthLoginLifecycleGuard>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedStoreLifecycle {
    Authorized,
    RefreshRequired,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OAuthLifecycleMarkerRelation {
    Matches,
    TokenNewer,
    TokenStale,
    Invalid,
}

#[cfg(not(target_arch = "wasm32"))]
fn oauth_lifecycle_publication_time_relation(
    marker: Option<u64>,
    snapshot: Option<u64>,
) -> Option<OAuthLifecycleMarkerRelation> {
    match (marker, snapshot) {
        (Some(marker), Some(snapshot)) if marker == snapshot => None,
        (Some(marker), Some(snapshot)) if marker > snapshot => {
            Some(OAuthLifecycleMarkerRelation::TokenNewer)
        }
        (Some(_), Some(_)) => Some(OAuthLifecycleMarkerRelation::TokenStale),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn oauth_lifecycle_marker_relation(
    tokens: &PersistedTokens,
    snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
) -> OAuthLifecycleMarkerRelation {
    let Some(publication) = meerkat_core::tokens_lifecycle_publication_with_explicit_expiry(tokens)
    else {
        return OAuthLifecycleMarkerRelation::Invalid;
    };
    let token_expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(tokens);
    if publication.expires_at != token_expires_at {
        return OAuthLifecycleMarkerRelation::Invalid;
    }
    if !snapshot.credential_present {
        return OAuthLifecycleMarkerRelation::TokenStale;
    }
    let generation_matches = publication
        .generation
        .is_some_and(|generation| generation == snapshot.generation);
    let snapshot_expires_at = snapshot.expires_at.unwrap_or(u64::MAX);

    if let (Some(marker_published_at), Some(snapshot_published_at)) = (
        publication.credential_published_at_millis,
        snapshot.credential_published_at_millis,
    ) {
        return match marker_published_at.cmp(&snapshot_published_at) {
            std::cmp::Ordering::Greater => OAuthLifecycleMarkerRelation::TokenNewer,
            std::cmp::Ordering::Less => OAuthLifecycleMarkerRelation::TokenStale,
            std::cmp::Ordering::Equal => {
                if token_expires_at == snapshot_expires_at && generation_matches {
                    OAuthLifecycleMarkerRelation::Matches
                } else {
                    OAuthLifecycleMarkerRelation::Invalid
                }
            }
        };
    }

    if let Some(relation) = oauth_lifecycle_publication_time_relation(
        publication.credential_published_at_millis,
        snapshot.credential_published_at_millis,
    ) {
        return relation;
    }

    match token_expires_at.cmp(&snapshot_expires_at) {
        std::cmp::Ordering::Greater => return OAuthLifecycleMarkerRelation::TokenNewer,
        std::cmp::Ordering::Less => return OAuthLifecycleMarkerRelation::TokenStale,
        std::cmp::Ordering::Equal => {
            if generation_matches {
                return OAuthLifecycleMarkerRelation::Matches;
            }
        }
    }

    OAuthLifecycleMarkerRelation::Invalid
}

#[cfg(not(target_arch = "wasm32"))]
fn oauth_lifecycle_marker_payload_valid_for_tokens(tokens: &PersistedTokens) -> bool {
    let Some(publication) = meerkat_core::tokens_lifecycle_publication_with_explicit_expiry(tokens)
    else {
        return false;
    };
    publication.expires_at == meerkat_core::persisted_token_expires_at_epoch_secs(tokens)
}

#[cfg(not(target_arch = "wasm32"))]
fn oauth_lifecycle_snapshot_allows_marker_rehydrate(
    phase: AuthStatusPhase,
    snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
    tokens: &PersistedTokens,
) -> bool {
    if phase != AuthStatusPhase::Unknown
        || snapshot.credential_present
        || !meerkat_core::tokens_lifecycle_published(tokens)
    {
        return false;
    }
    if snapshot.generation == 0 && snapshot.phase.is_none() {
        return true;
    }
    snapshot.phase == Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired)
}

#[cfg(not(target_arch = "wasm32"))]
fn restore_oauth_lifecycle_after_marker_save_failure(
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &meerkat_core::handles::LeaseKey,
    snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
    tokens: &PersistedTokens,
) -> String {
    let expires_at = snapshot
        .expires_at
        .or_else(|| Some(meerkat_core::persisted_token_expires_at_epoch_secs(tokens)));
    auth_lease
        .restore_auth_lifecycle_snapshot(lease_key, snapshot, expires_at)
        .err()
        .map(|e| format!("; AuthMachine lifecycle rollback failed: {e}"))
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn persisted_token_material_matches(left: &PersistedTokens, right: &PersistedTokens) -> bool {
    left.auth_mode == right.auth_mode
        && left.primary_secret == right.primary_secret
        && left.refresh_token == right.refresh_token
        && left.id_token == right.id_token
        && left.expires_at == right.expires_at
        && left.last_refresh == right.last_refresh
        && left.scopes == right.scopes
        && left.account_id == right.account_id
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn load_managed_store_tokens_with_lifecycle(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
) -> Result<ManagedStoreTokens, ProviderAuthError> {
    let store = env
        .token_store
        .as_ref()
        .ok_or_else(|| interactive_login_error(binding))?
        .clone();
    let key = TokenKey::from_connection_ref(&binding.connection_ref);
    let lease_key = meerkat_core::handles::LeaseKey::from_connection_ref(&binding.connection_ref);
    let lifecycle_guard = if crate::auth_store::persisted_auth_mode_for_auth_method(
        &binding.auth_profile.auth_method,
    )
    .is_some_and(crate::auth_store::persisted_auth_mode_is_oauth_login)
    {
        Some(meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await)
    } else {
        None
    };
    let mut tokens = store
        .load(&key)
        .await
        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
        .ok_or_else(|| interactive_login_error(binding))?;
    let expected_mode = require_persisted_auth_mode(&tokens, &binding.auth_profile.auth_method)?;
    let is_oauth_login = crate::auth_store::persisted_auth_mode_is_oauth_login(expected_mode);
    if is_oauth_login && !oauth_lifecycle_marker_payload_valid_for_tokens(&tokens) {
        return Err(interactive_login_error(binding));
    }

    let now = (env.now)();
    let token_phase = AuthStatusPhase::from_lease_expires_at(
        now,
        Some(meerkat_core::persisted_token_expires_at_epoch_secs(&tokens)),
    );
    let lifecycle = if token_phase == AuthStatusPhase::Expired {
        ManagedStoreLifecycle::RefreshRequired
    } else {
        ManagedStoreLifecycle::Authorized
    };

    if let Some(auth_lease) = env.auth_lease_handle.as_ref() {
        let mut snapshot = auth_lease.snapshot(&lease_key);
        let mut phase = AuthStatusPhase::from_lease_snapshot(now, &snapshot);
        if is_oauth_login
            && oauth_lifecycle_snapshot_allows_marker_rehydrate(phase, &snapshot, &tokens)
        {
            let previous_snapshot = snapshot.clone();
            let transition = meerkat_core::publish_token_lifecycle_acquired(
                auth_lease.as_ref(),
                &binding.connection_ref,
                &tokens,
            )
            .map_err(|e| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "AuthMachine lifecycle acquire failed: {e}"
                ))
            })?;
            let marked =
                meerkat_core::mark_tokens_lifecycle_published_for_transition(&tokens, transition);
            if marked != tokens {
                if let Err(save_error) = store.save(&key, &marked).await {
                    let rollback = restore_oauth_lifecycle_after_marker_save_failure(
                        auth_lease.as_ref(),
                        &lease_key,
                        &previous_snapshot,
                        &tokens,
                    );
                    return Err(ProviderAuthError::SourceResolutionFailed(format!(
                        "TokenStore lifecycle marker generation save failed after AuthMachine lifecycle acquire: {save_error}{rollback}"
                    )));
                }
                tokens = marked;
            }
            snapshot = auth_lease.snapshot(&lease_key);
            phase = AuthStatusPhase::from_lease_snapshot(now, &snapshot);
        }
        if is_oauth_login
            && matches!(
                phase,
                AuthStatusPhase::Valid | AuthStatusPhase::Expiring | AuthStatusPhase::Expired
            )
        {
            match oauth_lifecycle_marker_relation(&tokens, &snapshot) {
                OAuthLifecycleMarkerRelation::Matches => {}
                OAuthLifecycleMarkerRelation::TokenNewer => {
                    let previous_snapshot = snapshot.clone();
                    let transition = meerkat_core::publish_token_lifecycle_acquired(
                        auth_lease.as_ref(),
                        &binding.connection_ref,
                        &tokens,
                    )
                    .map_err(|e| {
                        ProviderAuthError::SourceResolutionFailed(format!(
                            "AuthMachine lifecycle acquire failed: {e}"
                        ))
                    })?;
                    let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                        &tokens, transition,
                    );
                    if marked != tokens {
                        if let Err(save_error) = store.save(&key, &marked).await {
                            let rollback = restore_oauth_lifecycle_after_marker_save_failure(
                                auth_lease.as_ref(),
                                &lease_key,
                                &previous_snapshot,
                                &tokens,
                            );
                            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                                "TokenStore lifecycle marker save failed after shared OAuth credential adoption: {save_error}{rollback}"
                            )));
                        }
                        tokens = marked;
                    }
                    snapshot = auth_lease.snapshot(&lease_key);
                    phase = AuthStatusPhase::from_lease_snapshot(now, &snapshot);
                }
                OAuthLifecycleMarkerRelation::TokenStale
                | OAuthLifecycleMarkerRelation::Invalid => {
                    return Err(interactive_login_error(binding));
                }
            }
        }
        if !is_oauth_login
            && phase == AuthStatusPhase::Unknown
            && snapshot.generation == 0
            && snapshot.phase.is_none()
            && !snapshot.credential_present
        {
            return Ok(managed_store_tokens(
                store,
                key,
                tokens,
                Some(snapshot),
                lifecycle,
                lifecycle_guard,
            ));
        }

        return match phase {
            AuthStatusPhase::Valid | AuthStatusPhase::Expiring => Ok(managed_store_tokens(
                store,
                key,
                tokens,
                Some(snapshot),
                lifecycle,
                lifecycle_guard,
            )),
            AuthStatusPhase::Expired => Ok(managed_store_tokens(
                store,
                key,
                tokens,
                Some(snapshot),
                ManagedStoreLifecycle::RefreshRequired,
                lifecycle_guard,
            )),
            AuthStatusPhase::ReauthRequired
            | AuthStatusPhase::RefreshFailed
            | AuthStatusPhase::Unknown => Err(interactive_login_error(binding)),
        };
    }

    if is_oauth_login {
        Err(interactive_login_error(binding))
    } else {
        Ok(managed_store_tokens(
            store,
            key,
            tokens,
            None,
            lifecycle,
            lifecycle_guard,
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn persisted_auth_mode_for_method(
    auth_method: &str,
) -> Result<PersistedAuthMode, ProviderAuthError> {
    crate::auth_store::persisted_auth_mode_for_auth_method(auth_method).ok_or_else(|| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "auth_method '{auth_method}' cannot resolve persisted credentials from TokenStore"
        ))
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn persisted_auth_mode_mismatch(
    tokens: &PersistedTokens,
    auth_method: &str,
    expected: PersistedAuthMode,
) -> ProviderAuthError {
    ProviderAuthError::SourceResolutionFailed(format!(
        "persisted credential mode {:?} does not match binding auth_method '{}' (expected {:?})",
        tokens.auth_mode, auth_method, expected,
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn managed_store_tokens(
    store: Arc<dyn TokenStore>,
    key: TokenKey,
    tokens: PersistedTokens,
    lifecycle_snapshot: Option<meerkat_core::handles::AuthLeaseSnapshot>,
    lifecycle: ManagedStoreLifecycle,
    lifecycle_guard: Option<meerkat_core::AuthLoginLifecycleGuard>,
) -> ManagedStoreTokens {
    ManagedStoreTokens {
        store,
        key,
        tokens,
        lifecycle_snapshot,
        lifecycle,
        lifecycle_guard,
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn publish_managed_store_tokens_lifecycle(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    tokens: &PersistedTokens,
) -> Result<meerkat_core::handles::AuthLeaseTransition, ProviderAuthError> {
    if AuthStatusPhase::from_lease_expires_at(
        (env.now)(),
        Some(meerkat_core::persisted_token_expires_at_epoch_secs(tokens)),
    ) == AuthStatusPhase::Expired
    {
        return Err(ProviderAuthError::Auth(AuthError::Expired));
    }
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;
    let transition = meerkat_core::publish_token_lifecycle_acquired(
        auth_lease.as_ref(),
        &binding.connection_ref,
        tokens,
    )
    .map_err(|e| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "AuthMachine lifecycle acquire failed: {e}"
        ))
    })?;
    require_credential_lifecycle_authority(env, binding)?;
    Ok(transition)
}

#[cfg(not(target_arch = "wasm32"))]
fn epoch_secs(ts: chrono::DateTime<chrono::Utc>) -> u64 {
    ts.timestamp().max(0) as u64
}

#[cfg(not(target_arch = "wasm32"))]
pub fn begin_managed_store_oauth_refresh_lifecycle(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    previous: &mut ManagedStoreTokens,
) -> Result<bool, ProviderAuthError> {
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;
    let lease_key = meerkat_core::handles::LeaseKey::from_connection_ref(&binding.connection_ref);
    let current_snapshot = auth_lease.snapshot(&lease_key);
    if current_snapshot.phase == Some(meerkat_core::handles::AuthLeasePhase::Refreshing) {
        previous.lifecycle_snapshot = Some(current_snapshot);
        return Ok(false);
    }
    if let Some(expected) = previous.lifecycle_snapshot.as_ref()
        && &current_snapshot != expected
    {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "AuthMachine lifecycle changed before OAuth refresh; discarding stale refresh attempt"
                .into(),
        ));
    }
    match current_snapshot.phase {
        Some(
            meerkat_core::handles::AuthLeasePhase::Valid
            | meerkat_core::handles::AuthLeasePhase::Expiring,
        ) if current_snapshot.credential_present => {
            auth_lease.begin_refresh(&lease_key).map_err(|e| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "AuthMachine lifecycle begin_refresh failed: {e}"
                ))
            })?;
            previous.lifecycle_snapshot = Some(auth_lease.snapshot(&lease_key));
            Ok(true)
        }
        _ => Err(ProviderAuthError::Auth(AuthError::Expired)),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn mark_managed_store_oauth_refresh_failed(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    refresh_started: bool,
    permanent: bool,
) -> Result<(), ProviderAuthError> {
    if !refresh_started {
        return Ok(());
    }
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;
    let lease_key = meerkat_core::handles::LeaseKey::from_connection_ref(&binding.connection_ref);
    if auth_lease.snapshot(&lease_key).phase
        != Some(meerkat_core::handles::AuthLeasePhase::Refreshing)
    {
        return Ok(());
    }
    auth_lease
        .refresh_failed(&lease_key, permanent)
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle refresh_failed failed: {e}"
            ))
        })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn managed_store_oauth_refresh_failure_coordinator(
    inner: Arc<dyn RefreshCoordinator>,
    env: ResolverEnvironment,
    binding: ValidatedBinding,
    refresh_started: bool,
) -> Arc<dyn RefreshCoordinator> {
    let pre_claim_guard =
        ManagedStoreOAuthRefreshPreClaimGuard::new(env.clone(), binding.clone(), refresh_started);
    Arc::new(ManagedStoreOAuthRefreshFailureCoordinator {
        inner,
        env,
        binding,
        refresh_started,
        pre_claim_guard,
    })
}

#[cfg(not(target_arch = "wasm32"))]
struct ManagedStoreOAuthRefreshPreClaimGuard {
    env: ResolverEnvironment,
    binding: ValidatedBinding,
    refresh_started: bool,
    active: std::sync::atomic::AtomicBool,
}

#[cfg(not(target_arch = "wasm32"))]
impl ManagedStoreOAuthRefreshPreClaimGuard {
    fn new(
        env: ResolverEnvironment,
        binding: ValidatedBinding,
        refresh_started: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            env,
            binding,
            refresh_started,
            active: std::sync::atomic::AtomicBool::new(refresh_started),
        })
    }

    fn disarm(&self) {
        self.active
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn fail_if_unclaimed(&self) -> Result<(), ProviderAuthError> {
        if self.active.swap(false, std::sync::atomic::Ordering::SeqCst) {
            mark_managed_store_oauth_refresh_failed(
                &self.env,
                &self.binding,
                self.refresh_started,
                false,
            )?;
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for ManagedStoreOAuthRefreshPreClaimGuard {
    fn drop(&mut self) {
        let _ = self.fail_if_unclaimed();
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ManagedStoreOAuthRefreshFailureCoordinator {
    inner: Arc<dyn RefreshCoordinator>,
    env: ResolverEnvironment,
    binding: ValidatedBinding,
    refresh_started: bool,
    pre_claim_guard: Arc<ManagedStoreOAuthRefreshPreClaimGuard>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ManagedStoreOAuthRefreshFailureCoordinator {
    fn wrap_refresh_fn(&self, refresh_fn: RefreshFn) -> RefreshFn {
        let env = self.env.clone();
        let binding = self.binding.clone();
        let refresh_started = self.refresh_started;
        let pre_claim_guard = Arc::clone(&self.pre_claim_guard);
        Box::new(move || {
            pre_claim_guard.disarm();
            Box::pin(async move {
                let result = refresh_fn().await;
                if let Err(err) = result.as_ref() {
                    let permanent =
                        managed_store_oauth_refresh_failure_is_permanent(&err.to_string());
                    if let Err(lifecycle_err) = mark_managed_store_oauth_refresh_failed(
                        &env,
                        &binding,
                        refresh_started,
                        permanent,
                    ) {
                        return Err(RefreshError::Refresh(format!("{err}; {lifecycle_err}")));
                    }
                }
                result
            })
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl RefreshCoordinator for ManagedStoreOAuthRefreshFailureCoordinator {
    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        let result = self
            .inner
            .with_refresh(key, self.wrap_refresh_fn(refresh_fn))
            .await;
        if let Err(err) = result.as_ref()
            && let Err(lifecycle_err) = self.pre_claim_guard.fail_if_unclaimed()
        {
            return Err(RefreshError::Refresh(format!("{err}; {lifecycle_err}")));
        }
        if result.is_ok() {
            self.pre_claim_guard.disarm();
        }
        result
    }

    async fn with_forced_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        let result = self
            .inner
            .with_forced_refresh(key, self.wrap_refresh_fn(refresh_fn))
            .await;
        if let Err(err) = result.as_ref()
            && let Err(lifecycle_err) = self.pre_claim_guard.fail_if_unclaimed()
        {
            return Err(RefreshError::Refresh(format!("{err}; {lifecycle_err}")));
        }
        if result.is_ok() {
            self.pre_claim_guard.disarm();
        }
        result
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn managed_store_oauth_refresh_failure_is_permanent(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    if let Some((status, body)) = parse_oauth_endpoint_failure(&message) {
        return managed_store_oauth_endpoint_failure_is_permanent(status, body);
    }

    managed_store_oauth_body_mentions_permanent_failure(&message)
        || message.contains("status=401")
        || message.contains("status=403")
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_oauth_endpoint_failure(message: &str) -> Option<(u16, &str)> {
    let (_, status_and_rest) = message.split_once("status=")?;
    let status_len = status_and_rest
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    if status_len == 0 {
        return None;
    }
    let status = status_and_rest[..status_len].parse().ok()?;
    let body = status_and_rest
        .split_once("body=")
        .map(|(_, body)| body)
        .unwrap_or("");
    Some((status, body))
}

#[cfg(not(target_arch = "wasm32"))]
fn managed_store_oauth_endpoint_failure_is_permanent(status: u16, body: &str) -> bool {
    if matches!(status, 408 | 409 | 425 | 429 | 500..=599) {
        return false;
    }
    if status == 400 && managed_store_oauth_body_mentions_permanent_failure(body) {
        return true;
    }
    if managed_store_oauth_body_mentions_transient_failure(body) {
        return false;
    }
    matches!(status, 401 | 403)
}

#[cfg(not(target_arch = "wasm32"))]
fn managed_store_oauth_body_mentions_transient_failure(body: &str) -> bool {
    managed_store_oauth_body_mentions_any(
        body,
        &[
            "temporarily_unavailable",
            "temporary_unavailable",
            "server_error",
            "rate_limit",
            "rate_limited",
            "too_many_requests",
            "timeout",
            "timed out",
            "try again",
        ],
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn managed_store_oauth_body_mentions_permanent_failure(body: &str) -> bool {
    managed_store_oauth_body_mentions_any(
        body,
        &[
            "missing refresh_token",
            "invalid_grant",
            "invalid refresh",
            "refresh token revoked",
            "invalid_client",
            "unauthorized_client",
            "invalid_scope",
            "access_denied",
            "permission_denied",
        ],
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn managed_store_oauth_body_mentions_any(body: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| body.contains(needle))
}

#[cfg(not(target_arch = "wasm32"))]
fn publish_managed_store_tokens_refresh_lifecycle(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    tokens: &PersistedTokens,
) -> Result<meerkat_core::handles::AuthLeaseTransition, ProviderAuthError> {
    if AuthStatusPhase::from_lease_expires_at(
        (env.now)(),
        Some(meerkat_core::persisted_token_expires_at_epoch_secs(tokens)),
    ) == AuthStatusPhase::Expired
    {
        return Err(ProviderAuthError::Auth(AuthError::Expired));
    }
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;
    let lease_key = meerkat_core::handles::LeaseKey::from_connection_ref(&binding.connection_ref);
    let snapshot = auth_lease.snapshot(&lease_key);
    let began_here = if snapshot.phase == Some(meerkat_core::handles::AuthLeasePhase::Refreshing) {
        false
    } else {
        auth_lease.begin_refresh(&lease_key).map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle begin_refresh failed: {e}"
            ))
        })?;
        true
    };
    let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(tokens);
    let transition = auth_lease
        .complete_refresh(&lease_key, expires_at, epoch_secs((env.now)()))
        .map_err(|e| {
            if began_here {
                let _ = auth_lease.refresh_failed(&lease_key, false);
            }
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle complete_refresh failed: {e}"
            ))
        })?;
    require_credential_lifecycle_authority(env, binding)?;
    Ok(transition)
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn publish_managed_store_tokens_lifecycle_and_save(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    previous: &ManagedStoreTokens,
    refreshed: &PersistedTokens,
) -> Result<PersistedTokens, ProviderAuthError> {
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or(ProviderAuthError::Auth(AuthError::InteractiveLoginRequired))?;
    let lease_key = meerkat_core::handles::LeaseKey::from_connection_ref(&binding.connection_ref);
    let _guard = if previous.lifecycle_guard.is_none() {
        Some(meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await)
    } else {
        None
    };
    let previous_snapshot = previous.lifecycle_snapshot.as_ref().ok_or_else(|| {
        ProviderAuthError::SourceResolutionFailed(
            "managed_store OAuth refresh missing AuthMachine lifecycle snapshot".into(),
        )
    })?;
    let current_tokens = previous
        .store
        .load(&previous.key)
        .await
        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?;
    let current_snapshot = auth_lease.snapshot(&lease_key);
    if current_tokens.as_ref() != Some(&previous.tokens) {
        if let Some(current) = current_tokens.as_ref() {
            let relation = oauth_lifecycle_marker_relation(current, &current_snapshot);
            if persisted_token_material_matches(current, refreshed)
                && matches!(
                    relation,
                    OAuthLifecycleMarkerRelation::Matches
                        | OAuthLifecycleMarkerRelation::TokenNewer
                )
            {
                if relation == OAuthLifecycleMarkerRelation::TokenNewer {
                    let previous_adoption_snapshot = current_snapshot.clone();
                    let transition = if current_snapshot.phase
                        == Some(meerkat_core::handles::AuthLeasePhase::Refreshing)
                    {
                        publish_managed_store_tokens_refresh_lifecycle(env, binding, current)?
                    } else {
                        publish_managed_store_tokens_lifecycle(env, binding, current)?
                    };
                    let committed = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                        current, transition,
                    );
                    if committed != *current {
                        if let Err(e) = previous.store.save(&previous.key, &committed).await {
                            let rollback = restore_oauth_lifecycle_after_marker_save_failure(
                                auth_lease.as_ref(),
                                &lease_key,
                                &previous_adoption_snapshot,
                                current,
                            );
                            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                                "TokenStore lifecycle marker save failed after shared OAuth refresh adoption: {e}{rollback}"
                            )));
                        }
                        return Ok(committed);
                    }
                }
                return Ok(current.clone());
            }
        }
        return Err(ProviderAuthError::SourceResolutionFailed(
            "managed_store tokens changed during OAuth refresh; discarding stale refresh result"
                .into(),
        ));
    }
    if &current_snapshot != previous_snapshot {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "AuthMachine lifecycle changed during OAuth refresh; discarding stale refresh result"
                .into(),
        ));
    }

    let transition = publish_managed_store_tokens_refresh_lifecycle(env, binding, refreshed)?;
    let committed =
        meerkat_core::mark_tokens_lifecycle_published_for_transition(refreshed, transition);
    if let Err(save_error) = previous.store.save(&previous.key, &committed).await {
        let mut rollback_errors = Vec::new();
        if let Err(err) = auth_lease.release_credential_lifecycle(&lease_key) {
            rollback_errors.push(format!(
                "AuthMachine lifecycle rollback release failed: {err}"
            ));
        }
        let mut restored_previous = previous.tokens.clone();
        if let Err(err) = meerkat_core::restore_token_lifecycle_snapshot(
            auth_lease.as_ref(),
            &lease_key,
            previous_snapshot,
            Some(&previous.tokens),
        ) {
            rollback_errors.push(format!("AuthMachine lifecycle rollback failed: {err}"));
        } else if previous_snapshot.credential_present {
            let restored_snapshot = auth_lease.snapshot(&lease_key);
            if restored_snapshot.credential_present {
                restored_previous = meerkat_core::mark_tokens_lifecycle_published_for_snapshot(
                    &previous.tokens,
                    &restored_snapshot,
                );
            }
        }
        if let Err(err) = previous.store.save(&previous.key, &restored_previous).await {
            rollback_errors.push(format!("TokenStore rollback save failed: {err}"));
        }
        let rollback_suffix = if rollback_errors.is_empty() {
            String::new()
        } else {
            format!("; {}", rollback_errors.join("; "))
        };
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "TokenStore save failed after AuthMachine lifecycle acquire: {save_error}{rollback_suffix}"
        )));
    }
    Ok(committed)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn require_persisted_auth_mode(
    tokens: &PersistedTokens,
    auth_method: &str,
) -> Result<PersistedAuthMode, ProviderAuthError> {
    let expected = persisted_auth_mode_for_method(auth_method)?;
    if tokens.auth_mode != expected {
        return Err(persisted_auth_mode_mismatch(tokens, auth_method, expected));
    }
    Ok(expected)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn require_credential_lifecycle_authority(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
) -> Result<(), ProviderAuthError> {
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(|| interactive_login_error(binding))?;
    let lease_key = meerkat_core::handles::LeaseKey::from_connection_ref(&binding.connection_ref);
    let snapshot = auth_lease.snapshot(&lease_key);
    match AuthStatusPhase::from_lease_snapshot((env.now)(), &snapshot) {
        AuthStatusPhase::Valid | AuthStatusPhase::Expiring => Ok(()),
        AuthStatusPhase::Expired => Err(ProviderAuthError::Auth(AuthError::Expired)),
        AuthStatusPhase::ReauthRequired
        | AuthStatusPhase::RefreshFailed
        | AuthStatusPhase::Unknown => Err(interactive_login_error(binding)),
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
    AuthMachine(AuthLeaseSnapshot),
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedOauthAccess {
    Cached {
        tokens: PersistedTokens,
        expires_at: Option<DateTime<Utc>>,
        lease_snapshot: AuthLeaseSnapshot,
    },
    Refresh {
        lifecycle: ManagedOauthRefreshLifecycle,
        tokens: PersistedTokens,
    },
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedOauthRefreshCompletion {
    pub expires_at: Option<DateTime<Utc>>,
    pub lease_snapshot: AuthLeaseSnapshot,
}

#[cfg(not(target_arch = "wasm32"))]
pub type StoredTokenRefreshFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<StoredTokenRefreshOutcome, AuthError>> + Send>,
>;

#[cfg(not(target_arch = "wasm32"))]
pub type StoredTokenRefreshFn =
    Arc<dyn Fn(AuthRefreshReason) -> StoredTokenRefreshFuture + Send + Sync>;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTokenRefreshOutcome {
    pub secret: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub lease_snapshot: Option<AuthLeaseSnapshot>,
}

#[cfg(not(target_arch = "wasm32"))]
pub struct RefreshableStoredTokenLease {
    secret: Mutex<Arc<String>>,
    metadata: AuthMetadata,
    expires_at: Mutex<Option<DateTime<Utc>>>,
    auth_lease_snapshot: Mutex<Option<AuthLeaseSnapshot>>,
    source_label: String,
    refresh: StoredTokenRefreshFn,
}

#[cfg(not(target_arch = "wasm32"))]
impl RefreshableStoredTokenLease {
    pub fn inline_secret(
        secret: String,
        metadata: AuthMetadata,
        expires_at: Option<DateTime<Utc>>,
        auth_lease_snapshot: Option<AuthLeaseSnapshot>,
        source_label: impl Into<String>,
        refresh: StoredTokenRefreshFn,
    ) -> Self {
        Self {
            secret: Mutex::new(Arc::new(secret)),
            metadata,
            expires_at: Mutex::new(expires_at),
            auth_lease_snapshot: Mutex::new(auth_lease_snapshot),
            source_label: source_label.into(),
            refresh,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl AuthLease for RefreshableStoredTokenLease {
    fn kind(&self) -> ResolvedAuthKind {
        ResolvedAuthKind::InlineSecret(
            self.secret
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }

    fn metadata(&self) -> &AuthMetadata {
        &self.metadata
    }

    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .to_owned()
    }

    fn source_label(&self) -> &str {
        &self.source_label
    }

    fn auth_lease_snapshot(&self) -> Option<AuthLeaseSnapshot> {
        self.auth_lease_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn refresh(&self, reason: AuthRefreshReason) -> Result<(), AuthError> {
        let refreshed = (self.refresh)(reason).await?;
        *self
            .secret
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(refreshed.secret);
        *self
            .expires_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = refreshed.expires_at;
        *self
            .auth_lease_snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = refreshed.lease_snapshot;
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
const MANAGED_OAUTH_REFRESH_WAIT_POLL_MS: u64 = 10;
#[cfg(not(target_arch = "wasm32"))]
const MANAGED_OAUTH_REFRESH_WAIT_TIMEOUT_SECS: u64 = 30;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseBoundStoredTokens {
    pub tokens: PersistedTokens,
    pub expires_at: Option<DateTime<Utc>>,
    pub lease_snapshot: AuthLeaseSnapshot,
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn resolve_lease_bound_stored_tokens(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    expected_mode: PersistedAuthMode,
) -> Result<LeaseBoundStoredTokens, ProviderAuthError> {
    let Some(handle) = env.auth_lease_handle.as_ref() else {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "stored credentials require an AuthMachine lease handle for freshness".into(),
        ));
    };

    let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
    let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
    let refresh_wait_deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(MANAGED_OAUTH_REFRESH_WAIT_TIMEOUT_SECS);

    loop {
        let snapshot = handle.snapshot(&lease_key);
        if snapshot.phase.is_none() {
            if bootstrap_lease_bound_stored_tokens(
                env,
                handle.as_ref(),
                &token_key,
                &lease_key,
                &expected_mode,
            )
            .await?
            {
                continue;
            }
            return Err(ProviderAuthError::Auth(AuthError::Expired));
        }

        match snapshot.phase {
            Some(AuthLeasePhase::Valid)
                if snapshot_expires_at_is_fresh(env, snapshot.expires_at) =>
            {
                let Some(observed) = load_stored_auth_tokens(env, &token_key).await? else {
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
                if observed.auth_mode != expected_mode
                    || !managed_oauth_tokens_match_lease_snapshot(&observed, &snapshot, &token_key)
                    || observed.primary_secret.is_none()
                {
                    if mark_managed_oauth_reauth_required_for_snapshot(
                        handle.as_ref(),
                        &lease_key,
                        &snapshot,
                    )? {
                        return Err(ProviderAuthError::Auth(AuthError::Expired));
                    }
                    continue;
                }
                return Ok(LeaseBoundStoredTokens {
                    tokens: observed,
                    expires_at: lease_snapshot_expires_at_datetime(&snapshot),
                    lease_snapshot: snapshot,
                });
            }
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                if mark_managed_oauth_reauth_required_for_snapshot(
                    handle.as_ref(),
                    &lease_key,
                    &snapshot,
                )? {
                    return Err(ProviderAuthError::Auth(AuthError::Expired));
                }
            }
            Some(AuthLeasePhase::Refreshing) => {
                wait_for_managed_oauth_owner_refresh(
                    env,
                    handle.as_ref(),
                    &lease_key,
                    &token_key,
                    &expected_mode,
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
pub async fn begin_managed_oauth_refresh(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    expected_mode: PersistedAuthMode,
) -> Result<(ManagedOauthRefreshLifecycle, PersistedTokens), ProviderAuthError> {
    let Some(handle) = env.auth_lease_handle.as_ref() else {
        let _ = binding;
        return Err(ProviderAuthError::SourceResolutionFailed(
            "managed OAuth refresh requires an AuthMachine lease handle".into(),
        ));
    };

    let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
    let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
    let refresh_wait_deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(MANAGED_OAUTH_REFRESH_WAIT_TIMEOUT_SECS);

    loop {
        let snapshot = handle.snapshot(&lease_key);
        if snapshot.phase.is_none() {
            if bootstrap_lease_bound_stored_tokens(
                env,
                handle.as_ref(),
                &token_key,
                &lease_key,
                &expected_mode,
            )
            .await?
            {
                continue;
            }
            return Err(ProviderAuthError::Auth(AuthError::Expired));
        }

        match snapshot.phase {
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                let Some(observed) = load_stored_auth_tokens(env, &token_key).await? else {
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
                if observed.auth_mode != expected_mode
                    || !managed_oauth_tokens_match_lease_snapshot(&observed, &snapshot, &token_key)
                {
                    if mark_managed_oauth_reauth_required_for_snapshot(
                        handle.as_ref(),
                        &lease_key,
                        &snapshot,
                    )? {
                        return Err(ProviderAuthError::Auth(AuthError::Expired));
                    }
                    continue;
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
                match claim_managed_oauth_refresh(handle.as_ref(), &lease_key, &snapshot) {
                    Ok(Some(lifecycle)) => return Ok((lifecycle, observed)),
                    Ok(None) => continue,
                    Err(err) => match handle.snapshot(&lease_key).phase {
                        Some(AuthLeasePhase::Refreshing) => {
                            wait_for_managed_oauth_owner_refresh(
                                env,
                                handle.as_ref(),
                                &lease_key,
                                &token_key,
                                &expected_mode,
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
                wait_for_managed_oauth_owner_refresh(
                    env,
                    handle.as_ref(),
                    &lease_key,
                    &token_key,
                    &expected_mode,
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
pub async fn resolve_managed_oauth_access(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    expected_mode: PersistedAuthMode,
) -> Result<ManagedOauthAccess, ProviderAuthError> {
    let Some(handle) = env.auth_lease_handle.as_ref() else {
        let _ = binding;
        return Err(ProviderAuthError::SourceResolutionFailed(
            "managed OAuth requires an AuthMachine lease handle for token freshness".into(),
        ));
    };

    let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
    let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
    let refresh_wait_deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(MANAGED_OAUTH_REFRESH_WAIT_TIMEOUT_SECS);

    loop {
        let snapshot = handle.snapshot(&lease_key);
        if snapshot.phase.is_none() {
            if bootstrap_lease_bound_stored_tokens(
                env,
                handle.as_ref(),
                &token_key,
                &lease_key,
                &expected_mode,
            )
            .await?
            {
                continue;
            }
            return Err(ProviderAuthError::Auth(AuthError::Expired));
        }

        match snapshot.phase {
            Some(AuthLeasePhase::Valid)
                if snapshot_expires_at_is_fresh(env, snapshot.expires_at) =>
            {
                let Some(observed) = load_stored_auth_tokens(env, &token_key).await? else {
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
                let token_matches_lease = observed.auth_mode == expected_mode
                    && managed_oauth_tokens_match_lease_snapshot(&observed, &snapshot, &token_key);
                if !token_matches_lease {
                    if mark_managed_oauth_reauth_required_for_snapshot(
                        handle.as_ref(),
                        &lease_key,
                        &snapshot,
                    )? {
                        return Err(ProviderAuthError::Auth(AuthError::Expired));
                    }
                    continue;
                }
                if observed.primary_secret.is_some() {
                    return Ok(ManagedOauthAccess::Cached {
                        tokens: observed,
                        expires_at: lease_snapshot_expires_at_datetime(&snapshot),
                        lease_snapshot: snapshot,
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
                match claim_managed_oauth_refresh(handle.as_ref(), &lease_key, &snapshot) {
                    Ok(Some(lifecycle)) => {
                        return Ok(ManagedOauthAccess::Refresh {
                            lifecycle,
                            tokens: observed,
                        });
                    }
                    Ok(None) => continue,
                    Err(err) => match handle.snapshot(&lease_key).phase {
                        Some(AuthLeasePhase::Refreshing) => {
                            wait_for_managed_oauth_owner_refresh(
                                env,
                                handle.as_ref(),
                                &lease_key,
                                &token_key,
                                &expected_mode,
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
                let Some(observed) = load_stored_auth_tokens(env, &token_key).await? else {
                    if mark_managed_oauth_reauth_required_for_snapshot(
                        handle.as_ref(),
                        &lease_key,
                        &snapshot,
                    )? {
                        return Err(ProviderAuthError::Auth(AuthError::Expired));
                    }
                    continue;
                };
                if observed.auth_mode != expected_mode
                    || !managed_oauth_tokens_match_lease_snapshot(&observed, &snapshot, &token_key)
                {
                    if mark_managed_oauth_reauth_required_for_snapshot(
                        handle.as_ref(),
                        &lease_key,
                        &snapshot,
                    )? {
                        return Err(ProviderAuthError::Auth(AuthError::Expired));
                    }
                    continue;
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
                match claim_managed_oauth_refresh(handle.as_ref(), &lease_key, &snapshot) {
                    Ok(Some(lifecycle)) => {
                        return Ok(ManagedOauthAccess::Refresh {
                            lifecycle,
                            tokens: observed,
                        });
                    }
                    Ok(None) => continue,
                    Err(err) => match handle.snapshot(&lease_key).phase {
                        Some(AuthLeasePhase::Refreshing) => {
                            wait_for_managed_oauth_owner_refresh(
                                env,
                                handle.as_ref(),
                                &lease_key,
                                &token_key,
                                &expected_mode,
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
                    env,
                    handle.as_ref(),
                    &lease_key,
                    &token_key,
                    &expected_mode,
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
fn claim_managed_oauth_refresh(
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
    let ManagedOauthRefreshLifecycle::AuthMachine(refreshing_snapshot) = lifecycle;
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
    env: &ResolverEnvironment,
    handle: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &LeaseKey,
    token_key: &TokenKey,
    expected_mode: &PersistedAuthMode,
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

        let snapshot = handle.snapshot(lease_key);
        match snapshot.phase {
            Some(AuthLeasePhase::Refreshing) => continue,
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring) => {
                if let Some(observed) = load_stored_auth_tokens(env, token_key).await?
                    && observed.auth_mode == *expected_mode
                    && managed_oauth_tokens_match_lease_snapshot(&observed, &snapshot, token_key)
                {
                    return Ok(());
                }
                continue;
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
async fn load_stored_auth_tokens(
    env: &ResolverEnvironment,
    token_key: &TokenKey,
) -> Result<Option<PersistedTokens>, ProviderAuthError> {
    let store = env.token_store.as_ref().ok_or_else(|| {
        ProviderAuthError::SourceResolutionFailed(
            "stored credentials require a TokenStore for AuthMachine lease verification".into(),
        )
    })?;
    store
        .load(token_key)
        .await
        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
async fn bootstrap_lease_bound_stored_tokens(
    env: &ResolverEnvironment,
    handle: &dyn meerkat_core::handles::AuthLeaseHandle,
    token_key: &TokenKey,
    lease_key: &LeaseKey,
    expected_mode: &PersistedAuthMode,
) -> Result<bool, ProviderAuthError> {
    let Some(store) = env.token_store.as_ref() else {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "stored credentials require a TokenStore for AuthMachine lease bootstrap".into(),
        ));
    };
    let Some(stored) = load_stored_auth_tokens(env, token_key).await? else {
        return Ok(false);
    };
    if stored.auth_mode != *expected_mode {
        return Ok(false);
    }
    let Some(binding) = stored.auth_lease.as_ref() else {
        return Ok(false);
    };
    if binding.token_key != *token_key || binding.pending_owner_generation.is_some() {
        return Ok(false);
    }

    let expected = handle.snapshot(lease_key);
    if expected.phase.is_some() {
        return Ok(false);
    }
    let expires_at = managed_oauth_token_expires_at_epoch_secs(&stored);
    let Some(transition) = handle
        .acquire_lease_if_snapshot(lease_key, &expected, expires_at)
        .map_err(|err| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle restart acquire failed: {err}"
            ))
        })?
    else {
        return Ok(false);
    };

    let snapshot = AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::Valid),
        expires_at: lease_expires_at_snapshot_arg(expires_at),
        generation: transition.generation,
    };
    let rebound = stored
        .clone()
        .with_auth_lease_binding(token_key.clone(), transition.generation);
    match store.save_if_current(token_key, &stored, &rebound).await {
        Ok(true) => {}
        Ok(false) => {
            if !stored_auth_tokens_match_or_mark_reauth(
                env,
                handle,
                lease_key,
                token_key,
                &snapshot,
                expected_mode,
            )
            .await?
            {
                return Err(ProviderAuthError::SourceResolutionFailed(
                    "TokenStore material changed before restart AuthMachine lease binding could be finalized"
                        .into(),
                ));
            }
        }
        Err(save_error) => {
            let reauth_result =
                mark_managed_oauth_reauth_required_for_snapshot(handle, lease_key, &snapshot);
            let stale_token_cleared =
                clear_stored_auth_tokens_if_current(store, token_key, &rebound).await?;
            let reauth_marked = match reauth_result {
                Ok(marked) => marked.to_string(),
                Err(mark_error) => format!("error: {mark_error}"),
            };
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "TokenStore save_if_current failed during restart AuthMachine lease binding; \
                 reauth_marked={reauth_marked}; stale_token_cleared={stale_token_cleared}: {save_error}"
            )));
        }
    }

    if handle.snapshot(lease_key) != snapshot {
        let cleared = clear_stored_auth_tokens_if_current(store, token_key, &rebound).await?;
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "AuthMachine lifecycle changed while restart lease binding was saved; stale_token_cleared={cleared}"
        )));
    }

    Ok(true)
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
    expected: &PersistedTokens,
    refreshed: &PersistedTokens,
) -> Result<ManagedOauthRefreshCompletion, ProviderAuthError> {
    let store = match env.token_store.as_ref() {
        Some(store) => store,
        None => {
            let ManagedOauthRefreshLifecycle::AuthMachine(refreshing_snapshot) = &lifecycle;
            let _ = mark_managed_oauth_refresh_failed_for_snapshot(
                env,
                binding,
                refreshing_snapshot,
                false,
            );
            return Err(ProviderAuthError::SourceResolutionFailed(
                "managed OAuth refresh completed but TokenStore is unavailable".into(),
            ));
        }
    };

    let Some((handle, lease_key, refreshing_snapshot)) =
        managed_oauth_refresh_owner_guard(env, binding, &lifecycle)?
    else {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "managed OAuth refresh requires AuthMachine lifecycle ownership".into(),
        ));
    };

    if handle.snapshot(&lease_key) != refreshing_snapshot {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "AuthMachine lifecycle changed before managed OAuth refreshed tokens could be saved"
                .into(),
        ));
    }

    let expected_mode = expected.auth_mode;
    if refreshed.auth_mode != expected_mode {
        let rollback_marked = mark_managed_oauth_refresh_failed_for_snapshot(
            env,
            binding,
            &refreshing_snapshot,
            true,
        )?;
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "managed OAuth refresh returned unexpected auth_mode; rollback_marked={rollback_marked}"
        )));
    }
    if refreshed.primary_secret.is_none() {
        let rollback_marked = mark_managed_oauth_refresh_failed_for_snapshot(
            env,
            binding,
            &refreshing_snapshot,
            true,
        )?;
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "managed OAuth refresh returned no primary_secret; rollback_marked={rollback_marked}"
        )));
    }

    let refreshed = refreshed.clone().canonicalize_for_persistence();
    let refreshed_expires_at = managed_oauth_token_expires_at_epoch_secs(&refreshed);
    let pending_refreshed = refreshed
        .clone()
        .with_auth_pending_owner_binding(token_key.clone(), refreshing_snapshot.generation);
    match store
        .save_if_current(token_key, expected, &pending_refreshed)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            let rollback_marked = mark_managed_oauth_refresh_failed_for_snapshot(
                env,
                binding,
                &refreshing_snapshot,
                true,
            )?;
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "TokenStore material changed before managed OAuth refreshed tokens could be prepared; \
                 rollback_marked={rollback_marked}"
            )));
        }
        Err(save_error) => {
            let rollback_marked = mark_managed_oauth_refresh_failed_for_snapshot(
                env,
                binding,
                &refreshing_snapshot,
                true,
            )?;
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "TokenStore save_if_current failed before AuthMachine refresh completion; \
                 rollback_marked={rollback_marked}: {save_error}"
            )));
        }
    }

    let transition = match handle.complete_refresh_if_snapshot(
        &lease_key,
        &refreshing_snapshot,
        refreshed_expires_at,
        epoch_secs((env.now)()),
    ) {
        Ok(Some(transition)) => transition,
        Ok(None) => {
            let restored = restore_managed_oauth_tokens_if_current(
                store,
                token_key,
                &pending_refreshed,
                expected,
            )
            .await?;
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle changed before managed OAuth refresh completion; \
                     prepared_token_restored={restored}"
            )));
        }
        Err(err) => {
            let restored = restore_managed_oauth_tokens_if_current(
                store,
                token_key,
                &pending_refreshed,
                expected,
            )
            .await?;
            let rollback_marked = mark_managed_oauth_refresh_failed_for_snapshot(
                env,
                binding,
                &refreshing_snapshot,
                false,
            )?;
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle complete_refresh failed for managed OAuth refreshed tokens; \
                 prepared_token_restored={restored}; rollback_marked={rollback_marked}: {err}"
            )));
        }
    };

    let snapshot = AuthLeaseSnapshot {
        phase: Some(AuthLeasePhase::Valid),
        expires_at: lease_expires_at_snapshot_arg(refreshed_expires_at),
        generation: transition.generation,
    };
    let bound_refreshed = refreshed
        .clone()
        .with_auth_lease_binding(token_key.clone(), transition.generation);
    match store
        .save_if_current(token_key, &pending_refreshed, &bound_refreshed)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            if !stored_auth_tokens_match_or_mark_reauth(
                env,
                handle.as_ref(),
                &lease_key,
                token_key,
                &snapshot,
                &expected_mode,
            )
            .await?
            {
                return Err(ProviderAuthError::SourceResolutionFailed(
                    "TokenStore material changed before managed OAuth refresh could finalize its AuthMachine lease binding"
                        .into(),
                ));
            }
        }
        Err(save_error) => {
            let _ = mark_managed_oauth_reauth_required_for_snapshot(
                handle.as_ref(),
                &lease_key,
                &snapshot,
            );
            let stale_token_cleared =
                clear_stored_auth_tokens_if_current(store, token_key, &bound_refreshed).await?;
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "TokenStore save_if_current failed after AuthMachine refresh completion; \
                 stale_token_cleared={stale_token_cleared}: {save_error}"
            )));
        }
    }

    if handle.snapshot(&lease_key) != snapshot {
        let cleared_bound =
            clear_stored_auth_tokens_if_current(store, token_key, &bound_refreshed).await?;
        let cleared_pending = if cleared_bound {
            false
        } else {
            clear_stored_auth_tokens_if_current(store, token_key, &pending_refreshed).await?
        };
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "AuthMachine lifecycle changed while managed OAuth refreshed tokens were saved; \
             stale_token_cleared={}",
            cleared_bound || cleared_pending
        )));
    }

    Ok(ManagedOauthRefreshCompletion {
        expires_at: lease_snapshot_expires_at_datetime(&snapshot).or(refreshed.expires_at),
        lease_snapshot: snapshot,
    })
}

#[cfg(not(target_arch = "wasm32"))]
async fn restore_managed_oauth_tokens_if_current(
    store: &Arc<dyn meerkat_core::auth::TokenStore>,
    token_key: &TokenKey,
    pending: &PersistedTokens,
    expected: &PersistedTokens,
) -> Result<bool, ProviderAuthError> {
    store
        .save_if_current(token_key, pending, expected)
        .await
        .map_err(|restore_error| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "managed OAuth TokenStore restore failed: {restore_error}"
            ))
        })
}

#[cfg(not(target_arch = "wasm32"))]
async fn stored_auth_tokens_match_lease_snapshot(
    env: &ResolverEnvironment,
    token_key: &TokenKey,
    snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
    expected_mode: &PersistedAuthMode,
) -> Result<bool, ProviderAuthError> {
    Ok(load_stored_auth_tokens(env, token_key)
        .await?
        .as_ref()
        .is_some_and(|observed| {
            observed.auth_mode == *expected_mode
                && managed_oauth_tokens_match_lease_snapshot(observed, snapshot, token_key)
        }))
}

#[cfg(not(target_arch = "wasm32"))]
async fn stored_auth_tokens_match_or_mark_reauth(
    env: &ResolverEnvironment,
    handle: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &LeaseKey,
    token_key: &TokenKey,
    snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
    expected_mode: &PersistedAuthMode,
) -> Result<bool, ProviderAuthError> {
    match stored_auth_tokens_match_lease_snapshot(env, token_key, snapshot, expected_mode).await {
        Ok(true) => Ok(true),
        Ok(false) => {
            let _ = mark_managed_oauth_reauth_required_for_snapshot(handle, lease_key, snapshot);
            Ok(false)
        }
        Err(err) => {
            let _ = mark_managed_oauth_reauth_required_for_snapshot(handle, lease_key, snapshot);
            Err(err)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn clear_stored_auth_tokens_if_current(
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
    let ManagedOauthRefreshLifecycle::AuthMachine(refreshing_snapshot) = lifecycle;
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
    token_key: &TokenKey,
) -> bool {
    let snapshot_expires_at = snapshot.expires_at.unwrap_or(u64::MAX);
    managed_oauth_token_expires_at_epoch_secs(tokens) == snapshot_expires_at
        && tokens.auth_lease.as_ref().is_some_and(|binding| {
            binding.token_key == *token_key
                && binding.pending_owner_generation.is_none()
                && binding.generation == snapshot.generation
        })
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
    use meerkat_core::auth::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat_core::handles::{AuthLeaseHandle, AuthLeaseTransition, DslTransitionError};
    use meerkat_core::{AuthProfile, AuthRouteHints, BindingPolicy, ConnectionRef, Provider};
    use meerkat_llm_core::provider_runtime::binding::{
        NormalizedAuthMethod, NormalizedBackendKind, ValidatedBinding,
    };
    #[cfg(not(target_arch = "wasm32"))]
    use std::sync::Mutex;

    #[cfg(not(target_arch = "wasm32"))]
    struct TestAuthLeaseHandle {
        snapshot: Mutex<AuthLeaseSnapshot>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct RefreshCompletingAuthLeaseHandle {
        snapshot: Mutex<AuthLeaseSnapshot>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct RestartAcquiringAuthLeaseHandle {
        snapshot: Mutex<AuthLeaseSnapshot>,
        fail_reauth: bool,
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct ManagedOauthFinalizeRaceTokenStore {
        stored: Mutex<Option<PersistedTokens>>,
        fail_load_after_final_save_attempt: bool,
        fail_final_save_after_writing: bool,
        fail_load: Mutex<bool>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl TestAuthLeaseHandle {
        fn valid_non_expiring(generation: u64) -> Self {
            Self {
                snapshot: Mutex::new(AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Valid),
                    expires_at: None,
                    generation,
                }),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl RefreshCompletingAuthLeaseHandle {
        fn refreshing(generation: u64, expires_at: Option<u64>) -> Self {
            Self {
                snapshot: Mutex::new(AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Refreshing),
                    expires_at,
                    generation,
                }),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl RestartAcquiringAuthLeaseHandle {
        fn empty(generation: u64) -> Self {
            Self {
                snapshot: Mutex::new(AuthLeaseSnapshot {
                    phase: None,
                    expires_at: None,
                    generation,
                }),
                fail_reauth: false,
            }
        }

        fn empty_with_reauth_error(generation: u64) -> Self {
            Self {
                snapshot: Mutex::new(AuthLeaseSnapshot {
                    phase: None,
                    expires_at: None,
                    generation,
                }),
                fail_reauth: true,
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl ManagedOauthFinalizeRaceTokenStore {
        fn new(stored: PersistedTokens) -> Self {
            Self {
                stored: Mutex::new(Some(stored)),
                fail_load_after_final_save_attempt: false,
                fail_final_save_after_writing: false,
                fail_load: Mutex::new(false),
            }
        }

        fn new_with_finalize_load_error(stored: PersistedTokens) -> Self {
            Self {
                stored: Mutex::new(Some(stored)),
                fail_load_after_final_save_attempt: true,
                fail_final_save_after_writing: false,
                fail_load: Mutex::new(false),
            }
        }

        fn new_with_final_save_error_after_write(stored: PersistedTokens) -> Self {
            Self {
                stored: Mutex::new(Some(stored)),
                fail_load_after_final_save_attempt: false,
                fail_final_save_after_writing: true,
                fail_load: Mutex::new(false),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl AuthLeaseHandle for TestAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            panic!("test handle should not acquire over pre-published lease truth")
        }

        fn acquire_lease_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _expires_at: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            panic!("test handle should not conditionally acquire over pre-published lease truth")
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            panic!("test handle should not mark expiring")
        }

        fn begin_refresh(
            &self,
            _lease_key: &LeaseKey,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            panic!("test handle should not begin refresh")
        }

        fn begin_refresh_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            panic!("test handle should not conditionally begin refresh")
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            panic!("test handle should not complete refresh")
        }

        fn complete_refresh_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            panic!("test handle should not conditionally complete refresh")
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            panic!("test handle should not fail refresh")
        }

        fn refresh_failed_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _permanent: bool,
        ) -> Result<bool, DslTransitionError> {
            panic!("test handle should not conditionally fail refresh")
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.phase = Some(AuthLeasePhase::ReauthRequired);
            snapshot.generation += 1;
            Ok(())
        }

        fn mark_reauth_required_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<bool, DslTransitionError> {
            if self.snapshot(lease_key) != *expected {
                return Ok(false);
            }
            self.mark_reauth_required(lease_key)?;
            Ok(true)
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot.lock().unwrap().clone()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl AuthLeaseHandle for RefreshCompletingAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            panic!("test handle should not acquire")
        }

        fn acquire_lease_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _expires_at: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            panic!("test handle should not conditionally acquire")
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            panic!("test handle should not mark expiring")
        }

        fn begin_refresh(
            &self,
            _lease_key: &LeaseKey,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            panic!("test handle should not begin refresh")
        }

        fn begin_refresh_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            panic!("test handle should not conditionally begin refresh")
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            let mut snapshot = self.snapshot.lock().unwrap();
            if snapshot.phase != Some(AuthLeasePhase::Refreshing) {
                return Err(DslTransitionError::new(
                    "complete_refresh",
                    "lease is not refreshing",
                ));
            }
            snapshot.phase = Some(AuthLeasePhase::Valid);
            snapshot.expires_at = lease_expires_at_snapshot_arg(new_expires_at);
            snapshot.generation += 1;
            Ok(AuthLeaseTransition {
                generation: snapshot.generation,
            })
        }

        fn complete_refresh_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
            new_expires_at: u64,
            now: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            if self.snapshot(lease_key) != *expected {
                return Ok(None);
            }
            self.complete_refresh(lease_key, new_expires_at, now)
                .map(Some)
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            panic!("test handle should not fail refresh")
        }

        fn refresh_failed_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _permanent: bool,
        ) -> Result<bool, DslTransitionError> {
            panic!("test handle should not conditionally fail refresh")
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.phase = Some(AuthLeasePhase::ReauthRequired);
            snapshot.generation += 1;
            Ok(())
        }

        fn mark_reauth_required_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<bool, DslTransitionError> {
            if self.snapshot(lease_key) != *expected {
                return Ok(false);
            }
            self.mark_reauth_required(lease_key)?;
            Ok(true)
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            panic!("test handle should not release")
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot.lock().unwrap().clone()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl AuthLeaseHandle for RestartAcquiringAuthLeaseHandle {
        fn acquire_lease(
            &self,
            lease_key: &LeaseKey,
            expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            self.acquire_lease_if_snapshot(lease_key, &self.snapshot(lease_key), expires_at)?
                .ok_or_else(|| DslTransitionError::new("acquire_lease", "snapshot changed"))
        }

        fn acquire_lease_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
            expires_at: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            let mut snapshot = self.snapshot.lock().unwrap();
            if *snapshot != *expected {
                return Ok(None);
            }
            snapshot.phase = Some(AuthLeasePhase::Valid);
            snapshot.expires_at = lease_expires_at_snapshot_arg(expires_at);
            snapshot.generation += 1;
            let _ = lease_key;
            Ok(Some(AuthLeaseTransition {
                generation: snapshot.generation,
            }))
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            panic!("test handle should not mark expiring")
        }

        fn begin_refresh(
            &self,
            _lease_key: &LeaseKey,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            panic!("test handle should not begin refresh")
        }

        fn begin_refresh_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            panic!("test handle should not conditionally begin refresh")
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            panic!("test handle should not complete refresh")
        }

        fn complete_refresh_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
            panic!("test handle should not conditionally complete refresh")
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            panic!("test handle should not fail refresh")
        }

        fn refresh_failed_if_snapshot(
            &self,
            _lease_key: &LeaseKey,
            _expected: &AuthLeaseSnapshot,
            _permanent: bool,
        ) -> Result<bool, DslTransitionError> {
            panic!("test handle should not conditionally fail refresh")
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            if self.fail_reauth {
                return Err(DslTransitionError::new(
                    "mark_reauth_required",
                    "injected reauth failure",
                ));
            }
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.phase = Some(AuthLeasePhase::ReauthRequired);
            snapshot.generation += 1;
            Ok(())
        }

        fn mark_reauth_required_if_snapshot(
            &self,
            lease_key: &LeaseKey,
            expected: &AuthLeaseSnapshot,
        ) -> Result<bool, DslTransitionError> {
            if self.snapshot(lease_key) != *expected {
                return Ok(false);
            }
            self.mark_reauth_required(lease_key)?;
            Ok(true)
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            panic!("test handle should not release")
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot.lock().unwrap().clone()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait::async_trait]
    impl TokenStore for ManagedOauthFinalizeRaceTokenStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            if *self.fail_load.lock().unwrap() {
                return Err(TokenStoreError::Serde("corrupt token".into()));
            }
            Ok(self.stored.lock().unwrap().clone())
        }

        async fn save(
            &self,
            _key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            *self.stored.lock().unwrap() = Some(tokens.clone());
            Ok(())
        }

        async fn save_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut stored = self.stored.lock().unwrap();
            if stored.as_ref() != Some(expected) {
                return Ok(false);
            }
            if replacement
                .auth_lease
                .as_ref()
                .and_then(|binding| binding.pending_owner_generation)
                .is_none()
            {
                if self.fail_final_save_after_writing {
                    *stored = Some(replacement.clone());
                    return Err(TokenStoreError::Unavailable(
                        "finalize write reported failure".into(),
                    ));
                }
                if self.fail_load_after_final_save_attempt {
                    *self.fail_load.lock().unwrap() = true;
                }
                return Ok(false);
            }
            *stored = Some(replacement.clone());
            Ok(true)
        }

        async fn save_if_current_optional(
            &self,
            _key: &TokenKey,
            expected: Option<&PersistedTokens>,
            replacement: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut stored = self.stored.lock().unwrap();
            if stored.as_ref() != expected {
                return Ok(false);
            }
            *stored = Some(replacement.clone());
            Ok(true)
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            *self.stored.lock().unwrap() = None;
            Ok(())
        }

        async fn clear_if_current(
            &self,
            _key: &TokenKey,
            expected: &PersistedTokens,
        ) -> Result<bool, TokenStoreError> {
            let mut stored = self.stored.lock().unwrap();
            if stored.as_ref() != Some(expected) {
                return Ok(false);
            }
            *stored = None;
            Ok(true)
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "managed-oauth-finalize-race"
        }
    }

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

    #[cfg(not(target_arch = "wasm32"))]
    struct StaticAuthLeaseHandle {
        snapshot: AuthLeaseSnapshot,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl StaticAuthLeaseHandle {
        fn valid() -> Arc<Self> {
            Self::valid_generation(1)
        }

        fn valid_generation(generation: u64) -> Arc<Self> {
            Arc::new(Self {
                snapshot: AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Valid),
                    expires_at: None,
                    credential_present: true,
                    generation,
                    credential_published_at_millis: None,
                },
            })
        }

        fn valid_generation_with_expiry(generation: u64, expires_at: u64) -> Arc<Self> {
            Self::valid_generation_with_expiry_and_publication_time(generation, expires_at, None)
        }

        fn valid_generation_with_expiry_and_publication_time(
            generation: u64,
            expires_at: u64,
            credential_published_at_millis: Option<u64>,
        ) -> Arc<Self> {
            Arc::new(Self {
                snapshot: AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Valid),
                    expires_at: Some(expires_at),
                    credential_present: true,
                    generation,
                    credential_published_at_millis,
                },
            })
        }

        fn unknown() -> Arc<Self> {
            Arc::new(Self {
                snapshot: AuthLeaseSnapshot {
                    phase: None,
                    expires_at: None,
                    credential_present: false,
                    generation: 0,
                    credential_published_at_millis: None,
                },
            })
        }

        fn released() -> Arc<Self> {
            Arc::new(Self {
                snapshot: AuthLeaseSnapshot {
                    phase: None,
                    expires_at: None,
                    credential_present: false,
                    generation: 1,
                    credential_published_at_millis: None,
                },
            })
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct MutableAuthLeaseHandle {
        snapshot: std::sync::Mutex<AuthLeaseSnapshot>,
        acquire_count: std::sync::atomic::AtomicUsize,
        begin_refresh_count: std::sync::atomic::AtomicUsize,
        complete_refresh_count: std::sync::atomic::AtomicUsize,
        refresh_failed_count: std::sync::atomic::AtomicUsize,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl MutableAuthLeaseHandle {
        fn unknown() -> Arc<Self> {
            Self::from_snapshot(AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                credential_present: false,
                generation: 0,
                credential_published_at_millis: None,
            })
        }

        fn from_snapshot(snapshot: AuthLeaseSnapshot) -> Arc<Self> {
            Arc::new(Self {
                snapshot: std::sync::Mutex::new(snapshot),
                acquire_count: std::sync::atomic::AtomicUsize::new(0),
                begin_refresh_count: std::sync::atomic::AtomicUsize::new(0),
                complete_refresh_count: std::sync::atomic::AtomicUsize::new(0),
                refresh_failed_count: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn acquire_count(&self) -> usize {
            self.acquire_count.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn begin_refresh_count(&self) -> usize {
            self.begin_refresh_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }

        fn complete_refresh_count(&self) -> usize {
            self.complete_refresh_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }

        fn refresh_failed_count(&self) -> usize {
            self.refresh_failed_count
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl AuthLeaseHandle for MutableAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            self.acquire_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut snapshot = self.snapshot.lock().expect("snapshot lock");
            snapshot.phase = Some(AuthLeasePhase::Valid);
            snapshot.expires_at = if expires_at == u64::MAX {
                None
            } else {
                Some(expires_at)
            };
            snapshot.credential_present = true;
            snapshot.generation += 1;
            snapshot.credential_published_at_millis = Some(10_000);
            Ok(AuthLeaseTransition {
                generation: snapshot.generation,
                credential_published_at_millis: snapshot.credential_published_at_millis,
            })
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            self.begin_refresh_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut snapshot = self.snapshot.lock().expect("snapshot lock");
            snapshot.phase = Some(AuthLeasePhase::Refreshing);
            Ok(())
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            self.complete_refresh_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut snapshot = self.snapshot.lock().expect("snapshot lock");
            snapshot.phase = Some(AuthLeasePhase::Valid);
            snapshot.expires_at = if new_expires_at == u64::MAX {
                None
            } else {
                Some(new_expires_at)
            };
            snapshot.credential_present = true;
            snapshot.generation += 1;
            snapshot.credential_published_at_millis = Some(10_000);
            Ok(AuthLeaseTransition {
                generation: snapshot.generation,
                credential_published_at_millis: snapshot.credential_published_at_millis,
            })
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            permanent: bool,
        ) -> Result<(), DslTransitionError> {
            self.refresh_failed_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut snapshot = self.snapshot.lock().expect("snapshot lock");
            snapshot.phase = if permanent {
                Some(AuthLeasePhase::ReauthRequired)
            } else {
                Some(AuthLeasePhase::Expiring)
            };
            Ok(())
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            let mut snapshot = self.snapshot.lock().expect("snapshot lock");
            snapshot.phase = None;
            snapshot.expires_at = None;
            snapshot.credential_present = false;
            snapshot.generation += 1;
            snapshot.credential_published_at_millis = None;
            Ok(())
        }

        fn restore_auth_lifecycle_snapshot(
            &self,
            _lease_key: &LeaseKey,
            snapshot: &AuthLeaseSnapshot,
            _expires_at: Option<u64>,
        ) -> Result<(), DslTransitionError> {
            *self.snapshot.lock().expect("snapshot lock") = snapshot.clone();
            Ok(())
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot.lock().expect("snapshot lock").clone()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl AuthLeaseHandle for StaticAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            Ok(AuthLeaseTransition {
                generation: self.snapshot.generation,
                credential_published_at_millis: None,
            })
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            Ok(AuthLeaseTransition {
                generation: self.snapshot.generation,
                credential_published_at_millis: None,
            })
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshot.clone()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn chatgpt_oauth_tokens(secret: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: meerkat_core::auth::PersistedAuthMode::ChatgptOauth,
            primary_secret: Some(secret.into()),
            refresh_token: Some(format!("{secret}-refresh")),
            id_token: None,
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            last_refresh: Some(chrono::Utc::now()),
            scopes: Vec::new(),
            account_id: Some("acct-1".into()),
            metadata: serde_json::Value::Null,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct SaveFailingTokenStore {
        inner: EphemeralTokenStore,
        fail_saves: std::sync::atomic::AtomicBool,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl SaveFailingTokenStore {
        fn new() -> Self {
            Self {
                inner: EphemeralTokenStore::new(),
                fail_saves: std::sync::atomic::AtomicBool::new(false),
            }
        }

        async fn seed(&self, key: &TokenKey, tokens: &PersistedTokens) {
            self.inner.save(key, tokens).await.unwrap();
            self.fail_saves
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        fn allow_saves(&self) {
            self.fail_saves
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait::async_trait]
    impl TokenStore for SaveFailingTokenStore {
        async fn load(
            &self,
            key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, meerkat_core::auth::TokenStoreError> {
            self.inner.load(key).await
        }

        async fn save(
            &self,
            key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            if self.fail_saves.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(meerkat_core::auth::TokenStoreError::Unavailable(
                    "save unavailable".into(),
                ));
            }
            self.inner.save(key, tokens).await
        }

        async fn clear(&self, key: &TokenKey) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.inner.clear(key).await
        }

        async fn list(&self) -> Result<Vec<TokenKey>, meerkat_core::auth::TokenStoreError> {
            self.inner.list().await
        }

        fn backend_name(&self) -> &'static str {
            "save_failing"
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
        let generation = 11;
        store
            .save(
                &key,
                &PersistedTokens::api_key("sk-managed")
                    .with_auth_lease_binding(key.clone(), generation),
            )
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(Arc::new(TestAuthLeaseHandle::valid_non_expiring(
                generation,
            )));

        let secret = resolve_simple_secret(&binding.auth_profile.source, &env, &binding)
            .await
            .unwrap();

        assert_eq!(secret, "sk-managed");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_non_oauth_source_reads_token_without_auth_lifecycle() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_connection_ref(&binding.connection_ref);
        let generation = 11;
        store
            .save(
                &key,
                &PersistedTokens::static_bearer("bearer")
                    .with_auth_lease_binding(key.clone(), generation),
            )
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(Arc::new(TestAuthLeaseHandle::valid_non_expiring(
                generation,
            )));

        let err = resolve_simple_secret(&binding.auth_profile.source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(err, ProviderAuthError::Auth(AuthError::Expired)));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_oauth_refresh_rejects_pending_material_when_final_binding_is_not_persisted() {
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
        let expected = PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("old-access".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()),
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
            auth_lease: None,
        }
        .with_auth_lease_binding(token_key.clone(), 11);
        let refreshed = PersistedTokens {
            primary_secret: Some("new-access".into()),
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap()),
            ..expected.clone()
        };
        let store = Arc::new(ManagedOauthFinalizeRaceTokenStore::new(expected.clone()));
        let handle = Arc::new(RefreshCompletingAuthLeaseHandle::refreshing(
            12,
            Some(1_700_000_000),
        ));
        let env = ResolverEnvironment::testing()
            .with_token_store(store.clone())
            .with_auth_lease_handle(handle.clone());
        let lifecycle = ManagedOauthRefreshLifecycle::AuthMachine(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Refreshing),
            expires_at: Some(1_700_000_000),
            generation: 12,
        });

        let err = save_and_complete_managed_oauth_refresh(
            &env, &binding, &token_key, lifecycle, &expected, &refreshed,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
        assert_eq!(
            handle.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired),
            "pending refreshed material must not be accepted as a finalized lease binding"
        );
        let stored = store.load(&token_key).await.unwrap().unwrap();
        assert_eq!(
            stored
                .auth_lease
                .expect("pending refreshed material remains stored")
                .pending_owner_generation,
            Some(12)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_oauth_refresh_marks_reauth_when_final_binding_verification_load_fails() {
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
        let expected = PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("old-access".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()),
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
            auth_lease: None,
        }
        .with_auth_lease_binding(token_key.clone(), 11);
        let refreshed = PersistedTokens {
            primary_secret: Some("new-access".into()),
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap()),
            ..expected.clone()
        };
        let store = Arc::new(
            ManagedOauthFinalizeRaceTokenStore::new_with_finalize_load_error(expected.clone()),
        );
        let handle = Arc::new(RefreshCompletingAuthLeaseHandle::refreshing(
            12,
            Some(1_700_000_000),
        ));
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(handle.clone());
        let lifecycle = ManagedOauthRefreshLifecycle::AuthMachine(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Refreshing),
            expires_at: Some(1_700_000_000),
            generation: 12,
        });

        let err = save_and_complete_managed_oauth_refresh(
            &env, &binding, &token_key, lifecycle, &expected, &refreshed,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
        assert_eq!(
            handle.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired),
            "unverifiable refreshed material must invalidate the AuthMachine lease"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_oauth_refresh_marks_reauth_when_final_save_errors_after_bound_material_is_visible()
     {
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
        let expected = PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("old-access".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()),
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
            auth_lease: None,
        }
        .with_auth_lease_binding(token_key.clone(), 11);
        let refreshed = PersistedTokens {
            primary_secret: Some("new-access".into()),
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap()),
            ..expected.clone()
        };
        let store = Arc::new(
            ManagedOauthFinalizeRaceTokenStore::new_with_final_save_error_after_write(
                expected.clone(),
            ),
        );
        let handle = Arc::new(RefreshCompletingAuthLeaseHandle::refreshing(
            12,
            Some(1_700_000_000),
        ));
        let env = ResolverEnvironment::testing()
            .with_token_store(store.clone())
            .with_auth_lease_handle(handle.clone());
        let lifecycle = ManagedOauthRefreshLifecycle::AuthMachine(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Refreshing),
            expires_at: Some(1_700_000_000),
            generation: 12,
        });

        let err = save_and_complete_managed_oauth_refresh(
            &env, &binding, &token_key, lifecycle, &expected, &refreshed,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
        assert_eq!(
            handle.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired),
            "reported finalization errors must not be forgiven by reloading visible bound material"
        );
        assert_eq!(
            store.load(&token_key).await.unwrap(),
            None,
            "visible refreshed material from a reported finalization error must be cleared so restart cannot bootstrap it"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn restart_bootstrap_marks_reauth_when_rebound_binding_verification_load_fails() {
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
        let stored = PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("access".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()),
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
            auth_lease: None,
        }
        .with_auth_lease_binding(token_key.clone(), 11);
        let store = Arc::new(
            ManagedOauthFinalizeRaceTokenStore::new_with_finalize_load_error(stored.clone()),
        );
        let handle = Arc::new(RestartAcquiringAuthLeaseHandle::empty(11));
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(handle.clone());

        let err = bootstrap_lease_bound_stored_tokens(
            &env,
            handle.as_ref(),
            &token_key,
            &lease_key,
            &PersistedAuthMode::ChatgptOauth,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
        assert_eq!(
            handle.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired),
            "unverifiable restart rebound material must invalidate the AuthMachine lease"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn restart_bootstrap_marks_reauth_when_rebound_save_errors_after_bound_material_is_visible()
     {
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
        let stored = PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("access".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()),
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
            auth_lease: None,
        }
        .with_auth_lease_binding(token_key.clone(), 11);
        let store = Arc::new(
            ManagedOauthFinalizeRaceTokenStore::new_with_final_save_error_after_write(
                stored.clone(),
            ),
        );
        let handle = Arc::new(RestartAcquiringAuthLeaseHandle::empty(11));
        let env = ResolverEnvironment::testing()
            .with_token_store(store.clone())
            .with_auth_lease_handle(handle.clone());

        let err = bootstrap_lease_bound_stored_tokens(
            &env,
            handle.as_ref(),
            &token_key,
            &lease_key,
            &PersistedAuthMode::ChatgptOauth,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
        assert_eq!(
            handle.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired),
            "reported restart rebound save errors must not be forgiven by reloading visible bound material"
        );
        assert_eq!(
            store.load(&token_key).await.unwrap(),
            None,
            "visible rebound material from a reported save error must be cleared so a later restart cannot bootstrap it"
        );
        let next_handle = Arc::new(RestartAcquiringAuthLeaseHandle::empty(11));
        let next_env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(next_handle.clone());
        let bootstrapped = bootstrap_lease_bound_stored_tokens(
            &next_env,
            next_handle.as_ref(),
            &token_key,
            &lease_key,
            &PersistedAuthMode::ChatgptOauth,
        )
        .await
        .unwrap();
        assert!(
            !bootstrapped,
            "a fresh process must not bootstrap material that was reported failed and cleared"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn restart_bootstrap_clears_rebound_material_when_reauth_mark_fails_after_save_error() {
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let token_key = TokenKey::from_connection_ref(&binding.connection_ref);
        let lease_key = LeaseKey::from_connection_ref(&binding.connection_ref);
        let stored = PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("access".into()),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: Some(DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()),
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
            auth_lease: None,
        }
        .with_auth_lease_binding(token_key.clone(), 11);
        let store = Arc::new(
            ManagedOauthFinalizeRaceTokenStore::new_with_final_save_error_after_write(
                stored.clone(),
            ),
        );
        let handle = Arc::new(RestartAcquiringAuthLeaseHandle::empty_with_reauth_error(11));
        let env = ResolverEnvironment::testing()
            .with_token_store(store.clone())
            .with_auth_lease_handle(handle.clone());

        let err = bootstrap_lease_bound_stored_tokens(
            &env,
            handle.as_ref(),
            &token_key,
            &lease_key,
            &PersistedAuthMode::ChatgptOauth,
        )
        .await
        .unwrap_err();

        let ProviderAuthError::SourceResolutionFailed(message) = err else {
            panic!("expected SourceResolutionFailed");
        };
        assert!(
            message.contains("reauth_marked=error:"),
            "reauth mark failure must be reported in the resolver error"
        );
        assert!(
            message.contains("stale_token_cleared=true"),
            "reported save failure cleanup must run even when reauth marking fails"
        );
        assert_eq!(
            store.load(&token_key).await.unwrap(),
            None,
            "visible rebound material from a reported save error must be cleared even when reauth marking fails"
        );

        let next_handle = Arc::new(RestartAcquiringAuthLeaseHandle::empty(11));
        let next_env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(next_handle.clone());
        let bootstrapped = bootstrap_lease_bound_stored_tokens(
            &next_env,
            next_handle.as_ref(),
            &token_key,
            &lease_key,
            &PersistedAuthMode::ChatgptOauth,
        )
        .await
        .unwrap();
        assert!(
            !bootstrapped,
            "a fresh process must not bootstrap material that was reported failed and cleared"
        );
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
