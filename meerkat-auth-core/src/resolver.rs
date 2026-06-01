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
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError,
    RefreshFailureObservation, RefreshFn, TokenKey, TokenStore,
};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::generated::auth_lease_durable_lifecycle_marker as durable_marker;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::handles::{AuthLeasePhase, CredentialUseDisposition};
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
    #[doc(hidden)]
    pub lifecycle_restore_snapshot: Option<meerkat_core::handles::AuthLeaseRestoreSnapshot>,
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
fn refresh_required_error() -> ProviderAuthError {
    ProviderAuthError::Auth(AuthError::RefreshRequired)
}

/// Drive the per-binding AuthMachine's credential-use admission classifier and
/// mirror the emitted disposition. The machine owns the `(lifecycle_phase,
/// credential_present, intent)` -> disposition POLICY; this shell helper only
/// translates the handle's `DslTransitionError` into a resolver error and never
/// decides the disposition.
#[cfg(not(target_arch = "wasm32"))]
fn resolve_credential_use_admission(
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    lease_key: &meerkat_core::handles::LeaseKey,
    intent: meerkat_core::handles::CredentialUseIntent,
) -> Result<CredentialUseDisposition, ProviderAuthError> {
    auth_lease
        .resolve_credential_use_admission(lease_key, intent)
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine credential-use admission classification failed: {e}"
            ))
        })
}

#[cfg(not(target_arch = "wasm32"))]
fn credential_phase_from_snapshot(
    snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
) -> Option<AuthLeasePhase> {
    snapshot
        .credential_present
        .then_some(snapshot.phase)
        .flatten()
}

#[cfg(not(target_arch = "wasm32"))]
async fn restore_marked_token_lifecycle_if_absent(
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    store: &dyn TokenStore,
    binding: &ValidatedBinding,
    expected_mode: PersistedAuthMode,
    lease_key: &meerkat_core::handles::LeaseKey,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), ProviderAuthError> {
    let snapshot = auth_lease.snapshot(lease_key);
    if snapshot.phase.is_some()
        || snapshot.credential_present
        || snapshot.generation != 0
        || snapshot.credential_published_at_millis.is_some()
    {
        return Ok(());
    }
    meerkat_core::rehydrate_marked_tokens_for_status(
        store,
        auth_lease,
        binding.auth_binding_ref(),
        expected_mode,
        now,
    )
    .await
    .map(|_| ())
    .map_err(|error| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "AuthMachine lifecycle restore failed: {error}"
        ))
    })
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
    if is_oauth_login && !durable_marker::marker_payload_valid_for_tokens(&tokens, &key) {
        return Err(stale_credential_error());
    }

    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(lease_absent_error)?;
    let now = (env.now)();
    restore_marked_token_lifecycle_if_absent(
        auth_lease,
        store.as_ref(),
        binding,
        expected_mode,
        &lease_key,
        now,
    )
    .await?;
    observe_auth_lease_freshness_for_now(auth_lease.as_ref(), &lease_key, now)?;
    let restore_snapshot = auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key);
    let snapshot = restore_snapshot.snapshot().clone();
    let phase = credential_phase_from_snapshot(&snapshot);
    if is_oauth_login
        && matches!(
            phase,
            Some(
                AuthLeasePhase::Valid
                    | AuthLeasePhase::Expiring
                    | AuthLeasePhase::Expired
                    | AuthLeasePhase::Refreshing
            )
        )
    {
        match durable_marker::marker_relation_for_tokens_and_snapshot(&tokens, &snapshot, &key) {
            durable_marker::AuthLeaseDurableMarkerRelation::Matches => {}
            durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
            | durable_marker::AuthLeaseDurableMarkerRelation::TokenStale
            | durable_marker::AuthLeaseDurableMarkerRelation::Invalid => {
                return Err(stale_credential_error());
            }
        }
    }

    // The credential-use disposition is owned by the per-binding AuthMachine,
    // not this shell: we feed only the typed `UseCredential` intent and mirror
    // the emitted verdict. Authorized -> usable now, RefreshRequired -> mark for
    // refresh, ReauthRequired/LeaseAbsent -> the matching error. AlreadyRefreshing
    // cannot arise for the `UseCredential` intent.
    let lifecycle = match resolve_credential_use_admission(
        auth_lease,
        &lease_key,
        meerkat_core::handles::CredentialUseIntent::UseCredential,
    )? {
        CredentialUseDisposition::Authorized => ManagedStoreLifecycle::Authorized,
        CredentialUseDisposition::RefreshRequired => ManagedStoreLifecycle::RefreshRequired,
        CredentialUseDisposition::ReauthRequired => return Err(user_reauth_required_error()),
        CredentialUseDisposition::LeaseAbsent | CredentialUseDisposition::AlreadyRefreshing => {
            return Err(lease_absent_error());
        }
    };
    Ok(managed_store_tokens(
        store,
        key,
        tokens,
        Some(snapshot),
        Some(restore_snapshot),
        lifecycle,
        lifecycle_guard,
    ))
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
    lifecycle_restore_snapshot: Option<meerkat_core::handles::AuthLeaseRestoreSnapshot>,
    lifecycle: ManagedStoreLifecycle,
    lifecycle_guard: Option<meerkat_core::AuthLoginLifecycleGuard>,
) -> ManagedStoreTokens {
    ManagedStoreTokens {
        store,
        key,
        tokens,
        lifecycle_snapshot,
        lifecycle_restore_snapshot,
        lifecycle,
        lifecycle_guard,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn epoch_secs(ts: chrono::DateTime<chrono::Utc>) -> u64 {
    ts.timestamp().max(0) as u64
}

#[cfg(not(target_arch = "wasm32"))]
fn observe_auth_lease_freshness_for_now(
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    lease_key: &meerkat_core::handles::LeaseKey,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), ProviderAuthError> {
    auth_lease
        .observe_credential_freshness(
            lease_key,
            epoch_secs(now),
            meerkat_core::handles::AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
        )
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle freshness observation failed: {e}"
            ))
        })
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
    observe_auth_lease_freshness_for_now(auth_lease.as_ref(), &lease_key, (env.now)())?;
    let current_restore_snapshot = auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key);
    let current_snapshot = current_restore_snapshot.snapshot().clone();
    if current_snapshot.phase == Some(meerkat_core::handles::AuthLeasePhase::Refreshing) {
        previous.lifecycle_snapshot = Some(current_snapshot);
        previous.lifecycle_restore_snapshot = Some(current_restore_snapshot);
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
    // The begin-refresh disposition is owned by the per-binding AuthMachine: we
    // feed only the typed `BeginRefresh` intent and mirror the verdict.
    // RefreshRequired -> begin the refresh and report it started (Ok(true));
    // AlreadyRefreshing -> a refresh is already in flight (Ok(false));
    // ReauthRequired/LeaseAbsent -> the matching error.
    match resolve_credential_use_admission(
        env.auth_lease_handle
            .as_ref()
            .ok_or_else(lease_absent_error)?,
        &lease_key,
        meerkat_core::handles::CredentialUseIntent::BeginRefresh,
    )? {
        CredentialUseDisposition::RefreshRequired => {
            auth_lease.begin_refresh(&lease_key).map_err(|e| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "AuthMachine lifecycle begin_refresh failed: {e}"
                ))
            })?;
            let refreshing_snapshot =
                auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key);
            previous.lifecycle_snapshot = Some(refreshing_snapshot.snapshot().clone());
            previous.lifecycle_restore_snapshot = Some(refreshing_snapshot);
            Ok(true)
        }
        CredentialUseDisposition::AlreadyRefreshing => {
            previous.lifecycle_snapshot = Some(current_snapshot);
            previous.lifecycle_restore_snapshot = Some(current_restore_snapshot);
            Ok(false)
        }
        CredentialUseDisposition::ReauthRequired => Err(user_reauth_required_error()),
        CredentialUseDisposition::LeaseAbsent | CredentialUseDisposition::Authorized => {
            Err(lease_absent_error())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn mark_managed_store_oauth_refresh_failed(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    refresh_started: bool,
    observation: RefreshFailureObservation,
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
        .refresh_failed(&lease_key, observation)
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
                RefreshFailureObservation::transient(),
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
                if let Err(err) = result.as_ref()
                    && let Err(lifecycle_err) = mark_managed_store_oauth_refresh_failed(
                        &env,
                        &binding,
                        refresh_started,
                        err.observation(),
                    )
                {
                    return Err(RefreshError::Refresh(format!("{err}; {lifecycle_err}")));
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
fn publish_managed_store_tokens_refresh_lifecycle(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    tokens: &PersistedTokens,
) -> Result<meerkat_core::handles::AuthLeaseTransition, ProviderAuthError> {
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(lease_absent_error)?;
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(binding.auth_binding_ref());
    observe_auth_lease_freshness_for_now(auth_lease.as_ref(), &lease_key, (env.now)())?;
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
                let _ =
                    auth_lease.refresh_failed(&lease_key, RefreshFailureObservation::transient());
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
    let previous_restore_snapshot =
        previous
            .lifecycle_restore_snapshot
            .as_ref()
            .ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "managed_store OAuth refresh missing AuthMachine lifecycle restore token"
                        .into(),
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
            && durable_marker::marker_relation_for_tokens_and_snapshot(
                current,
                &current_snapshot,
                &previous.key,
            ) == durable_marker::AuthLeaseDurableMarkerRelation::Matches
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
    let committed = meerkat_core::mark_tokens_lifecycle_published_for_transition(
        &previous.key,
        refreshed,
        &transition,
    )
    .map_err(|err| ProviderAuthError::SourceResolutionFailed(err.to_string()))?;
    if let Err(save_error) = previous.store.save(&previous.key, &committed).await {
        let mut rollback_errors = Vec::new();
        if let Err(err) = auth_lease.release_credential_lifecycle(&lease_key) {
            rollback_errors.push(format!(
                "AuthMachine lifecycle rollback release failed: {err}"
            ));
        }
        let mut restored_previous = previous.tokens.clone();
        let restored_transition = match meerkat_core::restore_token_lifecycle_snapshot(
            auth_lease,
            previous_restore_snapshot,
        ) {
            Ok(restored_transition) => restored_transition,
            Err(err) => {
                rollback_errors.push(format!("AuthMachine lifecycle rollback failed: {err}"));
                None
            }
        };
        if let Some(restored_transition) = restored_transition {
            restored_previous = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &previous.key,
                &previous.tokens,
                &restored_transition,
            )
            .map_err(|err| ProviderAuthError::SourceResolutionFailed(err.to_string()))?;
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
    // The post-publish lifecycle-authority disposition is owned by the
    // per-binding AuthMachine: we feed only the typed `HoldAuthority` intent and
    // mirror the verdict. Authorized -> authority held, RefreshRequired -> the
    // refresh-required error, ReauthRequired/LeaseAbsent -> the matching error.
    // AlreadyRefreshing cannot arise for the `HoldAuthority` intent.
    match resolve_credential_use_admission(
        auth_lease,
        &lease_key,
        meerkat_core::handles::CredentialUseIntent::HoldAuthority,
    )? {
        CredentialUseDisposition::Authorized => Ok(()),
        CredentialUseDisposition::RefreshRequired => Err(refresh_required_error()),
        CredentialUseDisposition::ReauthRequired => Err(user_reauth_required_error()),
        CredentialUseDisposition::LeaseAbsent | CredentialUseDisposition::AlreadyRefreshing => {
            Err(lease_absent_error())
        }
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
        GeneratedAuthLeaseHandle, LeaseKey,
    };
    use meerkat_core::{
        AuthBindingRef, AuthProfile, AuthRouteHints, BackendProfile, BindingPolicy, Provider,
    };
    use meerkat_llm_core::provider_runtime::{ProviderRuntimeCatalog, ValidatedBinding};

    #[cfg(not(target_arch = "wasm32"))]
    fn generated_auth_lease_handle_for_test(
        handle: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
    ) -> GeneratedAuthLeaseHandle {
        meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
            handle,
        )
        .expect("runtime AuthLeaseHandle is certified by generated AuthMachine authority")
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fixture_refresh_observation_time(expires_at: u64) -> u64 {
        if expires_at == u64::MAX {
            0
        } else {
            expires_at.saturating_sub(1)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn mark_tokens_lifecycle_published_for_transition_for_test(
        tokens: &PersistedTokens,
        transition: &AuthLeaseTransition,
    ) -> PersistedTokens {
        let key = default_test_token_key();
        meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, tokens, transition)
            .expect("generated AuthMachine transition marks fixture tokens")
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn mark_tokens_lifecycle_published_for_test(
        tokens: &PersistedTokens,
        generation: u64,
    ) -> PersistedTokens {
        let handle = meerkat_runtime::RuntimeAuthLeaseHandle::new();
        let lease_key = default_test_lease_key();
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(tokens);
        let mut transition = handle
            .acquire_lease(&lease_key, expires_at)
            .expect("fixture AuthMachine accepts acquired lease");
        let target_generation = generation.max(1);
        while transition.generation() < target_generation {
            handle
                .begin_refresh(&lease_key)
                .expect("fixture AuthMachine accepts refresh start");
            transition = handle
                .complete_refresh(
                    &lease_key,
                    expires_at,
                    fixture_refresh_observation_time(expires_at),
                )
                .expect("fixture AuthMachine accepts refresh completion");
        }
        mark_tokens_lifecycle_published_for_transition_for_test(tokens, &transition)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn marker_credential_published_at_for_test(tokens: &PersistedTokens) -> u64 {
        meerkat_core::tokens_lifecycle_publication(tokens)
            .and_then(|publication| publication.credential_published_at_millis)
            .expect("generated fixture marker carries publication time")
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn mark_tokens_lifecycle_published_after_time_for_test(
        tokens: &PersistedTokens,
        generation: u64,
        after_millis: u64,
    ) -> PersistedTokens {
        for _ in 0..100 {
            let marked = mark_tokens_lifecycle_published_for_test(tokens, generation);
            if marker_credential_published_at_for_test(&marked) > after_millis {
                return marked;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("fixture AuthMachine publication clock did not advance");
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
    fn default_test_lease_key() -> LeaseKey {
        LeaseKey::from_auth_binding(&AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
            profile: None,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn default_test_token_key() -> TokenKey {
        let lease_key = default_test_lease_key();
        TokenKey::new_with_profile(lease_key.realm, lease_key.binding, lease_key.profile)
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct StaticAuthLeaseHandle {
        handle: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
        publication_transition: Option<AuthLeaseTransition>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl StaticAuthLeaseHandle {
        fn valid() -> Arc<Self> {
            Self::valid_generation(1)
        }

        fn valid_generation(generation: u64) -> Arc<Self> {
            Self::valid_generation_with_expiry(generation, u64::MAX)
        }

        fn valid_generation_with_expiry(generation: u64, expires_at: u64) -> Arc<Self> {
            let lease_key = default_test_lease_key();
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let mut transition = handle
                .acquire_lease(&lease_key, expires_at)
                .expect("fixture AuthMachine accepts acquired lease");
            while transition.generation() < generation.max(1) {
                handle
                    .begin_refresh(&lease_key)
                    .expect("fixture AuthMachine accepts refresh start");
                transition = handle
                    .complete_refresh(
                        &lease_key,
                        expires_at,
                        fixture_refresh_observation_time(expires_at),
                    )
                    .expect("fixture AuthMachine accepts refresh completion");
            }
            Arc::new(Self {
                handle,
                publication_transition: Some(transition),
            })
        }

        fn unknown() -> Arc<Self> {
            Arc::new(Self {
                handle: Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new()),
                publication_transition: None,
            })
        }

        fn released() -> Arc<Self> {
            Self::unknown()
        }

        fn generated(&self) -> GeneratedAuthLeaseHandle {
            generated_auth_lease_handle_for_test(Arc::clone(&self.handle))
        }

        fn snapshot(&self, lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.handle.snapshot(lease_key)
        }

        fn mark_tokens_lifecycle_published_for_test(
            &self,
            tokens: &PersistedTokens,
        ) -> PersistedTokens {
            mark_tokens_lifecycle_published_for_transition_for_test(
                tokens,
                self.publication_transition
                    .as_ref()
                    .expect("fixture has generated credential publication transition"),
            )
        }

        fn capture_auth_lifecycle_restore_snapshot(
            &self,
            lease_key: &LeaseKey,
        ) -> meerkat_core::handles::AuthLeaseRestoreSnapshot {
            self.handle
                .capture_auth_lifecycle_restore_snapshot(lease_key)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct MutableAuthLeaseHandle {
        handle: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
        publication_transition: Option<AuthLeaseTransition>,
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
            let lease_key = default_test_lease_key();
            let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
            let mut publication_transition = None;
            if snapshot.credential_present || snapshot.phase.is_some() {
                let mut transition = handle
                    .acquire_lease(&lease_key, snapshot.expires_at.unwrap_or(u64::MAX))
                    .expect("fixture AuthMachine accepts acquired lease");
                publication_transition = Some(transition.clone());
                while transition.generation() < snapshot.generation.max(1) {
                    handle
                        .begin_refresh(&lease_key)
                        .expect("fixture AuthMachine accepts refresh start");
                    transition = handle
                        .complete_refresh(
                            &lease_key,
                            snapshot.expires_at.unwrap_or(u64::MAX),
                            fixture_refresh_observation_time(
                                snapshot.expires_at.unwrap_or(u64::MAX),
                            ),
                        )
                        .expect("fixture AuthMachine accepts refresh completion");
                    publication_transition = Some(transition.clone());
                }
                match snapshot.phase {
                    Some(AuthLeasePhase::Expiring) => {
                        handle.mark_expiring(&lease_key).unwrap();
                    }
                    Some(AuthLeasePhase::Expired) => {
                        handle
                            .observe_credential_freshness(
                                &lease_key,
                                snapshot.expires_at.unwrap_or(0).saturating_add(1),
                                meerkat_core::handles::AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
                            )
                            .unwrap();
                    }
                    Some(AuthLeasePhase::Refreshing) => {
                        handle.begin_refresh(&lease_key).unwrap();
                    }
                    Some(AuthLeasePhase::ReauthRequired) => {
                        handle.mark_reauth_required(&lease_key).unwrap();
                    }
                    Some(AuthLeasePhase::Released) | None => {
                        handle.release_lease(&lease_key).unwrap();
                    }
                    Some(AuthLeasePhase::Valid) => {}
                }
            }
            Arc::new(Self {
                handle,
                publication_transition,
            })
        }

        fn generated(&self) -> GeneratedAuthLeaseHandle {
            generated_auth_lease_handle_for_test(Arc::clone(&self.handle))
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.handle.snapshot(_lease_key)
        }

        fn mark_tokens_lifecycle_published_for_test(
            &self,
            tokens: &PersistedTokens,
        ) -> PersistedTokens {
            mark_tokens_lifecycle_published_for_transition_for_test(
                tokens,
                self.publication_transition
                    .as_ref()
                    .expect("fixture has generated credential publication transition"),
            )
        }

        fn capture_auth_lifecycle_restore_snapshot(
            &self,
            lease_key: &LeaseKey,
        ) -> meerkat_core::handles::AuthLeaseRestoreSnapshot {
            self.handle
                .capture_auth_lifecycle_restore_snapshot(lease_key)
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
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid().generated());

        let secret = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap();

        assert_eq!(secret, "sk-managed");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_source_restores_marked_token_lifecycle_after_restart() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = PersistedTokens::api_key("sk-restored");
        let source_auth_lease = meerkat_runtime::RuntimeAuthLeaseHandle::new();
        let transition = source_auth_lease
            .acquire_lease(
                &lease_key,
                meerkat_core::persisted_token_expires_at_epoch_secs(&tokens),
            )
            .unwrap();
        let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &key,
            &tokens,
            &transition,
        )
        .unwrap();
        store.save(&key, &marked).await.unwrap();
        let auth_lease = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(generated_auth_lease_handle_for_test(Arc::clone(
                &auth_lease,
            )));

        let secret = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap();

        assert_eq!(secret, "sk-restored");
        let snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        assert!(snapshot.credential_present);
        assert_eq!(snapshot.generation, transition.generation());
        assert_eq!(
            snapshot.credential_published_at_millis,
            transition.credential_published_at_millis()
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_non_oauth_source_rejects_token_without_auth_lifecycle() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(&key, &PersistedTokens::api_key("sk-standalone"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing().with_token_store(store);

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
    async fn managed_store_non_oauth_source_rejects_empty_auth_lifecycle() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        store
            .save(&key, &PersistedTokens::api_key("sk-runtime"))
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(StaticAuthLeaseHandle::unknown().generated());

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
            .with_auth_lease_handle(StaticAuthLeaseHandle::released().generated());

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
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid().generated());

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
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid_generation(2).generated());

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
        let auth_lease = StaticAuthLeaseHandle::valid_generation_with_expiry(7, expires_at);
        let marked = auth_lease.mark_tokens_lifecycle_published_for_test(&tokens);
        store.save(&key, &marked).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());

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
            .with_auth_lease_handle(auth_lease.generated());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
        assert_eq!(auth_lease.snapshot(&default_test_lease_key()).phase, None);
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
            .with_auth_lease_handle(auth_lease.generated());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
        assert_eq!(auth_lease.snapshot(&default_test_lease_key()).phase, None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_empty_lifecycle_restores_valid_lifecycle_marker() {
        let store = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let tokens = chatgpt_oauth_tokens("fresh-runtime-access");
        let marked = mark_tokens_lifecycle_published_for_test(&tokens, 1);
        let published_at = Some(marker_credential_published_at_for_test(&marked));
        store.save(&key, &marked).await.unwrap();
        let auth_lease = MutableAuthLeaseHandle::unknown();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());

        let secret = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap();

        assert_eq!(secret, "fresh-runtime-access");
        let snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(
            snapshot.expires_at,
            Some(meerkat_core::persisted_token_expires_at_epoch_secs(&tokens))
        );
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
            .save(&key, &mark_tokens_lifecycle_published_for_test(&tokens, 1))
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
            .with_auth_lease_handle(auth_lease.generated());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::UserReauthRequired)
        ));
        let snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::ReauthRequired));
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
            .save(&key, &mark_tokens_lifecycle_published_for_test(&tokens, 1))
            .await
            .unwrap();
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Released),
            expires_at: None,
            credential_present: false,
            generation: 1,
            credential_published_at_millis: None,
        });
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::LeaseAbsent)
        ));
        assert_eq!(auth_lease.snapshot(&default_test_lease_key()).phase, None);
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
        let auth_lease = StaticAuthLeaseHandle::valid_generation_with_expiry(2, expires_at);
        store
            .save(
                &key,
                &auth_lease.mark_tokens_lifecycle_published_for_test(&tokens),
            )
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());

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
        let requested_previous_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(newer_expires_at - 3600),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        };
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(requested_previous_snapshot);
        let previous_snapshot = auth_lease.snapshot(&lease_key);
        store
            .save(
                &key,
                &mark_tokens_lifecycle_published_for_test(
                    &tokens,
                    previous_snapshot.generation + 1,
                ),
            )
            .await
            .unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());

        let err = resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::StaleCredential)
        ));
        assert_eq!(auth_lease.snapshot(&lease_key), previous_snapshot);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_compares_publication_time_for_equal_expiry() {
        let mut tokens = chatgpt_oauth_tokens("same-expiry-access");
        tokens.expires_at = Some(chrono::Utc::now() + chrono::Duration::hours(1));
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
        let older = mark_tokens_lifecycle_published_for_test(&tokens, 2);
        let matching = mark_tokens_lifecycle_published_after_time_for_test(
            &tokens,
            2,
            marker_credential_published_at_for_test(&older),
        );
        let matching_published_at = marker_credential_published_at_for_test(&matching);
        let newer =
            mark_tokens_lifecycle_published_after_time_for_test(&tokens, 2, matching_published_at);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(matching_published_at),
        };

        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &older,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::TokenStale
        );

        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &newer,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
        );

        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &matching,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_prefers_publication_time_over_expiry() {
        let snapshot_expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let mut older_longer = chatgpt_oauth_tokens("older-longer-access");
        older_longer.expires_at = Some(snapshot_expires_at + chrono::Duration::hours(1));
        let older_longer = mark_tokens_lifecycle_published_for_test(&older_longer, 2);
        let mut snapshot_anchor = chatgpt_oauth_tokens("snapshot-anchor-access");
        snapshot_anchor.expires_at = Some(snapshot_expires_at);
        let snapshot_anchor = mark_tokens_lifecycle_published_after_time_for_test(
            &snapshot_anchor,
            2,
            marker_credential_published_at_for_test(&older_longer),
        );
        let snapshot_published_at = marker_credential_published_at_for_test(&snapshot_anchor);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(snapshot_expires_at.timestamp().max(0) as u64),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(snapshot_published_at),
        };

        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &older_longer,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::TokenStale
        );

        let mut newer_shorter = chatgpt_oauth_tokens("newer-shorter-access");
        newer_shorter.expires_at = Some(snapshot_expires_at - chrono::Duration::hours(1));
        let newer_shorter = mark_tokens_lifecycle_published_after_time_for_test(
            &newer_shorter,
            2,
            snapshot_published_at,
        );
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &newer_shorter,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_rejects_equal_publication_time_expiry_drift() {
        let snapshot_expires_at = chrono::Utc::now() + chrono::Duration::hours(2);

        let mut same_time_longer = chatgpt_oauth_tokens("same-time-longer-access");
        same_time_longer.expires_at = Some(snapshot_expires_at + chrono::Duration::hours(1));
        let same_time_longer = mark_tokens_lifecycle_published_for_test(&same_time_longer, 2);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(snapshot_expires_at.timestamp().max(0) as u64),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(marker_credential_published_at_for_test(
                &same_time_longer,
            )),
        };
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &same_time_longer,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Invalid,
            "same-ms publication ties must not adopt a token as newer based on expiry drift"
        );

        let mut same_time_shorter = chatgpt_oauth_tokens("same-time-shorter-access");
        same_time_shorter.expires_at = Some(snapshot_expires_at - chrono::Duration::hours(1));
        let same_time_shorter = mark_tokens_lifecycle_published_for_test(&same_time_shorter, 2);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(snapshot_expires_at.timestamp().max(0) as u64),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(marker_credential_published_at_for_test(
                &same_time_shorter,
            )),
        };
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &same_time_shorter,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Invalid,
            "same-ms publication ties with expiry drift are inconsistent, not ordered"
        );

        let mut same_time_same_expiry_future_generation =
            chatgpt_oauth_tokens("same-time-same-expiry-future-generation-access");
        same_time_same_expiry_future_generation.expires_at = Some(snapshot_expires_at);
        let same_time_same_expiry_future_generation =
            mark_tokens_lifecycle_published_for_test(&same_time_same_expiry_future_generation, 3);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(snapshot_expires_at.timestamp().max(0) as u64),
            credential_present: true,
            generation: 2,
            credential_published_at_millis: Some(marker_credential_published_at_for_test(
                &same_time_same_expiry_future_generation,
            )),
        };
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &same_time_same_expiry_future_generation,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Invalid
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn oauth_lifecycle_marker_relation_rejects_same_time_same_expiry_older_generation() {
        let snapshot_expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let mut tokens = chatgpt_oauth_tokens("same-time-same-expiry-stale-generation-access");
        tokens.expires_at = Some(snapshot_expires_at);
        let stale_same_time = mark_tokens_lifecycle_published_for_test(&tokens, 2);
        let snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(snapshot_expires_at.timestamp().max(0) as u64),
            credential_present: true,
            generation: 3,
            credential_published_at_millis: Some(marker_credential_published_at_for_test(
                &stale_same_time,
            )),
        };

        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &stale_same_time,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Invalid,
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
        let marker = mark_tokens_lifecycle_published_for_test(&tokens, 1);

        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &marker,
                &snapshot,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Invalid,
            "equal finite expiry alone must not let an older marker match a newer AuthMachine snapshot"
        );
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
        let requested_initial_snapshot = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        };
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(requested_initial_snapshot);
        let initial_snapshot = auth_lease.snapshot(&lease_key);
        let previous_tokens = auth_lease.mark_tokens_lifecycle_published_for_test(&tokens);
        let current_tokens =
            mark_tokens_lifecycle_published_for_test(&tokens, initial_snapshot.generation + 1);
        store.save(&key, &current_tokens).await.unwrap();
        let previous = managed_store_tokens(
            Arc::clone(&store),
            key.clone(),
            previous_tokens,
            Some(initial_snapshot.clone()),
            Some(auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.generated());

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
        assert_eq!(snapshot, initial_snapshot);
        assert_eq!(
            store.load(&key).await.unwrap().unwrap(),
            current_tokens,
            "rejecting TokenNewer must not rewrite the token-store marker"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_oauth_refresh_commit_uses_authmachine_refresh_lifecycle() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());
        let raw_previous_tokens = chatgpt_oauth_tokens("expired-access");
        let previous_expires_at =
            meerkat_core::persisted_token_expires_at_epoch_secs(&raw_previous_tokens);
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(previous_expires_at),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        });
        let before_refresh = auth_lease.snapshot(&lease_key);
        let previous_tokens =
            auth_lease.mark_tokens_lifecycle_published_for_test(&raw_previous_tokens);
        store.save(&key, &previous_tokens).await.unwrap();
        let previous = ManagedStoreTokens {
            store: Arc::clone(&store),
            key: key.clone(),
            tokens: previous_tokens,
            lifecycle_snapshot: Some(auth_lease.snapshot(&lease_key)),
            lifecycle_restore_snapshot: Some(
                auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key),
            ),
            lifecycle: ManagedStoreLifecycle::RefreshRequired,
            lifecycle_guard: None,
        };
        let refreshed = chatgpt_oauth_tokens("refreshed-access");
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.generated());

        let committed =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &refreshed)
                .await
                .expect("refresh commit succeeds");

        assert_eq!(
            committed.primary_secret.as_deref(),
            Some("refreshed-access")
        );
        let after_refresh = auth_lease.snapshot(&lease_key);
        assert_eq!(after_refresh.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(after_refresh.generation, before_refresh.generation + 1);
        assert!(after_refresh.credential_present);
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
            Some(auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());

        let refresh_started =
            begin_managed_store_oauth_refresh_lifecycle(&env, &binding, &mut previous)
                .expect("existing refreshing lifecycle can be adopted");
        assert!(
            !refresh_started,
            "adopted refresh should not publish a second begin transition"
        );

        mark_managed_store_oauth_refresh_failed(
            &env,
            &binding,
            refresh_started,
            RefreshFailureObservation::local_credential_unusable(),
        )
        .expect("non-owner refresh failure should not mutate shared lifecycle");

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
            Some(auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());

        let refresh_started =
            begin_managed_store_oauth_refresh_lifecycle(&env, &binding, &mut previous)
                .expect("valid lifecycle can enter refreshing");
        assert!(
            refresh_started,
            "owned refresh should publish a begin transition"
        );
        mark_managed_store_oauth_refresh_failed(
            &env,
            &binding,
            refresh_started,
            RefreshFailureObservation::local_credential_unusable(),
        )
        .expect("owned permanent failure should publish AuthMachine failure");

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
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Expiring),
            expires_at: Some(meerkat_core::persisted_token_expires_at_epoch_secs(
                &expired_tokens,
            )),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        });
        let tokens = auth_lease.mark_tokens_lifecycle_published_for_test(&expired_tokens);
        store.save(&key, &tokens).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.generated());
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
        mark_managed_store_oauth_refresh_failed(
            &env,
            &binding,
            refresh_started,
            RefreshFailureObservation::transient(),
        )
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
            Some(auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());
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
                Err(crate::auth_store::RefreshError::Observed {
                    message: "token endpoint rejected refresh".to_string(),
                    observation: RefreshFailureObservation::oauth_token_endpoint(
                        400,
                        Some("invalid_grant".to_string()),
                    ),
                })
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
                if auth_lease.snapshot(&lease_key).phase == Some(AuthLeasePhase::ReauthRequired) {
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
            Some(auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());
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
            Some(auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key)),
            ManagedStoreLifecycle::RefreshRequired,
            None,
        );
        let env = ResolverEnvironment::testing()
            .with_token_store(store)
            .with_auth_lease_handle(auth_lease.generated());
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
        let raw_initial_tokens = chatgpt_oauth_tokens("expired-access");
        let auth_lease = StaticAuthLeaseHandle::valid_generation_with_expiry(
            2,
            meerkat_core::persisted_token_expires_at_epoch_secs(&raw_initial_tokens),
        );
        let initial_tokens =
            auth_lease.mark_tokens_lifecycle_published_for_test(&raw_initial_tokens);
        store.save(&key, &initial_tokens).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.generated());

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
                .with_auth_lease_handle(second_auth_lease.generated());
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
            lifecycle_restore_snapshot: Some(
                auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key),
            ),
            lifecycle: ManagedStoreLifecycle::RefreshRequired,
            lifecycle_guard: None,
        };

        let newer_tokens = chatgpt_oauth_tokens("newer-login-access");
        store.save(&key, &newer_tokens).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.generated());

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
        let raw_previous_tokens = chatgpt_oauth_tokens("expired-access");
        let auth_lease = StaticAuthLeaseHandle::valid_generation_with_expiry(
            1,
            meerkat_core::persisted_token_expires_at_epoch_secs(&raw_previous_tokens),
        );
        let previous_tokens =
            auth_lease.mark_tokens_lifecycle_published_for_test(&raw_previous_tokens);
        store.save(&key, &previous_tokens).await.unwrap();
        let previous = ManagedStoreTokens {
            store: Arc::clone(&store),
            key: key.clone(),
            tokens: previous_tokens,
            lifecycle_snapshot: Some(auth_lease.snapshot(&lease_key)),
            lifecycle_restore_snapshot: Some(
                auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key),
            ),
            lifecycle: ManagedStoreLifecycle::RefreshRequired,
            lifecycle_guard: None,
        };
        let refreshed = chatgpt_oauth_tokens("shared-refresh-access");
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.generated());

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
        let raw_previous_tokens = chatgpt_oauth_tokens("expired-access");
        let mut refreshed = chatgpt_oauth_tokens("shared-refresh-complete-access");
        refreshed.expires_at = Some(chrono::Utc::now() + chrono::Duration::hours(2));
        let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Refreshing),
            expires_at: Some(meerkat_core::persisted_token_expires_at_epoch_secs(
                &raw_previous_tokens,
            )),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(2_000),
        });
        let previous_tokens =
            auth_lease.mark_tokens_lifecycle_published_for_test(&raw_previous_tokens);
        let shared_tokens = mark_tokens_lifecycle_published_for_test(&refreshed, 2);
        store.save(&key, &previous_tokens).await.unwrap();
        let previous = ManagedStoreTokens {
            store: Arc::clone(&store),
            key: key.clone(),
            tokens: previous_tokens,
            lifecycle_snapshot: Some(auth_lease.snapshot(&lease_key)),
            lifecycle_restore_snapshot: Some(
                auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key),
            ),
            lifecycle: ManagedStoreLifecycle::RefreshRequired,
            lifecycle_guard: None,
        };
        store.save(&key, &shared_tokens).await.unwrap();
        let env = ResolverEnvironment::testing()
            .with_token_store(Arc::clone(&store))
            .with_auth_lease_handle(auth_lease.generated());

        let err =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &refreshed)
                .await
                .unwrap_err();

        assert!(
            err.to_string().contains("changed during OAuth refresh"),
            "got {err}"
        );
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
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid().generated());

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
