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
use meerkat_core::AuthStatusPhase;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError, RefreshFn, TokenKey,
    TokenStore,
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
        let managed = load_managed_store_tokens_with_lifecycle(env, binding).await?;
        if managed.lifecycle == ManagedStoreLifecycle::RefreshRequired {
            return Err(refresh_required_error());
        }
        managed.tokens.primary_secret.ok_or_else(|| {
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
fn managed_store_lifecycle_from_phase(phase: AuthStatusPhase) -> ManagedStoreLifecycle {
    match phase {
        AuthStatusPhase::Valid => ManagedStoreLifecycle::Authorized,
        AuthStatusPhase::Expiring
        | AuthStatusPhase::Expired
        | AuthStatusPhase::ReauthRequired
        | AuthStatusPhase::RefreshFailed
        | AuthStatusPhase::Unknown => ManagedStoreLifecycle::RefreshRequired,
    }
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
fn restore_fresh_oauth_lifecycle_from_marker(
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &meerkat_core::handles::LeaseKey,
    snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
    tokens: &PersistedTokens,
) -> Result<Option<meerkat_core::handles::AuthLeaseSnapshot>, ProviderAuthError> {
    if snapshot.phase.is_some() || snapshot.credential_present || snapshot.generation != 0 {
        return Ok(None);
    }
    let Some(publication) = meerkat_core::tokens_lifecycle_publication_with_explicit_expiry(tokens)
    else {
        return Err(stale_credential_error());
    };
    if publication.expires_at != meerkat_core::persisted_token_expires_at_epoch_secs(tokens) {
        return Err(stale_credential_error());
    }
    let Some(generation) = publication.generation else {
        return Err(stale_credential_error());
    };
    let Some(credential_published_at_millis) = publication.credential_published_at_millis else {
        return Err(stale_credential_error());
    };
    let restored = meerkat_core::handles::AuthLeaseSnapshot {
        phase: Some(meerkat_core::handles::AuthLeasePhase::Valid),
        expires_at: (publication.expires_at != u64::MAX).then_some(publication.expires_at),
        credential_present: true,
        generation,
        credential_published_at_millis: Some(credential_published_at_millis),
    };
    auth_lease
        .restore_auth_lifecycle_snapshot(lease_key, &restored, restored.expires_at)
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle restore failed: {e}"
            ))
        })?;
    Ok(Some(auth_lease.snapshot(lease_key)))
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
fn stale_credential_error() -> ProviderAuthError {
    ProviderAuthError::Auth(AuthError::StaleCredential)
}

#[cfg(not(target_arch = "wasm32"))]
fn lease_absent_error() -> ProviderAuthError {
    ProviderAuthError::Auth(AuthError::LeaseAbsent)
}

#[cfg(not(target_arch = "wasm32"))]
fn user_reauth_required_error() -> ProviderAuthError {
    ProviderAuthError::Auth(AuthError::UserReauthRequired)
}

#[cfg(not(target_arch = "wasm32"))]
fn refresh_failed_error() -> ProviderAuthError {
    ProviderAuthError::Auth(AuthError::RefreshFailed(
        "AuthMachine credential refresh failed".into(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn refresh_required_error() -> ProviderAuthError {
    ProviderAuthError::Auth(AuthError::RefreshRequired)
}

#[cfg(not(target_arch = "wasm32"))]
fn auth_lease_phase_error(phase: AuthStatusPhase) -> ProviderAuthError {
    match phase {
        AuthStatusPhase::Valid => ProviderAuthError::Auth(AuthError::Other(
            "internal auth gate error: valid lease rejected".into(),
        )),
        AuthStatusPhase::Expiring | AuthStatusPhase::Expired => refresh_required_error(),
        AuthStatusPhase::ReauthRequired => user_reauth_required_error(),
        AuthStatusPhase::RefreshFailed => refresh_failed_error(),
        AuthStatusPhase::Unknown => lease_absent_error(),
    }
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
    let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(binding.auth_binding_ref());
    let lifecycle_guard = if crate::auth_store::persisted_auth_mode_for_auth_method(
        &binding.auth_profile().auth_method,
    )
    .is_some_and(crate::auth_store::persisted_auth_mode_is_oauth_login)
    {
        Some(meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await)
    } else {
        None
    };
    let tokens = store
        .load(&key)
        .await
        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
        .ok_or_else(|| interactive_login_error(binding))?;
    let expected_mode = require_persisted_auth_mode(&tokens, &binding.auth_profile().auth_method)?;
    let is_oauth_login = crate::auth_store::persisted_auth_mode_is_oauth_login(expected_mode);
    if is_oauth_login && !oauth_lifecycle_marker_payload_valid_for_tokens(&tokens) {
        return Err(stale_credential_error());
    }

    let now = (env.now)();
    let token_phase = AuthStatusPhase::from_lease_expires_at(
        now,
        Some(meerkat_core::persisted_token_expires_at_epoch_secs(&tokens)),
    );
    let lifecycle = managed_store_lifecycle_from_phase(token_phase);

    if let Some(auth_lease) = env.auth_lease_handle.as_ref() {
        let mut snapshot = auth_lease.snapshot(&lease_key);
        if is_oauth_login
            && let Some(restored) = restore_fresh_oauth_lifecycle_from_marker(
                auth_lease.as_ref(),
                &lease_key,
                &snapshot,
                &tokens,
            )?
        {
            snapshot = restored;
        }
        if snapshot.phase == Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired) {
            return Err(user_reauth_required_error());
        }
        let phase = AuthStatusPhase::from_lease_snapshot(now, &snapshot);
        if is_oauth_login
            && matches!(
                phase,
                AuthStatusPhase::Valid | AuthStatusPhase::Expiring | AuthStatusPhase::Expired
            )
        {
            match oauth_lifecycle_marker_relation(&tokens, &snapshot) {
                OAuthLifecycleMarkerRelation::Matches => {}
                OAuthLifecycleMarkerRelation::TokenNewer
                | OAuthLifecycleMarkerRelation::TokenStale
                | OAuthLifecycleMarkerRelation::Invalid => {
                    return Err(stale_credential_error());
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
            AuthStatusPhase::Valid => Ok(managed_store_tokens(
                store,
                key,
                tokens,
                Some(snapshot),
                managed_store_lifecycle_from_phase(phase),
                lifecycle_guard,
            )),
            AuthStatusPhase::Expiring | AuthStatusPhase::Expired => Ok(managed_store_tokens(
                store,
                key,
                tokens,
                Some(snapshot),
                ManagedStoreLifecycle::RefreshRequired,
                lifecycle_guard,
            )),
            AuthStatusPhase::ReauthRequired
            | AuthStatusPhase::RefreshFailed
            | AuthStatusPhase::Unknown => Err(auth_lease_phase_error(phase)),
        };
    }

    if is_oauth_login {
        Err(lease_absent_error())
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
        .ok_or_else(lease_absent_error)?;
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(binding.auth_binding_ref());
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
        Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired) => {
            Err(user_reauth_required_error())
        }
        Some(meerkat_core::handles::AuthLeasePhase::Refreshing) => {
            previous.lifecycle_snapshot = Some(current_snapshot);
            Ok(false)
        }
        Some(meerkat_core::handles::AuthLeasePhase::Released) | None => Err(lease_absent_error()),
        Some(
            meerkat_core::handles::AuthLeasePhase::Valid
            | meerkat_core::handles::AuthLeasePhase::Expiring,
        ) => Err(lease_absent_error()),
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
        .ok_or_else(lease_absent_error)?;
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(binding.auth_binding_ref());
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
        return Err(refresh_required_error());
    }
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(lease_absent_error)?;
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(binding.auth_binding_ref());
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
        .ok_or_else(lease_absent_error)?;
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(binding.auth_binding_ref());
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
        if let Some(current) = current_tokens.as_ref()
            && persisted_token_material_matches(current, refreshed)
            && oauth_lifecycle_marker_relation(current, &current_snapshot)
                == OAuthLifecycleMarkerRelation::Matches
        {
            return Ok(current.clone());
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
        .ok_or_else(lease_absent_error)?;
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(binding.auth_binding_ref());
    let snapshot = auth_lease.snapshot(&lease_key);
    if snapshot.phase == Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired) {
        return Err(user_reauth_required_error());
    }
    let phase = AuthStatusPhase::from_lease_snapshot((env.now)(), &snapshot);
    match phase {
        AuthStatusPhase::Valid | AuthStatusPhase::Expiring => Ok(()),
        AuthStatusPhase::Expired => Err(refresh_required_error()),
        AuthStatusPhase::ReauthRequired
        | AuthStatusPhase::RefreshFailed
        | AuthStatusPhase::Unknown => Err(auth_lease_phase_error(phase)),
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
    let defaults = &binding.auth_profile().metadata_defaults;
    if !binding.policy().allow_auth_override {
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
    binding.auth_profile().constraints.allow_refresh
}

/// Return the auth error to surface when runtime resolution would need an
/// interactive login that the binding does not permit.
pub fn interactive_login_error(binding: &ValidatedBinding) -> ProviderAuthError {
    if binding.auth_profile().constraints.allow_interactive_login {
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
        // Wave-c C-1 follow-up: `AuthBindingRef` has no `Display` impl by
        // wave-b design (the opaque `realm:binding` string form was
        // deleted so no code path silently ferries the join through the
        // runtime). Project realm/binding explicitly at this log/ident
        // site.
        format!(
            "external:{}:{}:{}",
            binding.auth_binding_ref().realm.as_str(),
            binding.auth_binding_ref().binding.as_str(),
            binding.auth_profile().id,
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
    if (binding.policy().require_metadata_account
        || binding.auth_profile().constraints.require_account_id)
        && metadata.account_id.is_none()
    {
        return Err(ProviderAuthError::Auth(AuthError::MissingRequiredMetadata(
            "account_id".into(),
        )));
    }

    if (binding.policy().require_metadata_workspace
        || binding.auth_profile().constraints.require_workspace_id)
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
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat_core::handles::{
        AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition,
        DslTransitionError, LeaseKey,
    };
    use meerkat_core::{
        AuthBindingRef, AuthProfile, AuthRouteHints, BackendProfile, BindingPolicy, Provider,
    };
    use meerkat_llm_core::provider_runtime::{ProviderRuntimeCatalog, ValidatedBinding};

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
        let backend = BackendProfile {
            id: "backend".into(),
            provider: Provider::Gemini,
            backend_kind: "google_genai".into(),
            base_url: None,
            options: serde_json::Value::Null,
        };
        let auth = AuthProfile {
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
        };
        ProviderRuntimeCatalog::validate_binding(
            &AuthBindingRef {
                realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
                binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
                profile: None,
            },
            &backend,
            &auth,
            &BindingPolicy::default(),
        )
        .unwrap()
    }

    fn simple_secret_binding(source: CredentialSourceSpec, auth_method: &str) -> ValidatedBinding {
        let (provider, backend_kind) = match auth_method {
            "managed_chatgpt_oauth" | "external_chatgpt_tokens" => {
                (Provider::OpenAI, "chatgpt_backend")
            }
            _ => (Provider::Gemini, "google_genai"),
        };
        let backend = BackendProfile {
            id: "backend".into(),
            provider,
            backend_kind: backend_kind.into(),
            base_url: None,
            options: serde_json::Value::Null,
        };
        let auth = AuthProfile {
            id: "managed".into(),
            provider,
            auth_method: auth_method.into(),
            source,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        };
        ProviderRuntimeCatalog::validate_binding(
            &AuthBindingRef {
                realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
                binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
                profile: None,
            },
            &backend,
            &auth,
            &BindingPolicy::default(),
        )
        .unwrap()
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

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_source_reads_binding_scoped_token_store() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(&key, &PersistedTokens::api_key("sk-managed"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid());

        let secret = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap();

        assert_eq!(secret, "sk-managed");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_non_oauth_source_reads_token_without_auth_lifecycle() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(&key, &PersistedTokens::api_key("sk-standalone"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing().with_token_store(store);

        let secret = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap();

        assert_eq!(secret, "sk-standalone");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_non_oauth_source_ignores_empty_auth_lifecycle() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(&key, &PersistedTokens::api_key("sk-runtime"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(StaticAuthLeaseHandle::unknown());

        let secret = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap();

        assert_eq!(secret, "sk-runtime");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_non_oauth_source_rejects_released_auth_lifecycle() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(&key, &PersistedTokens::api_key("sk-stale"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(StaticAuthLeaseHandle::released());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::LeaseAbsent)
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_source_rejects_token_without_auth_lifecycle() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(
                &key,
                &PersistedTokens {
                    auth_mode: meerkat_core::auth::PersistedAuthMode::ChatgptOauth,
                    primary_secret: Some("oauth-access".into()),
                    refresh_token: Some("oauth-refresh".into()),
                    id_token: None,
                    expires_at: None,
                    last_refresh: None,
                    scopes: Vec::new(),
                    account_id: None,
                    metadata: serde_json::Value::Null,
                },
            )
            .await
            .unwrap();
        let env = ResolverEnvironment::testing().with_token_store(store);

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_source_rejects_unmarked_token_even_with_valid_lifecycle() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(&key, &chatgpt_oauth_tokens("oauth-access"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_source_rejects_marker_from_stale_lifecycle_generation() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let mut stale_tokens = chatgpt_oauth_tokens("stale-generation-access");
        stale_tokens.metadata = serde_json::json!({
            "meerkat_auth_lifecycle": {
                "published": true,
                "version": 1,
                "generation": 1,
            },
        });
        store.save(&key, &stale_tokens).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid_generation(2));

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_source_rejects_expiring_authmachine_freshness() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let mut tokens = chatgpt_oauth_tokens("expiring-access");
        tokens.expires_at = Some(chrono::Utc::now() + chrono::Duration::seconds(30));
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let transition = AuthLeaseTransition {
            generation: 7,
            credential_published_at_millis: Some(2_000),
        };
        let marked =
            meerkat_core::mark_tokens_lifecycle_published_for_transition(&tokens, transition);
        store.save(&key, &marked).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(
                StaticAuthLeaseHandle::valid_generation_with_expiry_and_publication_time(
                    7,
                    expires_at,
                    Some(2_000),
                ),
            );

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(
            matches!(err, ProviderAuthError::Auth(AuthError::RefreshRequired)),
            "expiring OAuth token must be rejected at the AuthMachine/token-store boundary, got {err}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_empty_lifecycle_rejects_marker_with_mismatched_expiry() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("corrupt-marker-access");
        let token_expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let mut stale_marker = tokens.clone();
        stale_marker.metadata = serde_json::json!({
            "meerkat_auth_lifecycle": {
                "published": true,
                "version": 2,
                "generation": 1,
                "expires_at": token_expires_at + 3600,
            },
        });
        store.save(&key, &stale_marker).await.unwrap();
        let auth_lease = MutableAuthLeaseHandle::unknown();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
        assert_eq!(
            auth_lease.acquire_count(),
            0,
            "corrupt durable marker must not be laundered into a fresh AuthMachine snapshot"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_empty_lifecycle_rejects_marker_missing_explicit_expiry() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let mut incomplete_marker = chatgpt_oauth_tokens("incomplete-marker-access");
        incomplete_marker.metadata = serde_json::json!({
            "meerkat_auth_lifecycle": {
                "published": true,
            },
        });
        store.save(&key, &incomplete_marker).await.unwrap();
        let auth_lease = MutableAuthLeaseHandle::unknown();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
        assert_eq!(
            auth_lease.acquire_count(),
            0,
            "a published marker without an explicit expiry must not rehydrate AuthMachine"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_fresh_runtime_restores_valid_lifecycle_marker() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("fresh-runtime-access");
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let published_at = Some(12_345);
        store
            .save(
                &key,
                &meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &tokens,
                    AuthLeaseTransition {
                        generation: 1,
                        credential_published_at_millis: published_at,
                    },
                ),
            )
            .await
            .unwrap();
        let auth_lease = MutableAuthLeaseHandle::unknown();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());

        let secret = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .expect("fresh CLI process should use a valid lifecycle-marked OAuth token");

        assert_eq!(secret, "fresh-runtime-access");
        assert_eq!(auth_lease.acquire_count(), 0);
        let snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(snapshot.expires_at, Some(expires_at));
        assert!(snapshot.credential_present);
        assert_eq!(snapshot.generation, 1);
        assert_eq!(snapshot.credential_published_at_millis, published_at);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_only_lifecycle_rejects_valid_marked_token() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("oauth-only-access");
        store
            .save(
                &key,
                &meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &tokens,
                    AuthLeaseTransition {
                        generation: 1,
                        credential_published_at_millis: None,
                    },
                ),
            )
            .await
            .unwrap();
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::ReauthRequired),
            expires_at: None,
            credential_present: false,
            generation: 1,
            credential_published_at_millis: None,
        });
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::UserReauthRequired)
        ));
        let snapshot = auth_lease.snapshot(&lease_key);
        assert!(!snapshot.credential_present);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::ReauthRequired));
        assert_eq!(
            auth_lease.acquire_count(),
            0,
            "OAuth token material must not acquire AuthMachine lease truth"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_released_lifecycle_rejects_valid_marked_token() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("released-oauth-access");
        store
            .save(
                &key,
                &meerkat_core::mark_tokens_lifecycle_published_for_generation(&tokens, 1),
            )
            .await
            .unwrap();
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            credential_present: false,
            generation: 1,
            credential_published_at_millis: None,
        });
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::LeaseAbsent)
        ));
        assert_eq!(
            auth_lease.acquire_count(),
            0,
            "explicit release must not rehydrate from durable OAuth token material"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_source_accepts_marker_when_generation_and_publication_match() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("post-consume-access");
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let publication_time = Some(2_000);
        store
            .save(
                &key,
                &meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &tokens,
                    AuthLeaseTransition {
                        generation: 1,
                        credential_published_at_millis: publication_time,
                    },
                ),
            )
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(
                StaticAuthLeaseHandle::valid_generation_with_expiry_and_publication_time(
                    1,
                    expires_at,
                    publication_time,
                ),
            );

        let secret = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .expect("terminal OAuth flow consume must not stale a freshly committed marker");

        assert_eq!(secret, "post-consume-access");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_source_rejects_newer_token_marker_over_existing_lease() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let mut tokens = chatgpt_oauth_tokens("newer-shared-access");
        tokens.expires_at = Some(chrono::Utc::now() + chrono::Duration::hours(2));
        let newer_expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        store
            .save(
                &key,
                &meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &tokens,
                    AuthLeaseTransition {
                        generation: 2,
                        credential_published_at_millis: Some(3_000),
                    },
                ),
            )
            .await
            .unwrap();
        let previous_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(newer_expires_at - 3600),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        };
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(previous_snapshot.clone());
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
        assert_eq!(
            auth_lease.acquire_count(),
            0,
            "a newer durable marker must not acquire or replace an existing AuthMachine lease"
        );
        assert_eq!(auth_lease.snapshot(&lease_key), previous_snapshot);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_compares_publication_time_for_equal_expiry() {
        let mut tokens = chatgpt_oauth_tokens("same-expiry-access");
        tokens.expires_at = Some(chrono::Utc::now() + chrono::Duration::hours(1));
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        };

        let older = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &tokens,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(1_000),
            },
        );
        assert_eq!(
            oauth_lifecycle_marker_relation(&older, &snapshot),
            OAuthLifecycleMarkerRelation::TokenStale
        );

        let newer = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &tokens,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(3_000),
            },
        );
        assert_eq!(
            oauth_lifecycle_marker_relation(&newer, &snapshot),
            OAuthLifecycleMarkerRelation::TokenNewer
        );

        let matching = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &tokens,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(2_000),
            },
        );
        assert_eq!(
            oauth_lifecycle_marker_relation(&matching, &snapshot),
            OAuthLifecycleMarkerRelation::Matches
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_prefers_publication_time_over_expiry() {
        let snapshot_expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(snapshot_expires_at.timestamp().max(0) as u64),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        };

        let mut older_longer = chatgpt_oauth_tokens("older-longer-access");
        older_longer.expires_at = Some(snapshot_expires_at + chrono::Duration::hours(1));
        let older_longer = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &older_longer,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(1_000),
            },
        );
        assert_eq!(
            oauth_lifecycle_marker_relation(&older_longer, &snapshot),
            OAuthLifecycleMarkerRelation::TokenStale
        );

        let mut newer_shorter = chatgpt_oauth_tokens("newer-shorter-access");
        newer_shorter.expires_at = Some(snapshot_expires_at - chrono::Duration::hours(1));
        let newer_shorter = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &newer_shorter,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(3_000),
            },
        );
        assert_eq!(
            oauth_lifecycle_marker_relation(&newer_shorter, &snapshot),
            OAuthLifecycleMarkerRelation::TokenNewer
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_rejects_equal_publication_time_expiry_drift() {
        let snapshot_expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(snapshot_expires_at.timestamp().max(0) as u64),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        };

        let mut same_time_longer = chatgpt_oauth_tokens("same-time-longer-access");
        same_time_longer.expires_at = Some(snapshot_expires_at + chrono::Duration::hours(1));
        let same_time_longer = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &same_time_longer,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(2_000),
            },
        );
        assert_eq!(
            oauth_lifecycle_marker_relation(&same_time_longer, &snapshot),
            OAuthLifecycleMarkerRelation::Invalid,
            "same-ms publication ties must not adopt a token as newer based on expiry drift"
        );

        let mut same_time_shorter = chatgpt_oauth_tokens("same-time-shorter-access");
        same_time_shorter.expires_at = Some(snapshot_expires_at - chrono::Duration::hours(1));
        let same_time_shorter = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &same_time_shorter,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(2_000),
            },
        );
        assert_eq!(
            oauth_lifecycle_marker_relation(&same_time_shorter, &snapshot),
            OAuthLifecycleMarkerRelation::Invalid,
            "same-ms publication ties with expiry drift are inconsistent, not ordered"
        );

        let mut same_time_same_expiry_future_generation =
            chatgpt_oauth_tokens("same-time-same-expiry-future-generation-access");
        same_time_same_expiry_future_generation.expires_at = Some(snapshot_expires_at);
        let same_time_same_expiry_future_generation =
            meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &same_time_same_expiry_future_generation,
                AuthLeaseTransition {
                    generation: 3,
                    credential_published_at_millis: Some(2_000),
                },
            );
        assert_eq!(
            oauth_lifecycle_marker_relation(&same_time_same_expiry_future_generation, &snapshot),
            OAuthLifecycleMarkerRelation::Invalid
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_rejects_same_time_same_expiry_older_generation() {
        let snapshot_expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(snapshot_expires_at.timestamp().max(0) as u64),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        };
        let mut tokens = chatgpt_oauth_tokens("same-time-same-expiry-stale-generation-access");
        tokens.expires_at = Some(snapshot_expires_at);
        let stale_same_time = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &tokens,
            AuthLeaseTransition {
                generation: 1,
                credential_published_at_millis: Some(2_000),
            },
        );

        assert_eq!(
            oauth_lifecycle_marker_relation(&stale_same_time, &snapshot),
            OAuthLifecycleMarkerRelation::Invalid,
            "same-ms publication ties must not let older credential generations masquerade as current"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_rejects_equal_expiry_without_publication_match() {
        let mut tokens = chatgpt_oauth_tokens("equal-expiry-no-publication-access");
        tokens.expires_at = Some(chrono::Utc::now() + chrono::Duration::hours(1));
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: None,
        };
        let marker = meerkat_core::mark_tokens_lifecycle_published_for_generation(&tokens, 1);

        assert_eq!(
            oauth_lifecycle_marker_relation(&marker, &snapshot),
            OAuthLifecycleMarkerRelation::Invalid,
            "equal finite expiry alone must not let an older marker match a newer AuthMachine snapshot"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn managed_store_oauth_refresh_failure_classifies_permanent_credentials() {
        assert!(managed_store_oauth_refresh_failure_is_permanent(
            "missing refresh_token"
        ));
        assert!(managed_store_oauth_refresh_failure_is_permanent(
            "token endpoint error: status=400 body={\"error\":\"invalid_grant\"}"
        ));
        assert!(managed_store_oauth_refresh_failure_is_permanent(
            "token endpoint error: status=400 body={\"error\":\"invalid_grant\",\"error_description\":\"try again\"}"
        ));
        assert!(!managed_store_oauth_refresh_failure_is_permanent(
            "token endpoint error: status=400 body={\"error\":\"temporarily_unavailable\"}"
        ));
        assert!(managed_store_oauth_refresh_failure_is_permanent(
            "token endpoint error: status=401 body=unauthorized"
        ));
        assert!(managed_store_oauth_refresh_failure_is_permanent(
            "token endpoint error: status=403 body=forbidden"
        ));
        assert!(!managed_store_oauth_refresh_failure_is_permanent(
            "token endpoint error: status=500 body=try later"
        ));
        assert!(!managed_store_oauth_refresh_failure_is_permanent(
            "network error: connection reset"
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_refresh_rejects_newer_token_marker_over_existing_lease() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("token-newer-refresh-access");
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let previous_tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &tokens,
            AuthLeaseTransition {
                generation: 1,
                credential_published_at_millis: Some(2_000),
            },
        );
        let current_tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &tokens,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(3_000),
            },
        );
        store.save(&key, &current_tokens).await.unwrap();
        let initial_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        };
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(initial_snapshot.clone());
        let previous = managed_store_tokens(
            Arc::clone(&store),
            key.clone(),
            previous_tokens,
            Some(initial_snapshot),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.clone());

        let err =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &tokens)
                .await
                .unwrap_err();

        assert!(
            err.to_string().contains("changed during OAuth refresh"),
            "got {err}"
        );
        let snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(snapshot.generation, 1);
        assert_eq!(snapshot.credential_published_at_millis, Some(2_000));
        assert_eq!(
            store.load(&key).await.unwrap().unwrap(),
            current_tokens,
            "rejecting TokenNewer must not rewrite the token-store marker"
        );
        assert_eq!(auth_lease.acquire_count(), 0);
        assert_eq!(auth_lease.begin_refresh_count(), 0);
        assert_eq!(auth_lease.complete_refresh_count(), 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_refresh_commit_uses_authmachine_refresh_lifecycle() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let previous_tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &chatgpt_oauth_tokens("expired-access"),
            AuthLeaseTransition {
                generation: 1,
                credential_published_at_millis: Some(2_000),
            },
        );
        store.save(&key, &previous_tokens).await.unwrap();
        let previous_expires_at =
            meerkat_core::persisted_token_expires_at_epoch_secs(&previous_tokens);
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(previous_expires_at),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        });
        let previous = ManagedStoreTokens {
            store: Arc::clone(&store),
            key: key.clone(),
            tokens: previous_tokens,
            lifecycle_snapshot: Some(auth_lease.snapshot(&lease_key)),
            lifecycle: ManagedStoreLifecycle::RefreshRequired,
            lifecycle_guard: None,
        };
        let refreshed = chatgpt_oauth_tokens("refreshed-access");
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.clone());

        let committed =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &refreshed)
                .await
                .expect("refresh commit succeeds");

        assert_eq!(
            committed.primary_secret.as_deref(),
            Some("refreshed-access")
        );
        assert_eq!(
            auth_lease.acquire_count(),
            0,
            "managed OAuth refresh must complete an AuthMachine refresh, not publish a fresh acquire"
        );
        assert_eq!(auth_lease.begin_refresh_count(), 1);
        assert_eq!(auth_lease.complete_refresh_count(), 1);
        assert_eq!(auth_lease.refresh_failed_count(), 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn adopted_refresh_failure_does_not_poison_inflight_owner_lifecycle() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("refreshing-access");
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Refreshing),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        });
        let mut previous = managed_store_tokens(
            Arc::clone(&store),
            key,
            tokens,
            Some(auth_lease.snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());

        let refresh_started =
            begin_managed_store_oauth_refresh_lifecycle(&env, &binding, &mut previous)
                .expect("existing refreshing lifecycle can be adopted");
        assert!(
            !refresh_started,
            "adopted refresh should not publish a second begin transition"
        );

        mark_managed_store_oauth_refresh_failed(&env, &binding, refresh_started, true)
            .expect("non-owner refresh failure should not mutate shared lifecycle");

        assert_eq!(auth_lease.begin_refresh_count(), 0);
        assert_eq!(
            auth_lease.refresh_failed_count(),
            0,
            "a non-owner refresh failure must not poison the in-flight owner's lifecycle"
        );
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::Refreshing)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn managed_store_oauth_owned_refresh_failure_publishes_authmachine_failure() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("refreshing-access");
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        });
        let mut previous = managed_store_tokens(
            Arc::clone(&store),
            key,
            tokens,
            Some(auth_lease.snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());

        let refresh_started =
            begin_managed_store_oauth_refresh_lifecycle(&env, &binding, &mut previous)
                .expect("valid lifecycle can enter refreshing");
        assert!(
            refresh_started,
            "owned refresh should publish a begin transition"
        );
        mark_managed_store_oauth_refresh_failed(&env, &binding, refresh_started, true)
            .expect("owned permanent failure should publish AuthMachine failure");

        assert_eq!(auth_lease.begin_refresh_count(), 1);
        assert_eq!(auth_lease.refresh_failed_count(), 1);
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_transient_refresh_failure_keeps_retryable_marker() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let mut expired_tokens = chatgpt_oauth_tokens("transient-refresh-access");
        expired_tokens.expires_at = Some(chrono::Utc::now() - chrono::Duration::minutes(5));
        let tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &expired_tokens,
            AuthLeaseTransition {
                generation: 1,
                credential_published_at_millis: Some(2_000),
            },
        );
        store.save(&key, &tokens).await.unwrap();
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Expiring),
            expires_at: Some(meerkat_core::persisted_token_expires_at_epoch_secs(&tokens)),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        });
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.clone());
        let mut loaded = load_managed_store_tokens_with_lifecycle(&env, &binding)
            .await
            .expect("expiring managed OAuth token should load for refresh");
        assert!(matches!(
            loaded.lifecycle,
            ManagedStoreLifecycle::RefreshRequired
        ));

        let refresh_started =
            begin_managed_store_oauth_refresh_lifecycle(&env, &binding, &mut loaded)
                .expect("refresh lifecycle should begin");
        assert!(refresh_started);
        mark_managed_store_oauth_refresh_failed(&env, &binding, refresh_started, false)
            .expect("transient refresh failure should publish retryable lifecycle");

        let after_failure = auth_lease.snapshot(&lease_key);
        assert_eq!(after_failure.phase, Some(AuthLeasePhase::Expiring));
        drop(loaded);
        let retryable = load_managed_store_tokens_with_lifecycle(&env, &binding)
            .await
            .expect("transient refresh failure must leave stored OAuth marker retryable");
        assert!(matches!(
            retryable.lifecycle,
            ManagedStoreLifecycle::RefreshRequired
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn cancelled_oauth_refresh_failure_publishes_owner_lifecycle_failure() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("cancelled-refresh-access");
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        });
        let mut previous = managed_store_tokens(
            Arc::clone(&store),
            key.clone(),
            tokens,
            Some(auth_lease.snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());
        let refresh_started =
            begin_managed_store_oauth_refresh_lifecycle(&env, &binding, &mut previous)
                .expect("valid lifecycle can enter refreshing");
        assert!(refresh_started);
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::Refreshing)
        );

        let coord = managed_store_oauth_refresh_failure_coordinator(
            Arc::new(crate::InMemoryCoordinator::new()),
            env.clone(),
            binding.clone(),
            refresh_started,
        );
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let refresh_fn: crate::auth_store::RefreshFn = Box::new(move || {
            Box::pin(async move {
                let _ = started_tx.send(());
                let _ = release_rx.await;
                Err(crate::auth_store::RefreshError::Refresh(
                    "status=400 body=invalid_grant".to_string(),
                ))
            })
        });
        let waiter = tokio::spawn({
            let coord = Arc::clone(&coord);
            async move { coord.with_refresh(key, refresh_fn).await }
        });
        started_rx
            .await
            .expect("background refresh should start before waiter cancellation");
        waiter.abort();
        let _ = waiter.await;
        release_tx
            .send(())
            .expect("refresh closure should still be running after waiter cancellation");

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if auth_lease.refresh_failed_count() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background refresh failure should publish AuthMachine failure");
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::ReauthRequired)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dropped_oauth_refresh_before_coordinator_claim_marks_transient_failure() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("pre-coordinator-cancel-access");
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        });
        let mut previous = managed_store_tokens(
            Arc::clone(&store),
            key,
            tokens,
            Some(auth_lease.snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());
        let refresh_started =
            begin_managed_store_oauth_refresh_lifecycle(&env, &binding, &mut previous)
                .expect("valid lifecycle can enter refreshing");
        assert!(refresh_started);
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::Refreshing)
        );

        let coord = managed_store_oauth_refresh_failure_coordinator(
            Arc::new(crate::InMemoryCoordinator::new()),
            env,
            binding,
            refresh_started,
        );
        drop(coord);

        assert_eq!(
            auth_lease.refresh_failed_count(),
            1,
            "dropping refresh ownership before coordinator claim should release Refreshing"
        );
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::Expiring)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct RejectingRefreshCoordinator;

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait::async_trait]
    impl RefreshCoordinator for RejectingRefreshCoordinator {
        async fn with_refresh(
            &self,
            _key: TokenKey,
            _refresh_fn: RefreshFn,
        ) -> Result<PersistedTokens, RefreshError> {
            Err(RefreshError::Cancelled)
        }

        async fn with_forced_refresh(
            &self,
            _key: TokenKey,
            _refresh_fn: RefreshFn,
        ) -> Result<PersistedTokens, RefreshError> {
            Err(RefreshError::Cancelled)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn oauth_refresh_inner_coordinator_rejection_marks_transient_failure() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("inner-coordinator-reject-access");
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(2_000),
        });
        let mut previous = managed_store_tokens(
            Arc::clone(&store),
            key.clone(),
            tokens,
            Some(auth_lease.snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.clone());
        let refresh_started =
            begin_managed_store_oauth_refresh_lifecycle(&env, &binding, &mut previous)
                .expect("valid lifecycle can enter refreshing");
        assert!(refresh_started);

        let coord = managed_store_oauth_refresh_failure_coordinator(
            Arc::new(RejectingRefreshCoordinator),
            env,
            binding,
            refresh_started,
        );
        let refresh_fn: crate::auth_store::RefreshFn =
            Box::new(|| Box::pin(async { panic!("inner coordinator must not invoke refresh_fn") }));

        assert!(matches!(
            coord.with_refresh(key, refresh_fn).await,
            Err(RefreshError::Cancelled)
        ));
        assert_eq!(
            auth_lease.refresh_failed_count(),
            1,
            "inner coordinator rejection before provider ownership must release Refreshing"
        );
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::Expiring)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_resolution_holds_lifecycle_guard_until_commit_boundary() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let initial_tokens = meerkat_core::mark_tokens_lifecycle_published_for_generation(
            &chatgpt_oauth_tokens("expired-access"),
            1,
        );
        store.save(&key, &initial_tokens).await.unwrap();
        let auth_lease = StaticAuthLeaseHandle::valid_generation_with_expiry(
            1,
            meerkat_core::persisted_token_expires_at_epoch_secs(&initial_tokens),
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.clone());

        let first = load_managed_store_tokens_with_lifecycle(&env, &binding)
            .await
            .unwrap();
        assert!(first.lifecycle_guard.is_some());

        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let second_store = Arc::clone(&store);
        let second_auth_lease = auth_lease.clone();
        tokio::spawn(async move {
            let binding =
                simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
            let env = ResolverEnvironment::testing()
                .with_token_store(second_store)
                .with_auth_lease_handle(second_auth_lease);
            let result = load_managed_store_tokens_with_lifecycle(&env, &binding)
                .await
                .map(|_| ());
            let _ = done_tx.send(result);
        });

        tokio::pin!(done_rx);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut done_rx)
                .await
                .is_err(),
            "a second resolver should wait while the first holds the lifecycle guard"
        );

        drop(first);
        tokio::time::timeout(std::time::Duration::from_secs(1), &mut done_rx)
            .await
            .expect("second resolver should finish after the guard drops")
            .expect("second resolver should report its result")
            .expect("second resolver should succeed");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_refresh_commit_rejects_stale_loaded_tokens() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let auth_lease = StaticAuthLeaseHandle::valid();
        let previous_tokens = chatgpt_oauth_tokens("expired-access");
        store.save(&key, &previous_tokens).await.unwrap();
        let previous = ManagedStoreTokens {
            store: Arc::clone(&store),
            key: key.clone(),
            tokens: previous_tokens,
            lifecycle_snapshot: Some(auth_lease.snapshot(&lease_key)),
            lifecycle: ManagedStoreLifecycle::RefreshRequired,
            lifecycle_guard: None,
        };

        let newer_tokens = chatgpt_oauth_tokens("newer-login-access");
        store.save(&key, &newer_tokens).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease);

        let err = publish_managed_store_tokens_lifecycle_and_save(
            &env,
            &binding,
            &previous,
            &chatgpt_oauth_tokens("slow-refresh-access"),
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string().contains("changed during OAuth refresh"),
            "got {err}"
        );
        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored, newer_tokens);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_refresh_commit_accepts_already_committed_shared_refresh() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let previous_tokens = meerkat_core::mark_tokens_lifecycle_published_for_generation(
            &chatgpt_oauth_tokens("expired-access"),
            1,
        );
        let auth_lease = StaticAuthLeaseHandle::valid_generation_with_expiry(
            1,
            meerkat_core::persisted_token_expires_at_epoch_secs(&previous_tokens),
        );
        store.save(&key, &previous_tokens).await.unwrap();
        let previous = ManagedStoreTokens {
            store: Arc::clone(&store),
            key: key.clone(),
            tokens: previous_tokens,
            lifecycle_snapshot: Some(auth_lease.snapshot(&lease_key)),
            lifecycle: ManagedStoreLifecycle::RefreshRequired,
            lifecycle_guard: None,
        };
        let refreshed = chatgpt_oauth_tokens("shared-refresh-access");
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease);

        let first =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &refreshed)
                .await
                .unwrap();
        let second =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &refreshed)
                .await
                .unwrap();

        assert_eq!(second, first);
        assert_eq!(second, store.load(&key).await.unwrap().unwrap());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_refresh_rejects_newer_token_marker_over_refreshing_lease() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let previous_tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &chatgpt_oauth_tokens("expired-access"),
            AuthLeaseTransition {
                generation: 1,
                credential_published_at_millis: Some(2_000),
            },
        );
        let mut refreshed = chatgpt_oauth_tokens("shared-refresh-complete-access");
        refreshed.expires_at = Some(chrono::Utc::now() + chrono::Duration::hours(2));
        let shared_tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &refreshed,
            AuthLeaseTransition {
                generation: 2,
                credential_published_at_millis: Some(3_000),
            },
        );
        store.save(&key, &previous_tokens).await.unwrap();
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Refreshing),
            expires_at: Some(meerkat_core::persisted_token_expires_at_epoch_secs(
                &previous_tokens,
            )),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        });
        let previous = ManagedStoreTokens {
            store: Arc::clone(&store),
            key: key.clone(),
            tokens: previous_tokens,
            lifecycle_snapshot: Some(auth_lease.snapshot(&lease_key)),
            lifecycle: ManagedStoreLifecycle::RefreshRequired,
            lifecycle_guard: None,
        };
        store.save(&key, &shared_tokens).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.clone());

        let err =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &refreshed)
                .await
                .unwrap_err();

        assert!(
            err.to_string().contains("changed during OAuth refresh"),
            "got {err}"
        );
        assert_eq!(
            auth_lease.acquire_count(),
            0,
            "TokenNewer refresh adoption must not bypass AuthMachine lease ownership"
        );
        assert_eq!(auth_lease.begin_refresh_count(), 0);
        assert_eq!(auth_lease.complete_refresh_count(), 0);
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(AuthLeasePhase::Refreshing)
        );
        assert_eq!(store.load(&key).await.unwrap().unwrap(), shared_tokens);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_source_rejects_wrong_token_mode() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(&key, &PersistedTokens::static_bearer("bearer"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
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
