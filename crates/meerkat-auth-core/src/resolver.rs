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
use futures::future::BoxFuture;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::auth::{
    CredentialMutationError, CredentialMutationOutcome, PersistedAuthMode, PersistedTokens,
    ProviderAuthPersistence, RefreshCoordinator, RefreshError, RefreshFailureObservation, TokenKey,
    TokenStore,
};
#[cfg(all(not(target_arch = "wasm32"), test))]
use meerkat_core::auth::{CredentialMutationFn, RefreshFn};
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
use meerkat_llm_core::provider_runtime::runtime::CredentialReadiness;

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
                .ok_or_else(|| ProviderAuthError::ExternalResolverMissing(handle.to_string()))?;
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
            let tokens = resolve_command_credential_via_lease(env, binding, &runner).await?;
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

/// Observe a [`CredentialSourceSpec`]'s readiness for the api_key /
/// static_bearer auth methods, without materializing, refreshing, or
/// persisting it.
///
/// Mirrors [`resolve_simple_secret`] source by source, but only reads: the
/// process environment, the inline value, the managed token store row and the
/// AuthMachine's read-only credential-use classification. Sources observable
/// only by materializing them (an external resolver, a credential command)
/// report [`CredentialReadiness::MaterializedAtOpen`]. A host file descriptor
/// is [`CredentialReadiness::Missing`]: the simple-secret resolver always
/// rejects it, so the open could never use it.
pub async fn observe_simple_secret_readiness(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
) -> CredentialReadiness {
    match source {
        CredentialSourceSpec::InlineSecret { .. } => CredentialReadiness::Ready,
        CredentialSourceSpec::Env { env: var, fallback } => {
            let present = std::iter::once(var.as_str())
                .chain(fallback.iter().map(String::as_str))
                .any(|candidate| {
                    (!candidate.starts_with("RKAT_")
                        && (env.env_lookup)(&format!("RKAT_{candidate}")).is_some())
                        || (env.env_lookup)(candidate).is_some()
                });
            if present {
                CredentialReadiness::Ready
            } else {
                CredentialReadiness::Missing
            }
        }
        CredentialSourceSpec::ExternalResolver { handle } => {
            if env.external_resolvers.contains_key(handle) {
                CredentialReadiness::MaterializedAtOpen
            } else {
                CredentialReadiness::Missing
            }
        }
        CredentialSourceSpec::ManagedStore => observe_managed_store_readiness(env, binding).await,
        CredentialSourceSpec::Command { .. } => CredentialReadiness::MaterializedAtOpen,
        // `resolve_simple_secret` has no host-scoped reader and always fails
        // this source; readiness must not admit an open that cannot succeed.
        CredentialSourceSpec::FileDescriptor { .. } => CredentialReadiness::Missing,
        CredentialSourceSpec::PlatformDefault => CredentialReadiness::NeedsReauth,
    }
}

/// Read-only managed-store readiness: the token row and the AuthMachine's
/// read-only credential-use classification, never the lifecycle guard, a
/// lifecycle restore, a freshness observation, or a refresh.
///
/// Mirrors the verdicts [`resolve_managed_store_secret`] rejects: a lease the
/// machine classifies as needing a refresh or a re-login is not ready, since
/// the simple-secret open fails on it. A lease not yet registered keeps the
/// token row's answer, because the open restores it from the durable marker.
async fn observe_managed_store_readiness(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
) -> CredentialReadiness {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(store) = env
            .provider_auth_persistence()
            .map(ProviderAuthPersistence::token_store)
        else {
            return CredentialReadiness::NeedsReauth;
        };
        if let Some(auth_lease) = env.auth_lease_handle.as_ref() {
            let lease_key = meerkat_core::handles::LeaseKey::from_credential_identity(
                binding.credential_identity(),
            );
            match resolve_credential_use_admission(
                auth_lease,
                &lease_key,
                meerkat_core::handles::CredentialUseIntent::UseCredential,
            ) {
                Ok(
                    CredentialUseDisposition::Authorized | CredentialUseDisposition::LeaseAbsent,
                ) => {}
                Ok(
                    CredentialUseDisposition::RefreshRequired
                    | CredentialUseDisposition::RefreshDisallowed
                    | CredentialUseDisposition::ReauthRequired,
                ) => return CredentialReadiness::NeedsReauth,
                // Never emitted for `UseCredential`; the open rejects it too.
                Ok(CredentialUseDisposition::AlreadyRefreshing) | Err(_) => {
                    return CredentialReadiness::Missing;
                }
            }
        }
        let key = TokenKey::from_credential_identity(binding.credential_identity());
        match store.load(&key).await {
            Ok(Some(tokens))
                if require_persisted_auth_mode(&tokens, binding).is_ok()
                    && tokens.primary_secret.is_some() =>
            {
                CredentialReadiness::Ready
            }
            Ok(_) => CredentialReadiness::Missing,
            // An unreadable store is not a usable credential.
            Err(_) => CredentialReadiness::Missing,
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (env, binding);
        CredentialReadiness::Missing
    }
}

/// Resolve a command-source credential, routing the cached-vs-rerun freshness
/// verdict through the per-binding AuthMachine lease (row #47).
///
/// The subprocess execution stays in the [`CommandCredentialRunner`]; only the
/// freshness verdict moves to the lease. The runner's prior-run `last_refresh`
/// plus its configured `refresh_interval_ms` become the lease's expiry input,
/// and the AuthMachine `CredentialUseDisposition` — not a runner-local
/// `Instant` comparison — decides whether the cached token is reused or the
/// command is re-run. When no lease handle is present (standalone/ephemeral
/// surfaces) the credential is produced fresh on every resolve rather than
/// trusting a runner-local freshness clock.
#[cfg(not(target_arch = "wasm32"))]
async fn resolve_command_credential_via_lease(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    runner: &crate::auth_store::CommandCredentialRunner,
) -> Result<PersistedTokens, ProviderAuthError> {
    let Some(interval_ms) = runner.spec().refresh_interval_ms else {
        // No freshness window configured: there is nothing to cache, so the
        // command always runs. No lease decision is involved.
        return runner
            .run_and_cache()
            .await
            .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()));
    };
    let Some(auth_lease) = env.auth_lease_handle.as_ref() else {
        // Caching was requested but no AuthMachine lease owns the freshness
        // verdict here; fall through to a fresh run instead of trusting a
        // runner-local clock as the authority.
        return runner
            .run_and_cache()
            .await
            .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()));
    };

    let Some((cached, last_run)) = runner.cached() else {
        // No cached credential yet: run and cache the first credential.
        return runner
            .run_and_cache()
            .await
            .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()));
    };

    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
    let now = (env.now)();
    // Project the runner's cache age onto the lease as a synthetic expiry:
    // `last_run + refresh_interval`. Express it in epoch seconds relative to
    // `now` so the AuthMachine freshness observation can classify the cached
    // credential as fresh or due-for-rerun.
    let elapsed_ms = u64::try_from(last_run.elapsed().as_millis()).unwrap_or(u64::MAX);
    let remaining_secs = interval_ms.saturating_sub(elapsed_ms) / 1000;
    let synthetic_expiry = epoch_secs(now).saturating_add(remaining_secs);

    fn lifecycle_err(
        context: &str,
        error: meerkat_core::handles::DslTransitionError,
    ) -> ProviderAuthError {
        ProviderAuthError::SourceResolutionFailed(format!(
            "AuthMachine command-credential {context} failed: {error}"
        ))
    }
    // The handle is shared across bindings; reset this key's prior state, then
    // record the cached credential's synthetic expiry so the freshness verdict
    // reflects THIS cache entry.
    let _ = auth_lease.release_lease(&lease_key);
    auth_lease
        .acquire_lease(&lease_key, synthetic_expiry)
        .map_err(|e| lifecycle_err("acquire", e))?;
    auth_lease
        .observe_credential_freshness(
            &lease_key,
            epoch_secs(now),
            meerkat_core::handles::AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
        )
        .map_err(|e| lifecycle_err("freshness observation", e))?;
    match resolve_credential_use_admission(
        auth_lease,
        &lease_key,
        meerkat_core::handles::CredentialUseIntent::UseCredential,
    )? {
        // The machine authorizes the cached credential as fresh: reuse it.
        CredentialUseDisposition::Authorized => Ok(cached),
        // The machine asks for a refresh: re-run the command.
        CredentialUseDisposition::RefreshRequired => runner
            .run_and_cache()
            .await
            .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string())),
        // A command credential has no interactive reauth or silent-refresh
        // distinction; any other disposition fails closed by re-running the
        // command to obtain a fresh credential.
        CredentialUseDisposition::ReauthRequired
        | CredentialUseDisposition::RefreshDisallowed
        | CredentialUseDisposition::LeaseAbsent
        | CredentialUseDisposition::AlreadyRefreshing => runner
            .run_and_cache()
            .await
            .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string())),
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
impl ManagedStoreTokens {
    /// Release the process-local lifecycle guard before waiting for the shared
    /// credential coordinator.
    ///
    /// Every durable mutation acquires in the canonical order
    /// `coordinator/file lock -> lifecycle guard`. Holding the preload guard
    /// while waiting for the coordinator would invert that order against an
    /// interactive login/logout and can deadlock.
    pub fn release_prelock_lifecycle_guard(&mut self) {
        drop(self.lifecycle_guard.take());
    }
}

/// Result of revalidating a managed OAuth credential inside the refresh
/// coordinator's per-key transaction.
///
/// The provider runtime must not exchange a rotating refresh token until this
/// preparation has run: a different process may have committed a newer token
/// bundle while this resolver was waiting for the cross-process lock.
#[cfg(not(target_arch = "wasm32"))]
pub enum LockedManagedStoreOAuthRefresh {
    /// A different refresh owner already published a credential which the
    /// AuthMachine now admits for cached use. No provider exchange is needed.
    UseCached(PersistedTokens),
    /// This caller owns a refresh based on the durable predecessor captured
    /// while the coordinator lock is held.
    Refresh(PreparedManagedStoreOAuthRefresh),
}

/// How a coordinator-owned durable reload should be projected into the local
/// managed-store lifecycle.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedStoreOAuthRefreshPreparationMode {
    /// This refresh closure owns provider exchange when admission still
    /// requires it after the locked reload.
    RefreshOwner,
    /// This caller joined an in-process coalesced refresh whose owner already
    /// committed. Reproject the published durable result locally, but never
    /// start another provider exchange.
    AdoptPublished,
}

/// Provider callback that projects a coordinator-locked durable reload into a
/// managed-store refresh transaction.
#[cfg(not(target_arch = "wasm32"))]
pub type ManagedStoreOAuthRefreshPrepareFn = Box<
    dyn FnOnce(
            PersistedTokens,
            ManagedStoreOAuthRefreshPreparationMode,
        ) -> BoxFuture<'static, Result<LockedManagedStoreOAuthRefresh, RefreshError>>
        + Send
        + 'static,
>;

/// Single owner of prepare-claim and coalesced-result adoption semantics for
/// all managed OAuth providers.
///
/// A coordinator owner claims the callback inside its locked refresh closure.
/// An in-process waiter whose closure was coalesced re-enters the exclusive
/// per-key transaction and adopts the already-published durable result. Custom
/// coordinators used by embedders/tests may instead return an unmarked exchange
/// result without invoking the closure; that result is committed exactly once
/// against the locked predecessor for backwards-compatible coordinator
/// semantics.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct ManagedStoreOAuthRefreshPreparationSlot {
    prepare: Arc<std::sync::Mutex<Option<ManagedStoreOAuthRefreshPrepareFn>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ManagedStoreOAuthRefreshPreparationSlot {
    pub fn new(prepare: ManagedStoreOAuthRefreshPrepareFn) -> Self {
        Self {
            prepare: Arc::new(std::sync::Mutex::new(Some(prepare))),
        }
    }

    pub async fn claim_refresh_owner(
        &self,
        current: PersistedTokens,
    ) -> Result<LockedManagedStoreOAuthRefresh, RefreshError> {
        let prepare = self
            .prepare
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(|| {
                RefreshError::Refresh(
                    "managed OAuth refresh preparation was already claimed".into(),
                )
            })?;
        prepare(
            current,
            ManagedStoreOAuthRefreshPreparationMode::RefreshOwner,
        )
        .await
    }

    pub async fn finish_coordinated_refresh(
        &self,
        coordinator: Arc<dyn RefreshCoordinator>,
        token_store: Arc<dyn TokenStore>,
        key: TokenKey,
        refreshed: PersistedTokens,
    ) -> Result<PersistedTokens, RefreshError> {
        let unclaimed_prepare = self
            .prepare
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(prepare) = unclaimed_prepare else {
            return Ok(refreshed);
        };

        let returned = refreshed;
        let outcome = coordinator
            .with_exclusive_mutation(
                key.clone(),
                Box::new(move || {
                    Box::pin(async move {
                        let current = token_store
                            .load(&key)
                            .await
                            .map_err(|error| {
                                CredentialMutationError::TokenStore(error.to_string())
                            })?
                            .ok_or_else(|| {
                                CredentialMutationError::Operation(
                                    "persisted tokens disappeared before shared refresh adoption"
                                        .into(),
                                )
                            })?;
                        let returned_was_already_published = current == returned
                            || meerkat_core::tokens_lifecycle_published(&returned);
                        let mode = if returned_was_already_published {
                            ManagedStoreOAuthRefreshPreparationMode::AdoptPublished
                        } else {
                            ManagedStoreOAuthRefreshPreparationMode::RefreshOwner
                        };
                        match prepare(current, mode)
                            .await
                            .map_err(mutation_error_from_refresh)?
                        {
                            LockedManagedStoreOAuthRefresh::UseCached(cached) => {
                                Ok(CredentialMutationOutcome::Persisted(cached))
                            }
                            LockedManagedStoreOAuthRefresh::Refresh(transaction) => transaction
                                .commit(returned)
                                .await
                                .map(CredentialMutationOutcome::Persisted)
                                .map_err(mutation_error_from_refresh),
                        }
                    })
                }),
            )
            .await
            .map_err(refresh_error_from_mutation)?;
        match outcome {
            CredentialMutationOutcome::Persisted(tokens) => Ok(tokens),
            CredentialMutationOutcome::Cleared => Err(RefreshError::Refresh(
                "shared refresh adoption observed a cleared credential".into(),
            )),
        }
    }
}

/// Prepared managed-store refresh transaction.
///
/// `previous` contains both the token bytes and the opaque AuthMachine restore
/// snapshot observed under the coordinator lock. Commit and compensation must
/// stay on this object so neither can accidentally fall back to a pre-lock
/// resolver snapshot.
#[cfg(not(target_arch = "wasm32"))]
pub struct PreparedManagedStoreOAuthRefresh {
    env: ResolverEnvironment,
    binding: ValidatedBinding,
    previous: ManagedStoreTokens,
    refresh_started: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl PreparedManagedStoreOAuthRefresh {
    /// Commit under reacquired lifecycle custody. The coordinator remains held
    /// by the caller, including compensation for this transaction's own write.
    pub async fn commit(self, refreshed: PersistedTokens) -> Result<PersistedTokens, RefreshError> {
        publish_managed_store_tokens_lifecycle_and_save(
            &self.env,
            &self.binding,
            &self.previous,
            &refreshed,
        )
        .await
        .map_err(refresh_error_from_provider)
    }

    /// A provider failure can close only the exact lifecycle and durable
    /// predecessor captured before HTTP. A stale failure is not a new verdict.
    pub async fn fail(self, error: RefreshError) -> RefreshError {
        if matches!(&error, RefreshError::StalePreparation) {
            return error;
        }
        let lease_key = meerkat_core::handles::LeaseKey::from_credential_identity(
            self.binding.credential_identity(),
        );
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let Some(auth) = self.env.auth_lease_handle.as_ref() else {
            return RefreshError::Refresh("refresh authority is unavailable".into());
        };
        let stored = match self.previous.store.load(&self.previous.key).await {
            Ok(stored) => stored,
            Err(error) => return RefreshError::Refresh(error.to_string()),
        };
        if stored.as_ref() != Some(&self.previous.tokens)
            || self.previous.lifecycle_snapshot.as_ref() != Some(&auth.snapshot(&lease_key))
        {
            return RefreshError::StalePreparation;
        }
        match mark_managed_store_oauth_refresh_failed(
            &self.env,
            &self.binding,
            self.refresh_started,
            error.observation(),
        ) {
            Ok(()) => error,
            Err(lifecycle_error) => RefreshError::Refresh(format!("{error}; {lifecycle_error}")),
        }
    }
}

/// Preserve stale preparation across provider and coalesced-result boundaries.
#[cfg(not(target_arch = "wasm32"))]
pub fn refresh_error_from_provider(error: ProviderAuthError) -> RefreshError {
    match error {
        ProviderAuthError::Auth(AuthError::StaleCredential) => RefreshError::StalePreparation,
        other => RefreshError::Refresh(other.to_string()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn mutation_error_from_refresh(error: RefreshError) -> CredentialMutationError {
    match error {
        RefreshError::StalePreparation => CredentialMutationError::StalePreparation,
        other => CredentialMutationError::AuthLifecycle(other.to_string()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn refresh_error_from_mutation(error: CredentialMutationError) -> RefreshError {
    match error {
        CredentialMutationError::StalePreparation => RefreshError::StalePreparation,
        other => RefreshError::Refresh(other.to_string()),
    }
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

/// Machine-mirrored actionable outcome of the OAuth-login cached-vs-refresh
/// disposition. The provider runtime shell maps exactly one branch:
/// [`UseCached`](OAuthLoginCredentialAdmission::UseCached) uses the persisted
/// credential directly, [`BeginRefresh`](OAuthLoginCredentialAdmission::BeginRefresh)
/// enters [`prepare_managed_store_oauth_refresh_under_lock`], which begins the
/// lifecycle and provider exchange under shared mutation authority. The
/// refresh-disallowed / reauth / lease-absent dispositions are
/// surfaced as the matching [`ProviderAuthError`] rather than a variant, so the
/// provider never re-derives the use-vs-refresh-vs-error decision.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthLoginCredentialAdmission {
    /// Use the cached/persisted credential as-is.
    UseCached,
    /// Begin an OAuth refresh.
    BeginRefresh,
}

/// Route the OAuth-login cached-vs-refresh decision through the per-binding
/// AuthMachine's `ResolveOAuthLoginCredentialDisposition` classifier and mirror
/// the verdict. The provider runtime extracts only the pure observations it
/// holds — whether a persisted secret is present, whether the caller forced a
/// refresh, and whether the binding config permits silent refresh — and the
/// machine composes the full disposition. The shell decides nothing.
///
/// `Authorized` -> [`UseCached`](OAuthLoginCredentialAdmission::UseCached),
/// `RefreshRequired` -> [`BeginRefresh`](OAuthLoginCredentialAdmission::BeginRefresh),
/// `RefreshDisallowed` -> `Err(RefreshRequired)`, `ReauthRequired` -> reauth
/// error, `LeaseAbsent`/`AlreadyRefreshing` -> lease-absent error.
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve_oauth_login_credential_disposition(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    credential_present: bool,
) -> Result<OAuthLoginCredentialAdmission, ProviderAuthError> {
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(lease_absent_error)?;
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
    let facts = meerkat_core::handles::OAuthLoginCredentialFacts {
        credential_present,
        force_refresh: env.force_refresh,
        refresh_allowed: refresh_allowed(binding),
    };
    let disposition = auth_lease
        .resolve_oauth_login_credential_disposition(&lease_key, facts)
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine OAuth-login credential disposition classification failed: {e}"
            ))
        })?;
    match disposition {
        CredentialUseDisposition::Authorized => Ok(OAuthLoginCredentialAdmission::UseCached),
        CredentialUseDisposition::RefreshRequired => {
            Ok(OAuthLoginCredentialAdmission::BeginRefresh)
        }
        CredentialUseDisposition::RefreshDisallowed => Err(refresh_required_error()),
        CredentialUseDisposition::ReauthRequired => Err(user_reauth_required_error()),
        CredentialUseDisposition::LeaseAbsent | CredentialUseDisposition::AlreadyRefreshing => {
            Err(lease_absent_error())
        }
    }
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
    guard: Option<&meerkat_core::AuthLoginLifecycleGuard>,
) -> Result<(), ProviderAuthError> {
    let snapshot = auth_lease.snapshot(lease_key);
    if snapshot.phase.is_some()
        || snapshot.credential_present
        || snapshot.generation != 0
        || snapshot.credential_published_at_millis.is_some()
    {
        return Ok(());
    }
    let restored = if let Some(guard) = guard {
        meerkat_core::rehydrate_marked_tokens_for_status_for_identity_with_guard(
            store,
            auth_lease,
            binding.credential_identity(),
            expected_mode,
            now,
            guard,
        )
        .await
    } else {
        meerkat_core::rehydrate_marked_tokens_for_status_for_identity(
            store,
            auth_lease,
            binding.credential_identity(),
            expected_mode,
            now,
        )
        .await
    };
    restored.map(|_| ()).map_err(|error| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "AuthMachine lifecycle restore failed: {error}"
        ))
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum ManagedStorePurpose {
    FreshResolution,
    ExistingOwnerMaintenance,
}

/// Observe the existing generated credential owner while the caller holds its
/// exact normalized lifecycle guard. This performs no token-store or network I/O.
/// The returned snapshot is a comparison input, not reusable use authority.
#[cfg(not(target_arch = "wasm32"))]
pub fn observe_existing_managed_store_lifecycle_with_guard(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    guard: &meerkat_core::AuthLoginLifecycleGuard,
) -> Result<
    (
        meerkat_core::handles::AuthLeaseSnapshot,
        ManagedStoreLifecycle,
    ),
    ProviderAuthError,
> {
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
    if guard.lease_key() != &lease_key {
        return Err(stale_credential_error());
    }
    let auth = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(lease_absent_error)?;
    // Sample only after the caller has acquired this exact guard.
    observe_auth_lease_freshness_for_now(auth.as_ref(), &lease_key, (env.now)())?;
    let lifecycle = match resolve_credential_use_admission(
        auth,
        &lease_key,
        meerkat_core::handles::CredentialUseIntent::UseCredential,
    )? {
        CredentialUseDisposition::Authorized => ManagedStoreLifecycle::Authorized,
        CredentialUseDisposition::RefreshRequired => ManagedStoreLifecycle::RefreshRequired,
        CredentialUseDisposition::ReauthRequired => return Err(user_reauth_required_error()),
        CredentialUseDisposition::RefreshDisallowed => return Err(refresh_required_error()),
        CredentialUseDisposition::LeaseAbsent | CredentialUseDisposition::AlreadyRefreshing => {
            return Err(lease_absent_error());
        }
    };
    Ok((auth.snapshot(&lease_key), lifecycle))
}

/// Load managed material for fresh binding resolution, including the existing
/// marked-token restoration path for a new process-local owner.
#[cfg(not(target_arch = "wasm32"))]
pub async fn load_managed_store_tokens_with_lifecycle(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
) -> Result<ManagedStoreTokens, ProviderAuthError> {
    load_managed_store_tokens_for_purpose(env, binding, ManagedStorePurpose::FreshResolution).await
}

/// Load for a retained managed client. Missing/released owners are refused before
/// store I/O and may never be restored from durable token bytes by this route.
#[cfg(not(target_arch = "wasm32"))]
pub async fn load_existing_managed_store_tokens_with_lifecycle(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
) -> Result<ManagedStoreTokens, ProviderAuthError> {
    load_managed_store_tokens_for_purpose(
        env,
        binding,
        ManagedStorePurpose::ExistingOwnerMaintenance,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
async fn load_managed_store_tokens_for_purpose(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    purpose: ManagedStorePurpose,
) -> Result<ManagedStoreTokens, ProviderAuthError> {
    let store = env
        .provider_auth_persistence()
        .map(ProviderAuthPersistence::token_store)
        .ok_or_else(|| interactive_login_error(binding))?;
    let key = TokenKey::from_credential_identity(binding.credential_identity());
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
    let lifecycle_guard = if binding
        .auth()
        .persisted_auth_mode()
        .is_some_and(crate::auth_store::persisted_auth_mode_is_oauth_login)
    {
        Some(meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await)
    } else {
        None
    };
    if purpose == ManagedStorePurpose::ExistingOwnerMaintenance {
        observe_existing_managed_store_lifecycle_with_guard(
            env,
            binding,
            lifecycle_guard.as_ref().ok_or_else(lease_absent_error)?,
        )?;
    }
    let tokens = store
        .load(&key)
        .await
        .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?
        .ok_or_else(|| interactive_login_error(binding))?;
    let expected_mode = require_persisted_auth_mode(&tokens, binding)?;
    let is_oauth_login = crate::auth_store::persisted_auth_mode_is_oauth_login(expected_mode);
    if is_oauth_login && !durable_marker::marker_payload_valid_for_tokens(&tokens, &key) {
        return Err(stale_credential_error());
    }

    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(lease_absent_error)?;
    let now = (env.now)();
    if purpose == ManagedStorePurpose::FreshResolution {
        restore_marked_token_lifecycle_if_absent(
            auth_lease,
            store.as_ref(),
            binding,
            expected_mode,
            &lease_key,
            now,
            lifecycle_guard.as_ref(),
        )
        .await?;
    }
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
        // `RefreshDisallowed` is only emitted by the OAuth-login disposition, not
        // the `UseCredential` intent; fail closed if it ever surfaces here.
        CredentialUseDisposition::RefreshDisallowed => return Err(refresh_required_error()),
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
fn persisted_auth_mode_for_binding(
    binding: &ValidatedBinding,
) -> Result<PersistedAuthMode, ProviderAuthError> {
    binding.auth().persisted_auth_mode().ok_or_else(|| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "auth_method '{}' cannot resolve persisted credentials from TokenStore",
            binding.auth_profile().auth_method
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
fn begin_managed_store_oauth_refresh_lifecycle(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    previous: &mut ManagedStoreTokens,
) -> Result<bool, ProviderAuthError> {
    let auth_lease = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(lease_absent_error)?;
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
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
        return Err(stale_credential_error());
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
        // `RefreshDisallowed` is only emitted by the OAuth-login disposition, not
        // the `BeginRefresh` intent; fail closed if it ever surfaces here.
        CredentialUseDisposition::RefreshDisallowed => Err(refresh_required_error()),
        CredentialUseDisposition::LeaseAbsent | CredentialUseDisposition::Authorized => {
            Err(lease_absent_error())
        }
    }
}

/// Consult the actual generated owner for legacy refresh preparation only.
/// Callers must also verify the exact durable marker relation under lifecycle
/// custody. This is not permission to use an expired credential as a bearer.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn legacy_refresh_owner_allows_preparation(
    auth: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    lease: &meerkat_core::handles::LeaseKey,
) -> Result<bool, meerkat_core::handles::DslTransitionError> {
    use meerkat_core::handles::CredentialUseIntent;
    match auth.resolve_credential_use_admission(lease, CredentialUseIntent::HoldAuthority)? {
        CredentialUseDisposition::Authorized => Ok(true),
        CredentialUseDisposition::RefreshRequired => Ok(auth
            .resolve_credential_use_admission(lease, CredentialUseIntent::BeginRefresh)?
            == CredentialUseDisposition::RefreshRequired),
        _ => Ok(false),
    }
}

/// Normalize an interrupted durable Refreshing publication only while the
/// caller owns its existing refresh coordinator/file lock and this exact
/// lifecycle guard. This is not a general status or recovery policy.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn normalize_interrupted_refresh_under_coordinator(
    store: &dyn TokenStore,
    auth: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    key: &TokenKey,
    tokens: &PersistedTokens,
    guard: &meerkat_core::AuthLoginLifecycleGuard,
) -> Result<Option<PersistedTokens>, RefreshError> {
    let lease =
        meerkat_core::handles::LeaseKey::from_credential_identity(key.credential_identity());
    if guard.lease_key() != &lease {
        return Err(RefreshError::StalePreparation);
    }
    if meerkat_core::tokens_lifecycle_publication(tokens).and_then(|marker| marker.phase)
        != Some(AuthLeasePhase::Refreshing)
    {
        return Ok(None);
    }
    let snapshot = auth.snapshot(&lease);
    if durable_marker::marker_relation_for_tokens_and_snapshot(tokens, &snapshot, key)
        != durable_marker::AuthLeaseDurableMarkerRelation::Matches
    {
        return Err(RefreshError::StalePreparation);
    }
    // Preserve current reauthentication/absence while permitting an expired
    // credential that the generated owner admits for refresh preparation.
    if !legacy_refresh_owner_allows_preparation(auth, &lease)
        .map_err(|error| RefreshError::Refresh(error.to_string()))?
    {
        return Err(RefreshError::StalePreparation);
    }
    if snapshot.phase != Some(AuthLeasePhase::Refreshing) {
        let restored = meerkat_core::rehydrate_durable_predecessor_for_mutation_for_identity(
            store,
            auth,
            key.credential_identity(),
            chrono::Utc::now(),
            guard,
        )
        .await
        .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        if restored.as_ref() != Some(tokens) {
            return Err(RefreshError::StalePreparation);
        }
    }
    let closed = auth
        .refresh_failed(&lease, RefreshFailureObservation::transient())
        .map_err(|error| RefreshError::Refresh(error.to_string()))?;
    let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(key, tokens, &closed)
        .map_err(|error| RefreshError::Refresh(error.to_string()))?;
    store
        .save(key, &marked)
        .await
        .map_err(|error| RefreshError::Refresh(error.to_string()))?;
    Ok(Some(marked))
}

/// Establish the managed OAuth refresh baseline while the provider's
/// [`RefreshCoordinator`] transaction is held.
///
/// Resolver admission happens once before entering the coordinator to avoid a
/// lock for the common cached path, but that observation is only advisory. A
/// different process can publish newer rotating-token material before this
/// caller acquires the file lock. This function therefore reloads and verifies
/// the durable baseline, reprojects AuthMachine authority from its durable
/// marker when it changed, reruns cached-vs-refresh admission, and only then
/// begins a refresh. The returned transaction owns the under-lock token and
/// lifecycle restore snapshots used by both commit and rollback.
#[cfg(not(target_arch = "wasm32"))]
pub async fn prepare_managed_store_oauth_refresh_under_lock(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    previous: ManagedStoreTokens,
    locked_baseline: PersistedTokens,
    mode: ManagedStoreOAuthRefreshPreparationMode,
) -> Result<LockedManagedStoreOAuthRefresh, ProviderAuthError> {
    prepare_managed_store_oauth_refresh_for_purpose(
        env,
        binding,
        previous,
        locked_baseline,
        mode,
        ManagedStorePurpose::FreshResolution,
    )
    .await
}

/// Recheck retained-client maintenance under the existing coordinator and exact
/// lifecycle guard. A release during the wait may never become restoration.
#[cfg(not(target_arch = "wasm32"))]
pub async fn prepare_existing_managed_store_oauth_refresh_under_lock(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    previous: ManagedStoreTokens,
    locked_baseline: PersistedTokens,
    mode: ManagedStoreOAuthRefreshPreparationMode,
) -> Result<LockedManagedStoreOAuthRefresh, ProviderAuthError> {
    prepare_managed_store_oauth_refresh_for_purpose(
        env,
        binding,
        previous,
        locked_baseline,
        mode,
        ManagedStorePurpose::ExistingOwnerMaintenance,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
async fn prepare_managed_store_oauth_refresh_for_purpose(
    env: &ResolverEnvironment,
    binding: &ValidatedBinding,
    mut previous: ManagedStoreTokens,
    locked_baseline: PersistedTokens,
    mode: ManagedStoreOAuthRefreshPreparationMode,
    purpose: ManagedStorePurpose,
) -> Result<LockedManagedStoreOAuthRefresh, ProviderAuthError> {
    if previous.lifecycle_guard.is_some() {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "managed OAuth refresh entered coordinator while holding the pre-lock lifecycle guard"
                .into(),
        ));
    }
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
    previous.lifecycle_guard =
        Some(meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await);

    if purpose == ManagedStorePurpose::ExistingOwnerMaintenance {
        observe_existing_managed_store_lifecycle_with_guard(
            env,
            binding,
            previous
                .lifecycle_guard
                .as_ref()
                .ok_or_else(lease_absent_error)?,
        )?;
    }

    let mut durable_baseline = previous
        .store
        .load(&previous.key)
        .await
        .map_err(|error| ProviderAuthError::SourceResolutionFailed(error.to_string()))?
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "persisted tokens disappeared inside OAuth refresh transaction".into(),
            )
        })?;
    if durable_baseline != locked_baseline {
        return Err(stale_credential_error());
    }

    let expected_mode = require_persisted_auth_mode(&durable_baseline, binding)?;
    if !crate::auth_store::persisted_auth_mode_is_oauth_login(expected_mode) {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "managed_store refresh transaction requires an OAuth-login credential".into(),
        ));
    }
    if !durable_marker::marker_payload_valid_for_tokens(&durable_baseline, &previous.key) {
        return Err(stale_credential_error());
    }

    let mut owner_already_matches = false;
    if purpose == ManagedStorePurpose::ExistingOwnerMaintenance {
        // I/O may have waited. Observe again under retained custody before a
        // rebase, without treating a durable marker as current use permission.
        let (current, lifecycle) = observe_existing_managed_store_lifecycle_with_guard(
            env,
            binding,
            previous
                .lifecycle_guard
                .as_ref()
                .ok_or_else(lease_absent_error)?,
        )?;
        match durable_marker::marker_relation_for_tokens_and_snapshot(
            &durable_baseline,
            &current,
            &previous.key,
        ) {
            durable_marker::AuthLeaseDurableMarkerRelation::Matches => {
                owner_already_matches = true;
                let auth = env
                    .auth_lease_handle
                    .as_ref()
                    .ok_or_else(lease_absent_error)?;
                let captured = auth.capture_auth_lifecycle_restore_snapshot(&lease_key);
                previous.lifecycle_snapshot = Some(captured.snapshot().clone());
                previous.lifecycle_restore_snapshot = Some(captured);
                previous.lifecycle = lifecycle;
            }
            durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer => {
                // Only a changed locked row may advance the same credential
                // publication observed at preload. Never reset a replacement.
                let same_preload = previous.lifecycle_snapshot.as_ref().is_some_and(|preload| {
                    preload.credential_present == current.credential_present
                        && preload.generation == current.generation
                        && preload.expires_at == current.expires_at
                        && preload.credential_published_at_millis
                            == current.credential_published_at_millis
                });
                if durable_baseline == previous.tokens || !same_preload {
                    return Err(stale_credential_error());
                }
            }
            durable_marker::AuthLeaseDurableMarkerRelation::TokenStale
            | durable_marker::AuthLeaseDurableMarkerRelation::Invalid => {
                return Err(stale_credential_error());
            }
        }
    }

    // Legacy normalization may not use the ordinary changed-token rebase to
    // erase a newer or reauthentication owner before checking it.
    if meerkat_core::tokens_lifecycle_publication(&durable_baseline).and_then(|marker| marker.phase)
        == Some(AuthLeasePhase::Refreshing)
    {
        let auth = env
            .auth_lease_handle
            .as_ref()
            .ok_or_else(lease_absent_error)?;
        if durable_marker::marker_relation_for_tokens_and_snapshot(
            &durable_baseline,
            &auth.snapshot(&lease_key),
            &previous.key,
        ) != durable_marker::AuthLeaseDurableMarkerRelation::Matches
            || !legacy_refresh_owner_allows_preparation(auth, &lease_key)
                .map_err(|error| ProviderAuthError::SourceResolutionFailed(error.to_string()))?
        {
            return Err(stale_credential_error());
        }
    }

    if durable_baseline == previous.tokens || owner_already_matches {
        // Preserve byte-for-byte identity with the value loaded by the
        // coordinator-owned provider closure even when no rebase was needed.
        previous.tokens = durable_baseline.clone();
    } else {
        let auth_lease = env
            .auth_lease_handle
            .as_ref()
            .ok_or_else(lease_absent_error)?;
        // The process-local projection was restored from the pre-lock token.
        // Replace only its credential lifecycle side, then import the durable
        // marker that belongs to the locked predecessor through generated
        // AuthMachine authority.
        auth_lease
            .release_credential_lifecycle(&lease_key)
            .map_err(|error| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "AuthMachine lifecycle rebase release failed: {error}"
                ))
            })?;
        let restored = meerkat_core::rehydrate_marked_tokens_for_status_for_identity_with_guard(
            previous.store.as_ref(),
            auth_lease,
            binding.credential_identity(),
            expected_mode,
            (env.now)(),
            previous
                .lifecycle_guard
                .as_ref()
                .ok_or_else(lease_absent_error)?,
        )
        .await
        .map_err(|error| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle rebase failed: {error}"
            ))
        })?
        .ok_or_else(stale_credential_error)?;
        if restored != durable_baseline {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "AuthMachine lifecycle rebase did not restore the locked OAuth baseline".into(),
            ));
        }

        observe_auth_lease_freshness_for_now(auth_lease.as_ref(), &lease_key, (env.now)())?;
        let restore_snapshot = auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key);
        let snapshot = restore_snapshot.snapshot().clone();
        previous.tokens = durable_baseline.clone();
        previous.lifecycle_snapshot = Some(snapshot);
        previous.lifecycle_restore_snapshot = Some(restore_snapshot);
    }

    let auth = env
        .auth_lease_handle
        .as_ref()
        .ok_or_else(lease_absent_error)?;
    if let Some(normalized) = normalize_interrupted_refresh_under_coordinator(
        previous.store.as_ref(),
        auth,
        &previous.key,
        &durable_baseline,
        previous
            .lifecycle_guard
            .as_ref()
            .ok_or_else(lease_absent_error)?,
    )
    .await
    .map_err(|error| match error {
        RefreshError::StalePreparation => stale_credential_error(),
        other => ProviderAuthError::SourceResolutionFailed(other.to_string()),
    })? {
        durable_baseline = normalized;
        previous.tokens = durable_baseline.clone();
        let captured = auth.capture_auth_lifecycle_restore_snapshot(&lease_key);
        previous.lifecycle_snapshot = Some(captured.snapshot().clone());
        previous.lifecycle_restore_snapshot = Some(captured);
    }

    let admission = resolve_oauth_login_credential_disposition(
        env,
        binding,
        durable_baseline.primary_secret.is_some(),
    )?;
    if mode == ManagedStoreOAuthRefreshPreparationMode::AdoptPublished {
        return Ok(LockedManagedStoreOAuthRefresh::UseCached(durable_baseline));
    }

    match admission {
        OAuthLoginCredentialAdmission::UseCached => {
            Ok(LockedManagedStoreOAuthRefresh::UseCached(durable_baseline))
        }
        OAuthLoginCredentialAdmission::BeginRefresh => {
            let refresh_started =
                begin_managed_store_oauth_refresh_lifecycle(env, binding, &mut previous)?;
            previous.release_prelock_lifecycle_guard();
            Ok(LockedManagedStoreOAuthRefresh::Refresh(
                PreparedManagedStoreOAuthRefresh {
                    env: env.clone(),
                    binding: binding.clone(),
                    previous,
                    refresh_started,
                },
            ))
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
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
    if auth_lease.snapshot(&lease_key).phase
        != Some(meerkat_core::handles::AuthLeasePhase::Refreshing)
    {
        return Ok(());
    }
    auth_lease
        .refresh_failed(&lease_key, observation)
        .map(|_| ())
        .map_err(|e| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle refresh_failed failed: {e}"
            ))
        })
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
fn managed_store_oauth_refresh_failure_coordinator(
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
#[cfg(test)]
struct ManagedStoreOAuthRefreshPreClaimGuard {
    env: ResolverEnvironment,
    binding: ValidatedBinding,
    refresh_started: bool,
    active: std::sync::atomic::AtomicBool,
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
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
#[cfg(test)]
impl Drop for ManagedStoreOAuthRefreshPreClaimGuard {
    fn drop(&mut self) {
        let _ = self.fail_if_unclaimed();
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
struct ManagedStoreOAuthRefreshFailureCoordinator {
    inner: Arc<dyn RefreshCoordinator>,
    env: ResolverEnvironment,
    binding: ValidatedBinding,
    refresh_started: bool,
    pre_claim_guard: Arc<ManagedStoreOAuthRefreshPreClaimGuard>,
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
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
#[cfg(test)]
#[async_trait]
impl RefreshCoordinator for ManagedStoreOAuthRefreshFailureCoordinator {
    async fn with_exclusive_mutation(
        &self,
        key: TokenKey,
        mutation_fn: CredentialMutationFn,
    ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
        self.inner.with_exclusive_mutation(key, mutation_fn).await
    }

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
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
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
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
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
        // Preserve the existing exactly-published shared result control only
        // when it is the result this caller supplied and the owner matches it.
        if let Some(current) = current_tokens.as_ref()
            && persisted_token_material_matches(current, refreshed)
            && durable_marker::marker_relation_for_tokens_and_snapshot(
                current,
                &current_snapshot,
                &previous.key,
            ) == durable_marker::AuthLeaseDurableMarkerRelation::Matches
            && resolve_credential_use_admission(
                auth_lease,
                &lease_key,
                meerkat_core::handles::CredentialUseIntent::UseCredential,
            )? == CredentialUseDisposition::Authorized
        {
            return Ok(current.clone());
        }
        return Err(stale_credential_error());
    }
    if &current_snapshot != previous_snapshot {
        return Err(stale_credential_error());
    }

    let transition = match publish_managed_store_tokens_refresh_lifecycle(env, binding, refreshed) {
        Ok(transition) => transition,
        Err(error) => {
            // A rejected completion has not transferred authority. Close only
            // our unchanged captured refresh, never a newer owner or a prior
            // closure performed inside the publication helper.
            if &auth_lease.snapshot(&lease_key) == previous_snapshot
                && previous_snapshot.phase == Some(AuthLeasePhase::Refreshing)
                && let Err(closure) =
                    auth_lease.refresh_failed(&lease_key, RefreshFailureObservation::transient())
            {
                return Err(ProviderAuthError::SourceResolutionFailed(format!(
                    "{error}; refresh closure failed: {closure}"
                )));
            }
            return Err(error);
        }
    };
    let committed = match meerkat_core::mark_tokens_lifecycle_published_for_transition(
        &previous.key,
        refreshed,
        &transition,
    ) {
        Ok(committed) => match previous.store.save(&previous.key, &committed).await {
            Ok(()) => return Ok(committed),
            Err(error) => {
                format!("TokenStore save failed after AuthMachine lifecycle acquire: {error}")
            }
        },
        Err(error) => format!("refreshed credential marker failed: {error}"),
    };
    // This transaction has already changed the owner. Its own compensation
    // must not compare against the pre-write generation or call outer fail.
    let rollback = async {
        auth_lease
            .release_credential_lifecycle(&lease_key)
            .map_err(|error| format!("AuthMachine lifecycle rollback release failed: {error}"))?;
        let restored =
            meerkat_core::restore_token_lifecycle_snapshot(auth_lease, previous_restore_snapshot)
                .map_err(|error| format!("AuthMachine lifecycle rollback failed: {error}"))?
                .ok_or_else(|| {
                    "AuthMachine lifecycle rollback returned no credential".to_string()
                })?;
        let closed = if restored.phase() == AuthLeasePhase::Refreshing {
            auth_lease
                .refresh_failed(&lease_key, RefreshFailureObservation::transient())
                .map_err(|error| format!("AuthMachine rollback refresh closure failed: {error}"))?
        } else {
            restored
        };
        let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &previous.key,
            &previous.tokens,
            &closed,
        )
        .map_err(|error| format!("rollback marker failed: {error}"))?;
        previous
            .store
            .save(&previous.key, &marked)
            .await
            .map_err(|error| format!("TokenStore rollback save failed: {error}"))?;
        Ok::<(), String>(())
    }
    .await;
    let suffix = rollback
        .err()
        .map(|error| format!("; {error}"))
        .unwrap_or_default();
    Err(ProviderAuthError::SourceResolutionFailed(format!(
        "{committed}{suffix}"
    )))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn require_persisted_auth_mode(
    tokens: &PersistedTokens,
    binding: &ValidatedBinding,
) -> Result<PersistedAuthMode, ProviderAuthError> {
    let expected = persisted_auth_mode_for_binding(binding)?;
    if tokens.auth_mode != expected {
        return Err(persisted_auth_mode_mismatch(
            tokens,
            &binding.auth_profile().auth_method,
            expected,
        ));
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
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(binding.credential_identity());
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
        // `RefreshDisallowed` is only emitted by the OAuth-login disposition, not
        // the `HoldAuthority` intent; fail closed if it ever surfaces here.
        CredentialUseDisposition::RefreshDisallowed => Err(refresh_required_error()),
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
        .ok_or_else(|| ProviderAuthError::ExternalResolverMissing(handle.to_string()))?;
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
    use meerkat_core::auth::{PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore};
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
    fn test_provider_auth_persistence(store: Arc<dyn TokenStore>) -> ProviderAuthPersistence {
        ProviderAuthPersistence::new(store, Arc::new(crate::InMemoryCoordinator::new()))
    }

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
            server: None,
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
                origin: meerkat_core::connection::BindingOrigin::Configured,
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
            server: None,
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
                origin: meerkat_core::connection::BindingOrigin::Configured,
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
            origin: meerkat_core::connection::BindingOrigin::Configured,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn default_test_token_key() -> TokenKey {
        default_test_lease_key().to_token_key()
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

    /// Readiness observes every simple-secret source without materializing
    /// it: no resolver call, no command run, no store write.
    #[tokio::test]
    async fn simple_secret_readiness_observes_sources_without_materializing() {
        struct PanickingResolver;

        #[async_trait::async_trait]
        impl meerkat_llm_core::provider_runtime::registry::ExternalAuthResolverHandle
            for PanickingResolver
        {
            async fn resolve(
                &self,
                _binding: &ValidatedBinding,
            ) -> Result<ResolvedAuthEnvelope, AuthError> {
                panic!("readiness must never call an external resolver")
            }
        }

        let observe = |source: CredentialSourceSpec, env: ResolverEnvironment| async move {
            let binding = simple_secret_binding(source, "api_key");
            observe_simple_secret_readiness(&binding.auth_profile().source, &env, &binding).await
        };
        let env_with = |pairs: &'static [(&'static str, &'static str)]| {
            ResolverEnvironment::testing().with_env_lookup(move |name| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string())
            })
        };

        assert_eq!(
            observe(
                CredentialSourceSpec::InlineSecret {
                    secret: "sk-inline".into()
                },
                ResolverEnvironment::testing(),
            )
            .await,
            CredentialReadiness::Ready
        );
        let env_source = || CredentialSourceSpec::Env {
            env: "PRIMARY_KEY".into(),
            fallback: vec!["FALLBACK_KEY".into()],
        };
        assert_eq!(
            observe(env_source(), env_with(&[("FALLBACK_KEY", "sk")])).await,
            CredentialReadiness::Ready
        );
        assert_eq!(
            observe(env_source(), env_with(&[("RKAT_PRIMARY_KEY", "sk")])).await,
            CredentialReadiness::Ready
        );
        assert_eq!(
            observe(env_source(), env_with(&[])).await,
            CredentialReadiness::Missing
        );
        let external = || CredentialSourceSpec::ExternalResolver {
            handle: "host".into(),
        };
        assert_eq!(
            observe(
                external(),
                ResolverEnvironment::testing()
                    .with_external_resolver("host", Arc::new(PanickingResolver)),
            )
            .await,
            CredentialReadiness::MaterializedAtOpen
        );
        assert_eq!(
            observe(external(), ResolverEnvironment::testing()).await,
            CredentialReadiness::Missing
        );
        assert_eq!(
            observe(
                CredentialSourceSpec::Command {
                    program: "/definitely/not/run".into(),
                    args: Vec::new(),
                    cwd: None,
                    env: Default::default(),
                    timeout_ms: 1,
                    refresh_interval_ms: None,
                },
                ResolverEnvironment::testing(),
            )
            .await,
            CredentialReadiness::MaterializedAtOpen
        );
        assert_eq!(
            observe(
                CredentialSourceSpec::FileDescriptor {
                    fd: 3,
                    scope_override: None,
                },
                ResolverEnvironment::testing(),
            )
            .await,
            CredentialReadiness::Missing,
            "the simple-secret resolver always rejects a host file descriptor"
        );
        assert_eq!(
            observe(
                CredentialSourceSpec::PlatformDefault,
                ResolverEnvironment::testing()
            )
            .await,
            CredentialReadiness::NeedsReauth
        );
        assert_eq!(
            observe(
                CredentialSourceSpec::ManagedStore,
                ResolverEnvironment::testing()
            )
            .await,
            CredentialReadiness::NeedsReauth,
            "a managed store without persistence needs a login"
        );
    }

    /// A file-descriptor source is not ready, and the open agrees: the
    /// simple-secret resolver has no host-scoped reader for it.
    #[tokio::test]
    async fn file_descriptor_readiness_agrees_with_the_simple_secret_open() {
        let binding = simple_secret_binding(
            CredentialSourceSpec::FileDescriptor {
                fd: 3,
                scope_override: None,
            },
            "api_key",
        );
        let env = ResolverEnvironment::testing();
        let readiness =
            observe_simple_secret_readiness(&binding.auth_profile().source, &env, &binding).await;
        assert_eq!(readiness, CredentialReadiness::Missing);
        assert!(!readiness.admits_open());
        assert!(
            resolve_simple_secret(&binding.auth_profile().source, &env, &binding)
                .await
                .is_err(),
            "control: the open always fails a file-descriptor source"
        );
    }

    /// Managed-store readiness with persistence follows the AuthMachine's
    /// credential-use verdict, and agrees with the open in every lease phase:
    /// a valid lease with a stored secret is ready; a lease needing a refresh
    /// or a re-login is not, because the simple-secret open rejects both.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn managed_store_readiness_with_persistence_agrees_with_the_open() {
        async fn observe_then_open(
            phase: AuthLeasePhase,
        ) -> (CredentialReadiness, Result<String, ProviderAuthError>) {
            let store = Arc::new(EphemeralTokenStore::new());
            let binding = simple_secret_binding(CredentialSourceSpec::ManagedStore, "api_key");
            let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
            store
                .save(&key, &PersistedTokens::api_key("sk-managed-readiness"))
                .await
                .unwrap();
            let expires_at = match phase {
                AuthLeasePhase::Expired => 1_000,
                _ => u64::MAX,
            };
            let auth_lease = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
                phase: Some(phase),
                expires_at: Some(expires_at),
                credential_present: true,
                generation: 1,
                credential_published_at_millis: None,
            });
            let env = ResolverEnvironment::testing()
                .with_provider_auth_persistence(test_provider_auth_persistence(store))
                .with_auth_lease_handle(auth_lease.generated());
            let readiness =
                observe_simple_secret_readiness(&binding.auth_profile().source, &env, &binding)
                    .await;
            let open = resolve_simple_secret(&binding.auth_profile().source, &env, &binding).await;
            (readiness, open)
        }

        let (readiness, open) = observe_then_open(AuthLeasePhase::Valid).await;
        assert_eq!(readiness, CredentialReadiness::Ready);
        assert_eq!(open.expect("valid lease opens"), "sk-managed-readiness");

        let (readiness, open) = observe_then_open(AuthLeasePhase::ReauthRequired).await;
        assert_eq!(readiness, CredentialReadiness::NeedsReauth);
        assert!(matches!(
            open,
            Err(ProviderAuthError::Auth(AuthError::UserReauthRequired))
        ));

        let (readiness, open) = observe_then_open(AuthLeasePhase::Expired).await;
        assert_eq!(
            readiness,
            CredentialReadiness::NeedsReauth,
            "a lease the machine classifies as refresh-required is not ready"
        );
        assert!(matches!(
            open,
            Err(ProviderAuthError::Auth(AuthError::RefreshRequired))
        ));

        for phase in [
            AuthLeasePhase::Valid,
            AuthLeasePhase::Expiring,
            AuthLeasePhase::Expired,
            AuthLeasePhase::Refreshing,
            AuthLeasePhase::ReauthRequired,
        ] {
            let (readiness, open) = observe_then_open(phase).await;
            assert_eq!(
                readiness.admits_open(),
                open.is_ok(),
                "{phase:?}: readiness {readiness:?} must agree with the open {open:?}"
            );
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
    async fn command_credential_freshness_verdict_comes_from_auth_machine_lease() {
        // Row #47 gate: the cached-vs-rerun verdict for a command credential is
        // owned by the AuthMachine lease (`CredentialUseDisposition`), keyed by
        // the command binding's lease, not by a runner-local `Instant`
        // comparison. With a primed cache and a fresh valid lease, the lease
        // authorizes the cached credential and the subprocess is NOT re-run.
        use crate::auth_store::{CommandCredentialRunner, CommandCredentialSpec};

        let run_log = std::env::temp_dir().join(format!("rkat-cmd-{}", uuid::Uuid::new_v4()));
        // Each subprocess run appends a line and prints the running count as the
        // token, so a re-run is observable both by line count and by token text.
        let script = format!(
            "echo run >> '{path}'; wc -l < '{path}' | tr -d ' '",
            path = run_log.display()
        );
        let spec = CommandCredentialSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
            cwd: None,
            env: std::collections::HashMap::new(),
            timeout_ms: 5_000,
            // Large window so a runner-local clock would always reuse; the
            // lease, not the clock, must be the verdict owner.
            refresh_interval_ms: Some(3_600_000),
        };
        let runner = CommandCredentialRunner::new(spec);

        // Prime the cache (first real run).
        let first = runner.run_and_cache().await.unwrap();
        assert_eq!(first.primary_secret.as_deref(), Some("1"));

        let binding = simple_secret_binding(
            CredentialSourceSpec::Command {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "true".into()],
                cwd: None,
                env: std::collections::BTreeMap::new(),
                timeout_ms: 5_000,
                refresh_interval_ms: Some(3_600_000),
            },
            "api_key",
        );
        let env = ResolverEnvironment::testing()
            .with_auth_lease_handle(StaticAuthLeaseHandle::valid().generated());

        // Fresh valid lease authorizes the cached credential: reuse, no re-run.
        let reused = resolve_command_credential_via_lease(&env, &binding, &runner)
            .await
            .unwrap();
        assert_eq!(
            reused.primary_secret.as_deref(),
            Some("1"),
            "AuthMachine lease must authorize reuse of the cached command credential"
        );
        let line_count = std::fs::read_to_string(&run_log).unwrap().lines().count();
        assert_eq!(
            line_count, 1,
            "command must not be re-run when the lease authorizes the cached credential"
        );

        // With NO lease handle, caching has no owning authority, so the command
        // re-runs every resolve rather than trusting a runner-local clock.
        let env_no_lease = ResolverEnvironment::testing();
        let reran = resolve_command_credential_via_lease(&env_no_lease, &binding, &runner)
            .await
            .unwrap();
        assert_eq!(
            reran.primary_secret.as_deref(),
            Some("2"),
            "absent a lease authority, the command must re-run rather than honor a runner clock"
        );

        let _ = std::fs::remove_file(&run_log);
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
        let env = ResolverEnvironment::testing()
            .with_provider_auth_persistence(test_provider_auth_persistence(store));

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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
        let env = ResolverEnvironment::testing()
            .with_provider_auth_persistence(test_provider_auth_persistence(store));

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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
            .with_auth_lease_handle(auth_lease.generated());

        let err =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &tokens)
                .await
                .unwrap_err();

        assert!(
            matches!(&err, ProviderAuthError::Auth(AuthError::StaleCredential)),
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
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
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
    #[tokio::test]
    async fn account_alias_oauth_rebase_preserves_exact_credential_identity() {
        let template =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let identity =
            meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
                realm: template.auth_binding_ref().realm.clone(),
                account: meerkat_core::CredentialAccountId::parse("shared_oauth").unwrap(),
            });
        let lease_key = LeaseKey::from_credential_identity(&identity);
        let key = TokenKey::from_credential_identity(&identity);
        for alias in ["alias_one", "alias_two"] {
            let mut binding_ref = template.auth_binding_ref().clone();
            binding_ref.binding = meerkat_core::connection::BindingId::parse(alias).unwrap();
            let binding = ProviderRuntimeCatalog::validate_binding_with_credential_identity(
                &binding_ref,
                identity.clone(),
                template.backend_profile(),
                template.auth_profile(),
                template.policy(),
            )
            .expect("real catalog accepts a managed account binding");
            let alias_key = LeaseKey::from_auth_binding(&binding_ref);
            assert_ne!(lease_key, alias_key);
            let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
            let publisher = generated_auth_lease_handle_for_test(Arc::new(
                meerkat_runtime::RuntimeAuthLeaseHandle::new(),
            ));
            let raw_a = chatgpt_oauth_tokens("account-rebase-a");
            let acquired = meerkat_core::publish_token_lifecycle_acquired_for_identity(
                &publisher, &identity, &raw_a,
            )
            .unwrap();
            let marked_a = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &key, &raw_a, &acquired,
            )
            .unwrap();
            store.save(&key, &marked_a).await.unwrap();
            let local = generated_auth_lease_handle_for_test(Arc::new(
                meerkat_runtime::RuntimeAuthLeaseHandle::new(),
            ));
            meerkat_core::rehydrate_marked_tokens_for_status_for_identity(
                store.as_ref(),
                &local,
                &identity,
                PersistedAuthMode::ChatgptOauth,
                chrono::Utc::now(),
            )
            .await
            .unwrap()
            .expect("local owner starts with account A");
            let previous = managed_store_tokens(
                Arc::clone(&store),
                key.clone(),
                marked_a.clone(),
                Some(local.snapshot(&lease_key)),
                Some(local.capture_auth_lifecycle_restore_snapshot(&lease_key)),
                ManagedStoreLifecycle::RefreshRequired,
                None,
            );
            let raw_b = chatgpt_oauth_tokens("account-rebase-b");
            publisher.begin_refresh(&lease_key).unwrap();
            let refreshed = publisher
                .complete_refresh(
                    &lease_key,
                    meerkat_core::persisted_token_expires_at_epoch_secs(&raw_b),
                    chrono::Utc::now().timestamp().max(0) as u64,
                )
                .unwrap();
            let marked_b = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &key, &raw_b, &refreshed,
            )
            .unwrap();
            assert_ne!(marked_a, marked_b, "must execute the durable rebase branch");
            store.save(&key, &marked_b).await.unwrap();
            let env = ResolverEnvironment::testing()
                .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
                .with_auth_lease_handle(local.clone());
            let locked_store = Arc::clone(&store);
            let locked_key = key.clone();
            let refresh: RefreshFn = Box::new(move || {
                Box::pin(async move {
                    let locked = locked_store
                        .load(&locked_key)
                        .await
                        .map_err(|error| RefreshError::Refresh(error.to_string()))?
                        .ok_or_else(|| RefreshError::Refresh("fixture account absent".into()))?;
                    match prepare_managed_store_oauth_refresh_under_lock(
                        &env,
                        &binding,
                        previous,
                        locked,
                        ManagedStoreOAuthRefreshPreparationMode::AdoptPublished,
                    )
                    .await
                    .map_err(|error| RefreshError::Refresh(error.to_string()))?
                    {
                        LockedManagedStoreOAuthRefresh::UseCached(tokens) => Ok(tokens),
                        LockedManagedStoreOAuthRefresh::Refresh(_) => Err(RefreshError::Refresh(
                            "adopting a published account must not request a refresh".into(),
                        )),
                    }
                })
            });
            let coordinator = crate::InMemoryCoordinator::new();
            let adopted = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                coordinator.with_forced_refresh(key.clone(), refresh),
            )
            .await
            .expect("same-key coordinator and lifecycle guard do not recurse")
            .expect("account rebase must not validate the guard against its binding alias");
            assert_eq!(adopted, marked_b);
            assert_eq!(store.load(&key).await.unwrap(), Some(marked_b.clone()));
            assert_eq!(
                local.snapshot(&lease_key).phase,
                Some(AuthLeasePhase::Valid)
            );
            assert_eq!(
                durable_marker::marker_relation_for_tokens_and_snapshot(
                    &marked_b,
                    &local.snapshot(&lease_key),
                    &key,
                ),
                durable_marker::AuthLeaseDurableMarkerRelation::Matches,
            );
            assert_eq!(local.snapshot(&alias_key).phase, None);
            assert!(
                store
                    .load(&TokenKey::from_auth_binding(&binding_ref))
                    .await
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "file-lock"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forced_rotating_refreshes_rebase_token_and_lifecycle_baseline_under_file_lock() {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = TokenKey::from_auth_binding(binding.auth_binding_ref());
        let lease_key = LeaseKey::from_auth_binding(binding.auth_binding_ref());

        let mut raw_a = chatgpt_oauth_tokens("access-a");
        raw_a.refresh_token = Some("refresh-a".into());
        raw_a.expires_at = Some(chrono::Utc::now() - chrono::Duration::minutes(5));
        raw_a.last_refresh = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        let publisher = MutableAuthLeaseHandle::from_snapshot(AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Expired),
            expires_at: Some(meerkat_core::persisted_token_expires_at_epoch_secs(&raw_a)),
            credential_present: true,
            generation: 1,
            credential_published_at_millis: Some(1),
        });
        let marked_a = publisher.mark_tokens_lifecycle_published_for_test(&raw_a);
        store.save(&key, &marked_a).await.unwrap();

        // Model two processes which both loaded A before either obtained the
        // cross-process refresh lock. Each process has an independent
        // AuthMachine projection restored from the same durable marker.
        let make_preload = |auth_lease: GeneratedAuthLeaseHandle| {
            let snapshot = auth_lease.snapshot(&lease_key);
            ManagedStoreTokens {
                store: Arc::clone(&store),
                key: key.clone(),
                tokens: marked_a.clone(),
                lifecycle_snapshot: Some(snapshot),
                lifecycle_restore_snapshot: Some(
                    auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key),
                ),
                lifecycle: ManagedStoreLifecycle::RefreshRequired,
                lifecycle_guard: None,
            }
        };

        let runtime_one = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let auth_one = generated_auth_lease_handle_for_test(Arc::clone(&runtime_one));
        meerkat_core::rehydrate_marked_tokens_for_status(
            store.as_ref(),
            &auth_one,
            binding.auth_binding_ref(),
            PersistedAuthMode::ChatgptOauth,
            chrono::Utc::now(),
        )
        .await
        .unwrap()
        .expect("first process restores A");
        let preload_one = make_preload(auth_one.clone());

        let runtime_two = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let auth_two = generated_auth_lease_handle_for_test(Arc::clone(&runtime_two));
        meerkat_core::rehydrate_marked_tokens_for_status(
            store.as_ref(),
            &auth_two,
            binding.auth_binding_ref(),
            PersistedAuthMode::ChatgptOauth,
            chrono::Utc::now(),
        )
        .await
        .unwrap()
        .expect("second process restores A");
        let preload_two = make_preload(auth_two.clone());

        let env_one = ResolverEnvironment::testing()
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
            .with_auth_lease_handle(auth_one.clone())
            .with_force_refresh(true);
        let env_two = ResolverEnvironment::testing()
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
            .with_auth_lease_handle(auth_two.clone())
            .with_force_refresh(true);
        let lock_dir = tempfile::tempdir().unwrap();
        let coordinator_one: Arc<dyn RefreshCoordinator> =
            Arc::new(crate::FileLockCoordinator::new(lock_dir.path()));
        let coordinator_two: Arc<dyn RefreshCoordinator> =
            Arc::new(crate::FileLockCoordinator::new(lock_dir.path()));
        let start = Arc::new(tokio::sync::Barrier::new(3));
        let observed_baselines = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

        let spawn_refresh = |coordinator: Arc<dyn RefreshCoordinator>,
                             env: ResolverEnvironment,
                             binding: ValidatedBinding,
                             preload: ManagedStoreTokens| {
            let store = Arc::clone(&store);
            let key = key.clone();
            let start = Arc::clone(&start);
            let observed_baselines = Arc::clone(&observed_baselines);
            tokio::spawn(async move {
                start.wait().await;
                let refresh_key = key.clone();
                let refresh_fn: RefreshFn = Box::new(move || {
                    Box::pin(async move {
                        let baseline = store
                            .load(&refresh_key)
                            .await
                            .map_err(|error| RefreshError::Refresh(error.to_string()))?
                            .ok_or_else(|| {
                                RefreshError::Refresh("locked baseline disappeared".into())
                            })?;
                        observed_baselines
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(
                                baseline
                                    .refresh_token
                                    .clone()
                                    .expect("fixture baseline has rotating refresh token"),
                            );
                        let transaction = match prepare_managed_store_oauth_refresh_under_lock(
                            &env,
                            &binding,
                            preload,
                            baseline.clone(),
                            ManagedStoreOAuthRefreshPreparationMode::RefreshOwner,
                        )
                        .await
                        .map_err(|error| RefreshError::Refresh(error.to_string()))?
                        {
                            LockedManagedStoreOAuthRefresh::Refresh(transaction) => transaction,
                            LockedManagedStoreOAuthRefresh::UseCached(_) => {
                                return Err(RefreshError::Refresh(
                                    "forced refresh unexpectedly reused cached tokens".into(),
                                ));
                            }
                        };
                        let mut rotated = baseline;
                        match rotated.refresh_token.as_deref() {
                            Some("refresh-a") => {
                                rotated.primary_secret = Some("access-b".into());
                                rotated.refresh_token = Some("refresh-b".into());
                            }
                            Some("refresh-b") => {
                                rotated.primary_secret = Some("access-c".into());
                                rotated.refresh_token = Some("refresh-c".into());
                            }
                            other => {
                                return Err(transaction
                                    .fail(RefreshError::Refresh(format!(
                                        "unexpected rotating-token baseline: {other:?}"
                                    )))
                                    .await);
                            }
                        }
                        rotated.expires_at = Some(chrono::Utc::now() + chrono::Duration::hours(1));
                        rotated.last_refresh = Some(chrono::Utc::now());
                        transaction.commit(rotated).await
                    })
                });
                coordinator
                    .with_forced_refresh(key, refresh_fn)
                    .await
                    .expect("serialized forced refresh commits")
            })
        };

        let first = spawn_refresh(coordinator_one, env_one, binding.clone(), preload_one);
        let second = spawn_refresh(coordinator_two, env_two, binding, preload_two);
        start.wait().await;
        let first_result = first.await.unwrap();
        let second_result = second.await.unwrap();

        let baselines = observed_baselines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(baselines, vec!["refresh-a", "refresh-b"]);
        let result_secrets = [
            first_result.primary_secret.as_deref(),
            second_result.primary_secret.as_deref(),
        ];
        assert!(result_secrets.contains(&Some("access-b")));
        assert!(result_secrets.contains(&Some("access-c")));

        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("access-c"));
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-c"));
        assert!(durable_marker::marker_payload_valid_for_tokens(
            &stored, &key
        ));
        let final_snapshot_matches_an_owner = [&auth_one, &auth_two].into_iter().any(|handle| {
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &stored,
                &handle.snapshot(&lease_key),
                &key,
            ) == durable_marker::AuthLeaseDurableMarkerRelation::Matches
        });
        assert!(
            final_snapshot_matches_an_owner,
            "final C marker must match the AuthMachine that committed C"
        );
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
        async fn with_exclusive_mutation(
            &self,
            _key: TokenKey,
            _mutation_fn: CredentialMutationFn,
        ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
            Err(CredentialMutationError::Cancelled)
        }

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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
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
                .with_provider_auth_persistence(test_provider_auth_persistence(second_store))
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
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
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
            matches!(&err, ProviderAuthError::Auth(AuthError::StaleCredential)),
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
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
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
        // The same bytes no longer establish usability after the actual owner
        // requires reauthentication. Marker equality deliberately omits phase.
        let owner = env.auth_lease_handle.as_ref().unwrap();
        owner.mark_reauth_required(&lease_key).unwrap();
        let refused_owner = owner.snapshot(&lease_key);
        let rejected =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &refreshed)
                .await;
        assert!(matches!(
            rejected,
            Err(ProviderAuthError::Auth(AuthError::StaleCredential))
        ));
        assert_eq!(owner.snapshot(&lease_key), refused_owner);
        assert_eq!(store.load(&key).await.unwrap(), Some(first));
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
            .with_provider_auth_persistence(test_provider_auth_persistence(Arc::clone(&store)))
            .with_auth_lease_handle(auth_lease.generated());

        let err =
            publish_managed_store_tokens_lifecycle_and_save(&env, &binding, &previous, &refreshed)
                .await
                .unwrap_err();

        assert!(
            matches!(&err, ProviderAuthError::Auth(AuthError::StaleCredential)),
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
            .with_provider_auth_persistence(test_provider_auth_persistence(store))
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

    // C-e: real generated owner controls, not a mutable snapshot fake.
    #[cfg(not(target_arch = "wasm32"))]
    async fn ce_status_fixture() -> (
        Arc<dyn TokenStore>,
        GeneratedAuthLeaseHandle,
        PersistedTokens,
    ) {
        let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let auth = generated_auth_lease_handle_for_test(handle);
        let key = default_test_token_key();
        let lease = default_test_lease_key();
        let raw = chatgpt_oauth_tokens("ce-status-original");
        let transition = auth
            .acquire_lease(
                &lease,
                meerkat_core::persisted_token_expires_at_epoch_secs(&raw),
            )
            .unwrap();
        let marked =
            meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, &raw, &transition)
                .unwrap();
        store.save(&key, &marked).await.unwrap();
        (store, auth, marked)
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn ce_status_preserves_phase(phase: AuthLeasePhase) {
        let (store, auth, original) = ce_status_fixture().await;
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let lease = default_test_lease_key();
        match phase {
            AuthLeasePhase::Refreshing => auth.begin_refresh(&lease).unwrap(),
            AuthLeasePhase::ReauthRequired => auth.mark_reauth_required(&lease).unwrap(),
            _ => panic!("only the two live-phase controls are supported"),
        }
        let before = auth.snapshot(&lease);
        assert_eq!(before.phase, Some(phase));
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &original,
                &before,
                &default_test_token_key()
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches
        );
        let status = meerkat_core::rehydrate_marked_tokens_for_status(
            store.as_ref(),
            &auth,
            binding.auth_binding_ref(),
            PersistedAuthMode::ChatgptOauth,
            chrono::Utc::now(),
        )
        .await;
        let after = auth.snapshot(&lease);
        let disposition = auth
            .resolve_credential_use_admission(
                &lease,
                meerkat_core::handles::CredentialUseIntent::HoldAuthority,
            )
            .unwrap();
        assert!(status.is_ok(), "a matching publication remains readable");
        assert_eq!(
            after, before,
            "status must not replace an actual live phase with the durable Valid marker"
        );
        assert_eq!(
            store.load(&default_test_token_key()).await.unwrap(),
            Some(original)
        );
        let expected = if phase == AuthLeasePhase::Refreshing {
            meerkat_core::handles::CredentialUseDisposition::Authorized
        } else {
            meerkat_core::handles::CredentialUseDisposition::ReauthRequired
        };
        assert_eq!(
            disposition, expected,
            "the generated owner still decides use"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn ce_status_preserves_matching_refreshing() {
        ce_status_preserves_phase(AuthLeasePhase::Refreshing).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn ce_status_preserves_matching_reauth_required() {
        ce_status_preserves_phase(AuthLeasePhase::ReauthRequired).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn ce_status_absent_newer_and_unmarked_controls() {
        let (store, original_auth, original) = ce_status_fixture().await;
        let binding =
            simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
        let key = default_test_token_key();
        let lease = default_test_lease_key();
        let fresh = generated_auth_lease_handle_for_test(Arc::new(
            meerkat_runtime::RuntimeAuthLeaseHandle::new(),
        ));
        let absent = meerkat_core::rehydrate_marked_tokens_for_status(
            store.as_ref(),
            &fresh,
            binding.auth_binding_ref(),
            PersistedAuthMode::ChatgptOauth,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(
            absent,
            Some(original.clone()),
            "positive cold restore remains supported"
        );
        assert_eq!(fresh.snapshot(&lease), original_auth.snapshot(&lease));
        let newer_raw = chatgpt_oauth_tokens("ce-status-newer");
        let newer = mark_tokens_lifecycle_published_after_time_for_test(
            &newer_raw,
            2,
            marker_credential_published_at_for_test(&original),
        );
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &newer,
                &fresh.snapshot(&lease),
                &key
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
        );
        store.save(&key, &newer).await.unwrap();
        let restored = meerkat_core::rehydrate_marked_tokens_for_status(
            store.as_ref(),
            &fresh,
            binding.auth_binding_ref(),
            PersistedAuthMode::ChatgptOauth,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(
            restored,
            Some(newer.clone()),
            "positive newer durable publication is restored"
        );
        let before_unmarked = fresh.snapshot(&lease);
        let unmarked = chatgpt_oauth_tokens("ce-unmarked");
        store.save(&key, &unmarked).await.unwrap();
        let rejected = meerkat_core::rehydrate_marked_tokens_for_status(
            store.as_ref(),
            &fresh,
            binding.auth_binding_ref(),
            PersistedAuthMode::ChatgptOauth,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        assert!(
            rejected.is_none(),
            "unmarked bytes cannot restore a publication"
        );
        assert_eq!(fresh.snapshot(&lease), before_unmarked);
        // Nonmatching durable restoration keeps its existing contract. Stale
        // refresh-result behavior is tested at the actual HTTP transaction.
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn ce_coalesced_finish_preserves_typed_stale_preparation() {
        let store = Arc::new(crate::auth_store::EphemeralTokenStore::new());
        let key = default_test_token_key();
        let tokens = chatgpt_oauth_tokens("coalesced-stale");
        store.save(&key, &tokens).await.unwrap();
        let slot = ManagedStoreOAuthRefreshPreparationSlot::new(Box::new(|_, _| {
            Box::pin(async { Err(RefreshError::StalePreparation) })
        }));
        let result = slot
            .finish_coordinated_refresh(
                Arc::new(crate::auth_store::InMemoryCoordinator::new()),
                store.clone(),
                key.clone(),
                tokens.clone(),
            )
            .await;
        assert!(matches!(result, Err(RefreshError::StalePreparation)));
        assert_eq!(store.load(&key).await.unwrap(), Some(tokens));
    }
    #[cfg(not(target_arch = "wasm32"))]
    mod retained_maintenance {
        use super::*;

        async fn fixture(
            present: bool,
        ) -> (
            ResolverEnvironment,
            ValidatedBinding,
            Arc<dyn TokenStore>,
            GeneratedAuthLeaseHandle,
            PersistedTokens,
        ) {
            let binding =
                simple_secret_binding(CredentialSourceSpec::ManagedStore, "managed_chatgpt_oauth");
            let key = TokenKey::from_credential_identity(binding.credential_identity());
            let lease = LeaseKey::from_credential_identity(binding.credential_identity());
            let owner = generated_auth_lease_handle_for_test(Arc::new(
                meerkat_runtime::RuntimeAuthLeaseHandle::new(),
            ));
            let publisher = if present {
                owner.clone()
            } else {
                generated_auth_lease_handle_for_test(Arc::new(
                    meerkat_runtime::RuntimeAuthLeaseHandle::new(),
                ))
            };
            let tokens = chatgpt_oauth_tokens("maintenance-original");
            let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
            let acquired = publisher
                .acquire_lease(
                    &lease,
                    meerkat_core::persisted_token_expires_at_epoch_secs(&tokens),
                )
                .unwrap();
            let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &key, &tokens, &acquired,
            )
            .unwrap();
            let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
            store.save(&key, &marked).await.unwrap();
            drop(guard);
            let env = ResolverEnvironment::testing()
                .with_provider_auth_persistence(test_provider_auth_persistence(store.clone()))
                .with_auth_lease_handle(owner.clone());
            (env, binding, store, owner, marked)
        }

        #[tokio::test]
        async fn fresh_resolution_still_restores_an_absent_owner() {
            let (env, binding, store, owner, marked) = fixture(false).await;
            let lease = LeaseKey::from_credential_identity(binding.credential_identity());
            let key = TokenKey::from_credential_identity(binding.credential_identity());
            assert!(!owner.snapshot(&lease).credential_present);
            let loaded = load_managed_store_tokens_with_lifecycle(&env, &binding)
                .await
                .unwrap();
            assert_eq!(loaded.tokens, marked);
            assert_eq!(loaded.lifecycle, ManagedStoreLifecycle::Authorized);
            assert_eq!(owner.snapshot(&lease).phase, Some(AuthLeasePhase::Valid));
            drop(loaded);
            assert_eq!(store.load(&key).await.unwrap(), Some(marked));
        }
        #[tokio::test]
        async fn existing_maintenance_cannot_restore_absent_owner() {
            let (env, binding, store, owner, marked) = fixture(false).await;
            let lease = LeaseKey::from_credential_identity(binding.credential_identity());
            let key = TokenKey::from_credential_identity(binding.credential_identity());
            let before = owner.snapshot(&lease);
            let result = load_existing_managed_store_tokens_with_lifecycle(&env, &binding).await;
            assert!(matches!(
                result,
                Err(ProviderAuthError::Auth(AuthError::LeaseAbsent))
            ));
            assert_eq!(owner.snapshot(&lease), before);
            assert_eq!(store.load(&key).await.unwrap(), Some(marked));
        }

        #[tokio::test]
        async fn existing_maintenance_rechecks_release_before_locked_rebase() {
            for changed_row in [false, true] {
                let (env, binding, store, owner, original) = fixture(true).await;
                let lease = LeaseKey::from_credential_identity(binding.credential_identity());
                let key = TokenKey::from_credential_identity(binding.credential_identity());
                let mut previous =
                    load_existing_managed_store_tokens_with_lifecycle(&env, &binding)
                        .await
                        .unwrap();
                assert_eq!(previous.lifecycle, ManagedStoreLifecycle::Authorized);
                previous.release_prelock_lifecycle_guard();
                owner.release_lease(&lease).unwrap();
                let released = owner.snapshot(&lease);
                assert!(!released.credential_present);
                let durable = if changed_row {
                    let publisher = generated_auth_lease_handle_for_test(Arc::new(
                        meerkat_runtime::RuntimeAuthLeaseHandle::new(),
                    ));
                    let mut tokens = original.clone();
                    tokens.primary_secret = Some("maintenance-later-marked-row".into());
                    let expiry = meerkat_core::persisted_token_expires_at_epoch_secs(&tokens);
                    publisher.acquire_lease(&lease, expiry).unwrap();
                    publisher.begin_refresh(&lease).unwrap();
                    let transition = publisher
                        .complete_refresh(
                            &lease,
                            expiry,
                            chrono::Utc::now().timestamp().max(0) as u64,
                        )
                        .unwrap();
                    meerkat_core::mark_tokens_lifecycle_published_for_transition(
                        &key,
                        &tokens,
                        &transition,
                    )
                    .unwrap()
                } else {
                    original
                };
                store.save(&key, &durable).await.unwrap();
                let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let observed_in_refresh = observed.clone();
                let locked_store = store.clone();
                let locked_key = key.clone();
                let refresh: RefreshFn = Box::new(move || {
                    Box::pin(async move {
                        let locked = locked_store.load(&locked_key).await.unwrap().unwrap();
                        match prepare_existing_managed_store_oauth_refresh_under_lock(
                            &env,
                            &binding,
                            previous,
                            locked,
                            ManagedStoreOAuthRefreshPreparationMode::AdoptPublished,
                        )
                        .await
                        {
                            Err(error) => {
                                observed_in_refresh.store(
                                    matches!(
                                        &error,
                                        ProviderAuthError::Auth(AuthError::LeaseAbsent)
                                    ),
                                    std::sync::atomic::Ordering::SeqCst,
                                );
                                Err(refresh_error_from_provider(error))
                            }
                            Ok(_) => Err(RefreshError::Refresh("unexpected admission".into())),
                        }
                    })
                });
                let coordinator = crate::InMemoryCoordinator::new();
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    coordinator.with_forced_refresh(key.clone(), refresh),
                )
                .await
                .expect("actual coordinator and lease custody finish");
                assert!(result.is_err());
                assert!(
                    observed.load(std::sync::atomic::Ordering::SeqCst),
                    "changed_row={changed_row}: exact typed owner absence must reach the callback"
                );
                assert_eq!(owner.snapshot(&lease), released);
                assert_eq!(store.load(&key).await.unwrap(), Some(durable));
            }
        }

        struct LoadCountingStore {
            inner: Arc<dyn TokenStore>,
            loads: std::sync::atomic::AtomicUsize,
        }

        #[async_trait::async_trait]
        impl TokenStore for LoadCountingStore {
            async fn load(
                &self,
                key: &TokenKey,
            ) -> Result<Option<PersistedTokens>, meerkat_core::auth::TokenStoreError> {
                self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.inner.load(key).await
            }
            async fn save(
                &self,
                key: &TokenKey,
                tokens: &PersistedTokens,
            ) -> Result<(), meerkat_core::auth::TokenStoreError> {
                self.inner.save(key, tokens).await
            }
            async fn clear(
                &self,
                key: &TokenKey,
            ) -> Result<(), meerkat_core::auth::TokenStoreError> {
                self.inner.clear(key).await
            }
            async fn list(&self) -> Result<Vec<TokenKey>, meerkat_core::auth::TokenStoreError> {
                self.inner.list().await
            }
            fn backend_name(&self) -> &'static str {
                self.inner.backend_name()
            }
        }

        #[tokio::test]
        async fn absent_or_released_maintenance_preload_never_reads_the_store() {
            for released in [false, true] {
                let (env, binding, store, owner, marked) = fixture(released).await;
                let lease = LeaseKey::from_credential_identity(binding.credential_identity());
                let key = TokenKey::from_credential_identity(binding.credential_identity());
                if released {
                    owner.release_lease(&lease).unwrap();
                }
                let before = owner.snapshot(&lease);
                assert!(!before.credential_present);
                let counted = Arc::new(LoadCountingStore {
                    inner: store.clone(),
                    loads: std::sync::atomic::AtomicUsize::new(0),
                });
                let env = env.with_provider_auth_persistence(test_provider_auth_persistence(
                    counted.clone(),
                ));
                let result =
                    load_existing_managed_store_tokens_with_lifecycle(&env, &binding).await;
                assert!(
                    matches!(result, Err(ProviderAuthError::Auth(AuthError::LeaseAbsent))),
                    "released={released}: only exact current owner absence may refuse preload"
                );
                assert_eq!(
                    counted.loads.load(std::sync::atomic::Ordering::SeqCst),
                    0,
                    "released={released}: marked storage must not be read to resurrect an owner"
                );
                assert_eq!(owner.snapshot(&lease), before);
                assert_eq!(store.load(&key).await.unwrap(), Some(marked));
            }
        }

        async fn assert_locked_newer_publication(replace_owner: bool) {
            let (env, binding, store, owner, _) = fixture(true).await;
            let lease = LeaseKey::from_credential_identity(binding.credential_identity());
            let key = TokenKey::from_credential_identity(binding.credential_identity());
            assert_eq!(key, default_test_token_key());
            let mut previous = load_existing_managed_store_tokens_with_lifecycle(&env, &binding)
                .await
                .unwrap();
            assert_eq!(previous.lifecycle, ManagedStoreLifecycle::Authorized);
            let preload = owner.snapshot(&lease);
            previous.release_prelock_lifecycle_guard();
            if replace_owner {
                let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
                owner.release_lease_with_guard(&lease, &guard).unwrap();
                owner
                    .acquire_lease(&lease, preload.expires_at.unwrap())
                    .unwrap();
                drop(guard);
            }
            let before = owner.snapshot(&lease);
            assert!(before.credential_present);
            assert_eq!(before != preload, replace_owner);
            let next = chatgpt_oauth_tokens("maintenance-newer-locked-publication");
            // This helper uses actual generated Acquire/Begin/Complete outputs,
            // waiting only for their real publication clock to exceed `before`.
            let durable = mark_tokens_lifecycle_published_after_time_for_test(
                &next,
                before.generation + 1,
                before.credential_published_at_millis.unwrap(),
            );
            assert_eq!(
                durable_marker::marker_relation_for_tokens_and_snapshot(&durable, &before, &key),
                durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
            );
            store.save(&key, &durable).await.unwrap();
            let locked_store = store.clone();
            let locked_key = key.clone();
            let prepare_env = env.clone();
            let prepare_binding = binding.clone();
            let refresh: RefreshFn = Box::new(move || {
                Box::pin(async move {
                    let locked = locked_store.load(&locked_key).await.unwrap().unwrap();
                    match prepare_existing_managed_store_oauth_refresh_under_lock(
                        &prepare_env,
                        &prepare_binding,
                        previous,
                        locked,
                        ManagedStoreOAuthRefreshPreparationMode::AdoptPublished,
                    )
                    .await
                    .map_err(refresh_error_from_provider)?
                    {
                        LockedManagedStoreOAuthRefresh::UseCached(tokens) => Ok(tokens),
                        LockedManagedStoreOAuthRefresh::Refresh(_) => Err(RefreshError::Refresh(
                            "adoption unexpectedly started a provider exchange".into(),
                        )),
                    }
                })
            });
            let coordinator = crate::InMemoryCoordinator::new();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                coordinator.with_forced_refresh(key.clone(), refresh),
            )
            .await
            .expect("actual coordinator and generated owner finish");
            if replace_owner {
                assert!(
                    matches!(result, Err(RefreshError::StalePreparation)),
                    "a changed actual owner must not be reset from a later durable row"
                );
                assert_eq!(owner.snapshot(&lease), before);
            } else {
                assert_eq!(result.unwrap(), durable);
                let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
                let (after, lifecycle) =
                    observe_existing_managed_store_lifecycle_with_guard(&env, &binding, &guard)
                        .unwrap();
                assert_eq!(lifecycle, ManagedStoreLifecycle::Authorized);
                assert_eq!(after.phase, Some(AuthLeasePhase::Valid));
                assert!(after.generation > before.generation);
                assert_eq!(
                    durable_marker::marker_relation_for_tokens_and_snapshot(&durable, &after, &key),
                    durable_marker::AuthLeaseDurableMarkerRelation::Matches
                );
                drop(guard);
            }
            assert_eq!(store.load(&key).await.unwrap(), Some(durable));
        }

        #[tokio::test]
        async fn existing_maintenance_adopts_newer_locked_row_from_same_owner() {
            assert_locked_newer_publication(false).await;
        }

        #[tokio::test]
        async fn existing_maintenance_rejects_newer_locked_row_after_owner_replacement() {
            assert_locked_newer_publication(true).await;
        }
    }
}
