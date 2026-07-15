//! `auth/*` + `realm/*` method handlers.
//!
//! Real implementations using the shared `SessionRuntime.token_store()`.
//! OAuth login is split across two calls (runtime authority keeps state -> PKCE verifier):
//!
//!   auth/login/start     → returns authorize_url + state
//!   auth/login/complete  → verifies state, exchanges code → persists

use std::sync::Arc;

use serde_json::value::RawValue;

use meerkat_anthropic::runtime::oauth as a_oauth;
use meerkat_contracts::{
    BindingIdParams, CreateProfileParams, DeviceCompleteParams, DeviceStartParams,
    LoginCompleteParams, LoginStartParams, ProvisionApiKeyParams, RealmIdParams, WireAuthProfile,
    WireAuthStatusDetail, WireBackendProfile, WireBindingIdentity, WireDeviceCompleteResult,
    WireProviderBinding, WireProvisionApiKeyResult, WireRealmConnectionSet,
};
use meerkat_core::handles::LeaseKey;
use meerkat_core::{
    AuthBindingRef, ConnectionTargetError, CredentialSourceSpec, Provider, RealmConnectionSet,
    ResolvedConnectionTarget,
};
use meerkat_providers::auth_oauth::{
    DevicePollOutcome, OAuthError, PkcePair, exchange_authorization_code_with_state,
    poll_device_code, request_device_code,
};
use meerkat_providers::auth_store::{
    CredentialMutationError, CredentialMutationOutcome, PersistedAuthMode, PersistedTokens,
    RefreshCoordinator, TokenKey, TokenStore, credential_source_uses_persisted_store,
    persisted_auth_mode_is_oauth_login,
};
use meerkat_providers::oauth_flow::{
    OAuthDevicePollLease, OAuthFlowError, OAuthProviderIdentity, resolve_oauth_provider,
    validate_oauth_login_binding, validate_oauth_target_binding_for_auth_mode,
};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

/// Effective config the auth-resolution read path consumes.
///
/// When a per-realm config-document source is attached (decision 2/3), compose
/// the active realm's parent chain (workspace head ⊕ home-rooted `global`) into
/// a flat [`Config`] so an inherited (`global`-owned) binding is visible to
/// `resolve_oauth_target` and the strict-owner `resolve_write_owner` guard.
/// Composition is read-only; `config get/set` stay on the raw `config_runtime`
/// (the write path never composes — read/write split). When no source is
/// attached, fall back to the raw `config_runtime` snapshot (today's behavior).
async fn load_config(runtime: &SessionRuntime) -> Result<meerkat_core::Config, RpcResponse> {
    // The HEAD realm's config is the authoritative one from config_runtime (may
    // be in-memory). When a source is attached, compose the ancestor chain (the
    // `global` tail) OVER that head; composing purely from the source would drop
    // an in-memory head config.
    let head_config = if let Some(cfg_runtime) = runtime.config_runtime() {
        cfg_runtime
            .get()
            .await
            .map(|snap| snap.config)
            .map_err(|e| {
                RpcResponse::error(
                    None,
                    error::INTERNAL_ERROR,
                    format!("Failed to load config: {e}"),
                )
            })?
    } else {
        meerkat_core::Config::default()
    };
    if let Some(source) = runtime.realm_config_source() {
        let head = runtime
            .realm_id()
            .unwrap_or_else(meerkat_core::connection::RealmId::global);
        let reader = meerkat_core::EffectiveConfigReader::new(source);
        let mut config = reader
            .effective_config_over_head(&head, head_config)
            .await
            .map_err(|e| {
                RpcResponse::error(
                    None,
                    error::INTERNAL_ERROR,
                    format!("Failed to compose effective config: {e}"),
                )
            })?;
        config.apply_env_overrides().map_err(|e| {
            RpcResponse::error(
                None,
                error::INTERNAL_ERROR,
                format!("Failed to apply env overrides: {e}"),
            )
        })?;
        return Ok(config);
    }
    // No source attached: the raw head config is the effective config.
    Ok(head_config)
}

/// Resolve the typed [`NormalizedAuthMethod`] for a resolved [`AuthProfile`],
/// using the profile's typed `provider` to select the correct per-provider
/// matrix enum (provider-disambiguated parse-at-boundary).
///
/// Row 100: the auth-status / create surfaces hold a typed
/// `meerkat_core::AuthProfile` (which carries a typed `Provider` and the
/// declared `auth_method` string). Rather than re-deriving the persisted mode
/// through a provider-agnostic string shim that guesses the provider by trying
/// each matrix in order (now deleted), these surfaces parse the method against
/// the profile's known provider and then ask the typed enum directly via
/// [`NormalizedAuthMethod::persisted_auth_mode`] — the single owner of the
/// auth-method -> persisted-mode mapping. `None` when the declared method is
/// not a member of the provider's auth matrix.
fn normalized_auth_method(
    auth_profile: &meerkat_core::AuthProfile,
) -> Option<meerkat_providers::NormalizedAuthMethod> {
    meerkat_providers::NormalizedAuthMethod::from_auth_profile(auth_profile)
}

/// Resolve the persisted credential mode for a resolved auth profile through
/// the typed [`NormalizedAuthMethod`] owner (row 100). Returns `None` for
/// methods that hold no persisted secret (authorizer/ADC/SigV4-backed) or that
/// are not a member of the profile provider's auth matrix.
fn persisted_auth_mode_for_profile(
    auth_profile: &meerkat_core::AuthProfile,
) -> Option<PersistedAuthMode> {
    normalized_auth_method(auth_profile)
        .and_then(meerkat_providers::NormalizedAuthMethod::persisted_auth_mode)
}

async fn resolve_realm(
    runtime: &SessionRuntime,
    realm_id: &str,
) -> Result<RealmConnectionSet, RpcResponse> {
    let config = load_config(runtime).await?;
    let section = config.realm.get(realm_id).ok_or_else(|| {
        RpcResponse::error(
            None,
            error::INVALID_PARAMS,
            format!("Unknown realm: {realm_id}"),
        )
    })?;
    RealmConnectionSet::from_config(realm_id, section).map_err(|e| {
        RpcResponse::error(
            None,
            error::INTERNAL_ERROR,
            format!("Realm config invalid: {e}"),
        )
    })
}

async fn resolve_binding_identity(
    runtime: &SessionRuntime,
    realm_id: &str,
    binding_id: &str,
    profile_id: Option<&str>,
) -> Result<
    (
        AuthBindingRef,
        meerkat_core::ProviderBinding,
        meerkat_core::AuthProfile,
    ),
    RpcResponse,
> {
    let realm = resolve_realm(runtime, realm_id).await?;
    let resolved_realm_id = realm.realm_id.clone();
    let realm_typed = meerkat_core::connection::RealmId::parse(resolved_realm_id).map_err(|e| {
        RpcResponse::error(
            None,
            error::INVALID_PARAMS,
            format!("Invalid realm id {realm_id}: {e}"),
        )
    })?;
    let binding_typed = meerkat_core::connection::BindingId::parse(binding_id).map_err(|e| {
        RpcResponse::error(
            None,
            error::INVALID_PARAMS,
            format!("Invalid binding id {binding_id}: {e}"),
        )
    })?;
    let profile_typed = profile_id
        .map(meerkat_core::connection::ProfileId::parse)
        .transpose()
        .map_err(|e| {
            RpcResponse::error(
                None,
                error::INVALID_PARAMS,
                format!("Invalid profile id: {e}"),
            )
        })?;
    let auth_binding = AuthBindingRef {
        realm: realm_typed,
        binding: binding_typed,
        profile: profile_typed,
        origin: meerkat_core::connection::BindingOrigin::Configured,
    };
    match realm.lookup_auth_binding(&auth_binding) {
        Ok((binding, _, auth_profile)) => Ok((auth_binding, binding.clone(), auth_profile.clone())),
        Err(e) => {
            // Strict-owner write (decision 5) is owned by ONE core seam; this
            // surface only maps the typed verdict to an RPC error.
            let config = load_config(runtime).await?;
            let message = match meerkat_core::connection::resolve_write_owner(
                &config,
                &auth_binding.realm,
                &auth_binding.binding,
            ) {
                Err(inherited @ meerkat_core::connection::WriteOwnerError::Inherited { .. }) => {
                    inherited.to_string()
                }
                _ => format!("Unknown auth identity {realm_id}:{binding_id}: {e}"),
            };
            Err(RpcResponse::error(None, error::INVALID_PARAMS, message))
        }
    }
}

async fn resolve_oauth_target(
    runtime: &SessionRuntime,
    provider: Provider,
    realm_id: Option<&str>,
    binding_id: Option<&str>,
    profile_id: Option<&str>,
) -> Result<ResolvedConnectionTarget, RpcResponse> {
    let config = load_config(runtime).await?;
    let explicit_realm = realm_id
        .map(meerkat_core::RealmId::parse)
        .transpose()
        .map_err(|e| RpcResponse::error(None, error::INVALID_PARAMS, e.to_string()))?;
    let explicit_binding = binding_id
        .map(meerkat_core::BindingId::parse)
        .transpose()
        .map_err(|e| RpcResponse::error(None, error::INVALID_PARAMS, e.to_string()))?;
    let explicit_profile = profile_id
        .map(meerkat_core::ProfileId::parse)
        .transpose()
        .map_err(|e| RpcResponse::error(None, error::INVALID_PARAMS, e.to_string()))?;
    let runtime_realm = runtime.realm_id();
    meerkat_core::resolve_realm_binding_target_for_provider(
        &config,
        provider,
        explicit_realm.as_ref(),
        explicit_binding.as_ref(),
        explicit_profile.as_ref(),
        runtime_realm.as_ref(),
        false,
    )
    .map_err(target_error_response)
}

fn target_error_response(error: ConnectionTargetError) -> RpcResponse {
    let code = match error {
        ConnectionTargetError::RealmConfigInvalid { .. } => error::INTERNAL_ERROR,
        _ => error::INVALID_PARAMS,
    };
    RpcResponse::error(None, code, error.to_string())
}

/// Strict-owner write guard (decision 5) for OAuth login / device / provision
/// persistence.
///
/// Credential READS inherit down the realm chain, so `resolve_oauth_target`
/// happily resolves a binding that the requested realm only INHERITS from an
/// ancestor (e.g. the home-rooted `global` realm). Persisting against that
/// requested realm would either fork the credential into a child doc or write
/// it to a realm that does not own it. The single owner of "may this realm
/// write this binding?" is `meerkat_core::connection::resolve_write_owner`; this
/// surface only loads the composed (chain-folded) config and maps the typed
/// `Inherited` verdict to an `INVALID_PARAMS` error naming the owning realm
/// (mirrors [`resolve_binding_identity`]). `Ok` (head owns the binding) and the
/// other verdicts let the persist proceed unchanged.
#[allow(clippy::result_large_err)]
async fn guard_strict_owner_write(
    runtime: &SessionRuntime,
    id: &Option<RpcId>,
    realm_id: &str,
    binding_id: &str,
) -> Result<(), RpcResponse> {
    let realm = meerkat_core::connection::RealmId::parse(realm_id).map_err(|e| {
        RpcResponse::error(
            id.clone(),
            error::INVALID_PARAMS,
            format!("Invalid realm id {realm_id}: {e}"),
        )
    })?;
    let binding = meerkat_core::connection::BindingId::parse(binding_id).map_err(|e| {
        RpcResponse::error(
            id.clone(),
            error::INVALID_PARAMS,
            format!("Invalid binding id {binding_id}: {e}"),
        )
    })?;
    let config = load_config(runtime)
        .await
        .map_err(|r| r.with_id(id.clone()))?;
    match meerkat_core::connection::resolve_write_owner(&config, &realm, &binding) {
        Err(inherited @ meerkat_core::connection::WriteOwnerError::Inherited { .. }) => Err(
            RpcResponse::error(id.clone(), error::INVALID_PARAMS, inherited.to_string()),
        ),
        _ => Ok(()),
    }
}

#[allow(clippy::result_large_err)]
fn require_explicit_oauth_identity<'a>(
    id: Option<RpcId>,
    realm_id: Option<&'a str>,
    binding_id: Option<&'a str>,
) -> Result<(&'a str, &'a str), RpcResponse> {
    let realm_id = realm_id.ok_or_else(|| {
        RpcResponse::error(
            id.clone(),
            error::INVALID_PARAMS,
            "realm_id is required for OAuth login completion",
        )
    })?;
    let binding_id = binding_id.ok_or_else(|| {
        RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            "binding_id is required for OAuth login completion",
        )
    })?;
    Ok((realm_id, binding_id))
}

#[allow(clippy::result_large_err)]
fn require_token_store(
    runtime: &SessionRuntime,
    id: Option<RpcId>,
) -> Result<Arc<dyn meerkat_providers::auth_store::TokenStore>, RpcResponse> {
    runtime
        .token_store()
        .map_err(|err| {
            // K6: a token-store open failure is a typed fault, not an
            // absence of credentials.
            RpcResponse::error(
                id.clone(),
                error::INTERNAL_ERROR,
                format!("TokenStore unavailable: {err}"),
            )
        })?
        .ok_or_else(|| {
            RpcResponse::error(
                id.clone(),
                error::INTERNAL_ERROR,
                "TokenStore not configured for this runtime",
            )
        })
}

#[allow(clippy::result_large_err)]
fn require_provider_auth_persistence(
    runtime: &SessionRuntime,
    id: Option<RpcId>,
) -> Result<meerkat_providers::auth_store::ProviderAuthPersistence, RpcResponse> {
    runtime
        .provider_auth_persistence()
        .map_err(|error| {
            RpcResponse::error(
                id.clone(),
                error::INTERNAL_ERROR,
                format!("provider auth persistence unavailable: {error}"),
            )
        })?
        .ok_or_else(|| {
            RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                "provider auth persistence is not configured for this runtime",
            )
        })
}

fn require_managed_store_source(
    id: Option<RpcId>,
    binding_id: &str,
    auth_profile: &meerkat_core::AuthProfile,
) -> Option<RpcResponse> {
    if matches!(&auth_profile.source, CredentialSourceSpec::ManagedStore) {
        return None;
    }
    Some(RpcResponse::error(
        id,
        error::INVALID_PARAMS,
        format!(
            "binding {binding_id} resolves auth profile '{}' with source '{}'; \
             auth/profile/create can only persist credentials for source.kind = 'managed_store'",
            auth_profile.id,
            source_kind_label(&auth_profile.source),
        ),
    ))
}

fn source_kind_label(source: &CredentialSourceSpec) -> &'static str {
    match source {
        CredentialSourceSpec::InlineSecret { .. } => "inline_secret",
        CredentialSourceSpec::ManagedStore => "managed_store",
        CredentialSourceSpec::Env { .. } => "env",
        CredentialSourceSpec::ExternalResolver { .. } => "external_resolver",
        CredentialSourceSpec::PlatformDefault => "platform_default",
        CredentialSourceSpec::Command { .. } => "command",
        CredentialSourceSpec::FileDescriptor { .. } => "file_descriptor",
    }
}

fn oauth_device_state_error(id: Option<RpcId>, err: OAuthFlowError) -> RpcResponse {
    match err {
        OAuthFlowError::Missing => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            "oauth device code is missing or expired",
        ),
        OAuthFlowError::RegistryProjectionMissing { .. } => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("oauth device registry projection failed: {err}"),
        ),
        other => RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("oauth device state verification failed: {other}"),
        ),
    }
}

fn oauth_terminal_device_consume_error(id: Option<RpcId>, err: OAuthFlowError) -> RpcResponse {
    match err {
        OAuthFlowError::LifecycleRejected { .. } => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("oauth device terminal consume failed: {err}"),
        ),
        other => oauth_device_state_error(id, other),
    }
}

fn release_uncredentialed_terminal_oauth_lifecycle(
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) {
    let lease_key = LeaseKey::from_auth_binding(auth_binding);
    if !auth_lease.snapshot(&lease_key).credential_present {
        let _ = auth_lease.release_credential_lifecycle(&lease_key);
    }
}

fn consume_terminal_device_flow(
    id: Option<RpcId>,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    poll_lease: OAuthDevicePollLease,
) -> Option<RpcResponse> {
    match poll_lease.consume() {
        Ok(_) => None,
        Err(err) => {
            release_uncredentialed_terminal_oauth_lifecycle(auth_lease, auth_binding);
            Some(oauth_terminal_device_consume_error(id, err))
        }
    }
}

fn finish_device_flow_poll(
    id: Option<RpcId>,
    poll_lease: OAuthDevicePollLease,
) -> Option<RpcResponse> {
    match poll_lease.finish() {
        Ok(()) => None,
        Err(err) => Some(oauth_device_state_error(id, err)),
    }
}

fn verify_terminal_device_flow(
    id: Option<RpcId>,
    poll_lease: &OAuthDevicePollLease,
) -> Option<RpcResponse> {
    match poll_lease.verify() {
        Ok(_) => None,
        Err(err) => Some(oauth_device_state_error(id, err)),
    }
}

struct PreparedTokenCommitSnapshot {
    key: TokenKey,
    lease_key: LeaseKey,
    previous: Option<PersistedTokens>,
}

struct TokenCommitSnapshot {
    key: TokenKey,
    lease_key: LeaseKey,
    previous: Option<PersistedTokens>,
    previous_lifecycle: meerkat_core::handles::AuthLeaseSnapshot,
    previous_lifecycle_restore: meerkat_core::handles::AuthLeaseRestoreSnapshot,
    lifecycle_transition: meerkat_core::handles::AuthLeaseTransition,
}

async fn prepare_token_commit_unlocked(
    id: Option<RpcId>,
    store: &Arc<dyn TokenStore>,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) -> Result<PreparedTokenCommitSnapshot, RpcResponse> {
    let key = TokenKey::from_auth_binding(auth_binding);
    let previous = meerkat_core::rehydrate_durable_predecessor_for_mutation(
        store.as_ref(),
        auth_lease,
        auth_binding,
        chrono::Utc::now(),
    )
    .await
    .map_err(|error| {
        RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("durable credential predecessor rehydrate failed: {error}"),
        )
    })?;
    Ok(PreparedTokenCommitSnapshot {
        key,
        lease_key: LeaseKey::from_auth_binding(auth_binding),
        previous,
    })
}

/// Acquire-first token commit (the MCP OAuth shape, `meerkat-auth-core`
/// `publish_login_tokens_via_lease`): the AuthMachine lease acquisition is
/// recorded FIRST (an in-memory DSL transition, no durable I/O), the durable
/// lifecycle marker is stamped from that transition, and only then does the
/// single durable `TokenStore::save` run. No unmarked token bytes ever reach
/// the store — on a crash the durable record either carries the
/// proof-of-acquisition marker or does not exist.
async fn save_tokens_and_publish_lifecycle_commit_unlocked(
    id: Option<RpcId>,
    store: &Arc<dyn TokenStore>,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
) -> Result<TokenCommitSnapshot, RpcResponse> {
    let key = TokenKey::from_auth_binding(auth_binding);
    let lease_key = LeaseKey::from_auth_binding(auth_binding);
    let previous = meerkat_core::rehydrate_durable_predecessor_for_mutation(
        store.as_ref(),
        auth_lease,
        auth_binding,
        chrono::Utc::now(),
    )
    .await
    .map_err(|error| {
        RpcResponse::error(
            id.clone(),
            error::INTERNAL_ERROR,
            format!("durable credential predecessor rehydrate failed: {error}"),
        )
    })?;
    let previous_lifecycle_restore = auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key);
    let previous_lifecycle = previous_lifecycle_restore.snapshot().clone();
    // Acquire FIRST. A rejected acquisition mutates nothing — no token bytes
    // were persisted and the lease is unchanged — so the error propagates
    // without compensation.
    let transition =
        match meerkat_core::publish_token_lifecycle_acquired(auth_lease, auth_binding, tokens) {
            Ok(transition) => transition,
            Err(e) => {
                return Err(RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("AuthMachine lifecycle acquire failed: {e}"),
                ));
            }
        };
    let commit = TokenCommitSnapshot {
        key,
        lease_key,
        previous,
        previous_lifecycle,
        previous_lifecycle_restore,
        lifecycle_transition: transition,
    };
    save_marked_token_commit_unlocked(id, store.as_ref(), auth_lease, &commit, tokens, "").await?;
    Ok(commit)
}

/// Stamp the durable lifecycle marker from the already-acquired AuthMachine
/// transition and perform the single durable token write. If the marker
/// handoff or the durable save fails, the freshly acquired lease is rolled
/// back via [`rollback_token_commit`] (release + previous credential/lifecycle
/// restore) so no half-state survives.
async fn save_marked_token_commit_unlocked(
    id: Option<RpcId>,
    store: &dyn TokenStore,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    commit: &TokenCommitSnapshot,
    tokens: &PersistedTokens,
    failure_context: &str,
) -> Result<(), RpcResponse> {
    let committed_tokens = match meerkat_core::mark_tokens_lifecycle_published_for_transition(
        &commit.key,
        tokens,
        &commit.lifecycle_transition,
    ) {
        Ok(committed_tokens) => committed_tokens,
        Err(e) => {
            let message = match rollback_token_commit(store, auth_lease, commit).await {
                Ok(()) => format!(
                    "AuthMachine lifecycle marker handoff failed{failure_context}: {e}; acquired lease rolled back"
                ),
                Err(rollback_error) => format!(
                    "AuthMachine lifecycle marker handoff failed{failure_context}: {e}; acquired lease rollback failed: {rollback_error}"
                ),
            };
            return Err(RpcResponse::error(id, error::INTERNAL_ERROR, message));
        }
    };
    if let Err(e) = store.save(&commit.key, &committed_tokens).await {
        let message = match rollback_token_commit(store, auth_lease, commit).await {
            Ok(()) => {
                format!("TokenStore save failed{failure_context}: {e}; acquired lease rolled back")
            }
            Err(rollback_error) => {
                format!(
                    "TokenStore save failed{failure_context}: {e}; acquired lease rollback failed: {rollback_error}"
                )
            }
        };
        return Err(RpcResponse::error(id, error::INTERNAL_ERROR, message));
    }
    Ok(())
}

async fn save_tokens_and_publish_lifecycle(
    id: Option<RpcId>,
    runtime: &SessionRuntime,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
) -> Result<(), RpcResponse> {
    let persistence = require_provider_auth_persistence(runtime, id.clone())?;
    let refresh_coordinator = persistence.refresh_coordinator();
    let auth_lease = runtime.generated_auth_lease_handle();
    let mutation_store = persistence.token_store();
    let mutation_binding = auth_binding.clone();
    let mutation_tokens = tokens.clone();
    let key = TokenKey::from_auth_binding(auth_binding);
    let mutation_id = id.clone();
    let outcome = refresh_coordinator
        .with_exclusive_mutation(
            key,
            Box::new(move || {
                Box::pin(async move {
                    let lease_key = LeaseKey::from_auth_binding(&mutation_binding);
                    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
                    let commit = save_tokens_and_publish_lifecycle_commit_unlocked(
                        mutation_id,
                        &mutation_store,
                        &auth_lease,
                        &mutation_binding,
                        &mutation_tokens,
                    )
                    .await
                    .map_err(rpc_response_to_credential_mutation_error)?;
                    let committed = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                        &commit.key,
                        &mutation_tokens,
                        &commit.lifecycle_transition,
                    )
                    .map_err(|error| CredentialMutationError::AuthLifecycle(error.to_string()))?;
                    Ok(CredentialMutationOutcome::Persisted(committed))
                })
            }),
        )
        .await
        .map_err(|error| {
            RpcResponse::error(id.clone(), error::INTERNAL_ERROR, error.to_string())
        })?;
    match outcome {
        CredentialMutationOutcome::Persisted(_) => Ok(()),
        CredentialMutationOutcome::Cleared => Err(RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "credential save transaction returned cleared outcome",
        )),
    }
}

fn rpc_response_to_credential_mutation_error(response: RpcResponse) -> CredentialMutationError {
    CredentialMutationError::Operation(
        response
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| "credential mutation failed without a typed RPC error".to_string()),
    )
}

/// Release the lease acquired by an acquire-first token commit and restore the
/// previous credential + lifecycle. With acquire-first ordering the durable
/// store still holds the previous bytes when this runs, so the durable writes
/// here re-assert the previous record and re-stamp its marker from the
/// restored transition. The lease release always runs first: even if the
/// durable restore fails, no freshly acquired lease survives a failed commit.
async fn rollback_token_commit(
    store: &dyn TokenStore,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    commit: &TokenCommitSnapshot,
) -> Result<(), String> {
    match &commit.previous {
        Some(previous) => match commit.previous_lifecycle.phase {
            Some(phase) if phase != meerkat_core::handles::AuthLeasePhase::Released => {
                auth_lease
                    .release_credential_lifecycle(&commit.lease_key)
                    .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
                store
                    .save(&commit.key, previous)
                    .await
                    .map_err(|e| format!("TokenStore rollback save failed: {e}"))?;
                let restored_transition = meerkat_core::restore_token_lifecycle_snapshot(
                    auth_lease,
                    &commit.previous_lifecycle_restore,
                )
                .map_err(|e| format!("AuthMachine lifecycle rollback failed: {e}"))?;
                if let Some(restored_transition) = restored_transition {
                    let restored_previous =
                        meerkat_core::mark_tokens_lifecycle_published_for_transition(
                            &commit.key,
                            previous,
                            &restored_transition,
                        )
                        .map_err(|e| format!("AuthMachine rollback marker handoff failed: {e}"))?;
                    store
                        .save(&commit.key, &restored_previous)
                        .await
                        .map_err(|e| format!("TokenStore rollback marker save failed: {e}"))?;
                }
            }
            _ => {
                auth_lease
                    .release_credential_lifecycle(&commit.lease_key)
                    .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
                store
                    .save(&commit.key, previous)
                    .await
                    .map_err(|e| format!("TokenStore rollback save failed: {e}"))?;
            }
        },
        None => {
            auth_lease
                .release_credential_lifecycle(&commit.lease_key)
                .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
            store
                .clear(&commit.key)
                .await
                .map_err(|e| format!("TokenStore rollback clear failed: {e}"))?;
        }
    }
    Ok(())
}

async fn save_prepared_tokens_after_terminal_consume_unlocked(
    id: Option<RpcId>,
    store: &Arc<dyn TokenStore>,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    prepared: PreparedTokenCommitSnapshot,
) -> Result<(), RpcResponse> {
    let previous_lifecycle_restore =
        auth_lease.capture_auth_lifecycle_restore_snapshot(&prepared.lease_key);
    let previous_lifecycle = previous_lifecycle_restore.snapshot().clone();
    let transition =
        match meerkat_core::publish_token_lifecycle_acquired(auth_lease, auth_binding, tokens) {
            Ok(transition) => transition,
            Err(e) => {
                return Err(RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("AuthMachine lifecycle acquire failed after OAuth consume: {e}"),
                ));
            }
        };
    let commit = TokenCommitSnapshot {
        key: prepared.key,
        lease_key: prepared.lease_key,
        previous: prepared.previous,
        previous_lifecycle,
        previous_lifecycle_restore,
        lifecycle_transition: transition,
    };
    save_marked_token_commit_unlocked(
        id,
        store.as_ref(),
        auth_lease,
        &commit,
        tokens,
        " after OAuth consume",
    )
    .await
}

async fn save_tokens_and_consume_device_flow_unlocked(
    id: Option<RpcId>,
    store: &Arc<dyn TokenStore>,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    poll_lease: OAuthDevicePollLease,
) -> Option<RpcResponse> {
    if !poll_lease.terminal_flow_state_is_authmachine_owned() {
        return Some(RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "consume_oauth_device_flow requires an AuthMachine-owned OAuth device poll lease",
        ));
    }
    if let Some(resp) = verify_terminal_device_flow(id.clone(), &poll_lease) {
        return Some(resp);
    }
    let prepared =
        match prepare_token_commit_unlocked(id.clone(), store, auth_lease, auth_binding).await {
            Ok(prepared) => prepared,
            Err(resp) => return Some(resp),
        };
    if let Some(resp) =
        consume_terminal_device_flow(id.clone(), auth_lease, auth_binding, poll_lease)
    {
        return Some(resp);
    }
    save_prepared_tokens_after_terminal_consume_unlocked(
        id,
        store,
        auth_lease,
        auth_binding,
        tokens,
        prepared,
    )
    .await
    .err()
}

async fn save_tokens_and_consume_device_flow(
    id: Option<RpcId>,
    runtime: &SessionRuntime,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    poll_lease: OAuthDevicePollLease,
) -> Option<RpcResponse> {
    let persistence = match require_provider_auth_persistence(runtime, id.clone()) {
        Ok(persistence) => persistence,
        Err(response) => return Some(response),
    };
    let refresh_coordinator = persistence.refresh_coordinator();
    let auth_lease = runtime.generated_auth_lease_handle();
    let mutation_store = persistence.token_store();
    let mutation_binding = auth_binding.clone();
    let mutation_tokens = tokens.clone();
    let key = TokenKey::from_auth_binding(auth_binding);
    let load_key = key.clone();
    let mutation_id = id.clone();
    let outcome = refresh_coordinator
        .with_exclusive_mutation(
            key,
            Box::new(move || {
                Box::pin(async move {
                    let lease_key = LeaseKey::from_auth_binding(&mutation_binding);
                    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
                    if let Some(response) = save_tokens_and_consume_device_flow_unlocked(
                        mutation_id,
                        &mutation_store,
                        &auth_lease,
                        &mutation_binding,
                        &mutation_tokens,
                        poll_lease,
                    )
                    .await
                    {
                        return Err(rpc_response_to_credential_mutation_error(response));
                    }
                    let committed = mutation_store
                        .load(&load_key)
                        .await
                        .map_err(|error| CredentialMutationError::TokenStore(error.to_string()))?
                        .ok_or_else(|| {
                            CredentialMutationError::TokenStore(
                                "successful device login left no persisted credential".to_string(),
                            )
                        })?;
                    Ok(CredentialMutationOutcome::Persisted(committed))
                })
            }),
        )
        .await;
    match outcome {
        Ok(CredentialMutationOutcome::Persisted(_)) => None,
        Ok(CredentialMutationOutcome::Cleared) => Some(RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "device-login transaction returned cleared outcome",
        )),
        Err(error) => Some(RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            error.to_string(),
        )),
    }
}

struct BrowserFlowConsume<'a> {
    authority: &'a dyn meerkat_providers::oauth_flow::OAuthFlowAuthority,
    state: &'a str,
    provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
    redirect_uri: &'a str,
}

struct OwnedBrowserFlowConsume {
    authority: Arc<dyn meerkat_providers::oauth_flow::OAuthFlowAuthority>,
    state: String,
    provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
    redirect_uri: String,
}

async fn save_tokens_and_consume_browser_flow_unlocked(
    id: Option<RpcId>,
    store: &Arc<dyn TokenStore>,
    auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    flow: BrowserFlowConsume<'_>,
) -> Result<(), RpcResponse> {
    if !flow.authority.terminal_flow_state_is_authmachine_owned() {
        return Err(RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "consume_oauth_browser_flow requires an AuthMachine-owned OAuth flow authority",
        ));
    }
    let prepared =
        prepare_token_commit_unlocked(id.clone(), store, auth_lease, auth_binding).await?;
    flow.authority
        .consume(flow.state, auth_binding, flow.provider, flow.redirect_uri)
        .map_err(|err| {
            release_uncredentialed_terminal_oauth_lifecycle(auth_lease, auth_binding);
            RpcResponse::error(
                id.clone(),
                error::INTERNAL_ERROR,
                format!("oauth state terminal consume failed: {err}"),
            )
        })?;
    save_prepared_tokens_after_terminal_consume_unlocked(
        id,
        store,
        auth_lease,
        auth_binding,
        tokens,
        prepared,
    )
    .await?;
    Ok(())
}

async fn save_tokens_and_consume_browser_flow(
    id: Option<RpcId>,
    runtime: &SessionRuntime,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    flow: OwnedBrowserFlowConsume,
) -> Result<(), RpcResponse> {
    let persistence = require_provider_auth_persistence(runtime, id.clone())?;
    let refresh_coordinator = persistence.refresh_coordinator();
    let auth_lease = runtime.generated_auth_lease_handle();
    let mutation_store = persistence.token_store();
    let mutation_binding = auth_binding.clone();
    let mutation_tokens = tokens.clone();
    let key = TokenKey::from_auth_binding(auth_binding);
    let load_key = key.clone();
    let mutation_id = id.clone();
    let outcome = refresh_coordinator
        .with_exclusive_mutation(
            key,
            Box::new(move || {
                Box::pin(async move {
                    let lease_key = LeaseKey::from_auth_binding(&mutation_binding);
                    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
                    save_tokens_and_consume_browser_flow_unlocked(
                        mutation_id,
                        &mutation_store,
                        &auth_lease,
                        &mutation_binding,
                        &mutation_tokens,
                        BrowserFlowConsume {
                            authority: flow.authority.as_ref(),
                            state: &flow.state,
                            provider: flow.provider,
                            redirect_uri: &flow.redirect_uri,
                        },
                    )
                    .await
                    .map_err(rpc_response_to_credential_mutation_error)?;
                    let committed = mutation_store
                        .load(&load_key)
                        .await
                        .map_err(|error| CredentialMutationError::TokenStore(error.to_string()))?
                        .ok_or_else(|| {
                            CredentialMutationError::TokenStore(
                                "successful browser login left no persisted credential".to_string(),
                            )
                        })?;
                    Ok(CredentialMutationOutcome::Persisted(committed))
                })
            }),
        )
        .await
        .map_err(|error| {
            RpcResponse::error(id.clone(), error::INTERNAL_ERROR, error.to_string())
        })?;
    match outcome {
        CredentialMutationOutcome::Persisted(_) => Ok(()),
        CredentialMutationOutcome::Cleared => Err(RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            "browser-login transaction returned cleared outcome",
        )),
    }
}

async fn clear_tokens_and_publish_lifecycle(
    id: Option<RpcId>,
    runtime: &SessionRuntime,
    auth_binding: &AuthBindingRef,
) -> Result<(), RpcResponse> {
    let persistence = require_provider_auth_persistence(runtime, id.clone())?;
    let auth_lease = runtime.generated_auth_lease_handle();
    meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated(
        persistence,
        auth_lease,
        auth_binding.clone(),
    )
    .await
    .map_err(|error| RpcResponse::error(id, error::INTERNAL_ERROR, error.to_string()))
}

// --- Realm projection -------------------------------------------------

pub async fn handle_realm_list(id: Option<RpcId>, runtime: &SessionRuntime) -> RpcResponse {
    let config = match load_config(runtime).await {
        Ok(c) => c,
        Err(r) => return r.with_id(id),
    };
    let realms: Vec<meerkat_contracts::WireRealmSummary> = config
        .realm
        .iter()
        .map(|(realm_id, section)| meerkat_contracts::WireRealmSummary {
            realm_id: realm_id.clone(),
            default_binding: section.default_binding.clone(),
            backend_count: section.backend.len(),
            auth_profile_count: section.auth.len(),
            binding_count: section.binding.len(),
        })
        .collect();
    RpcResponse::success(id, meerkat_contracts::WireRealmList { realms })
}

pub async fn handle_realm_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: RealmIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let realm = match resolve_realm(runtime, &parsed.realm_id).await {
        Ok(r) => r,
        Err(r) => return r.with_id(id),
    };
    let wire = WireRealmConnectionSet::from(&realm);
    match serde_json::to_value(wire) {
        Ok(v) => RpcResponse::success(id, v),
        Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

// --- Auth profile CRUD ------------------------------------------------

pub async fn handle_auth_profile_list(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: RealmIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let realm = match resolve_realm(runtime, &parsed.realm_id).await {
        Ok(r) => r,
        Err(r) => return r.with_id(id),
    };
    let profiles: Vec<WireAuthProfile> = realm
        .auth_profiles
        .values()
        .map(WireAuthProfile::from)
        .collect();
    let backends: Vec<WireBackendProfile> = realm
        .backends
        .values()
        .map(WireBackendProfile::from)
        .collect();
    let bindings: Vec<WireProviderBinding> = realm
        .bindings
        .values()
        .map(WireProviderBinding::from)
        .collect();
    RpcResponse::success(
        id,
        meerkat_contracts::WireAuthProfilesList {
            realm_id: realm.realm_id.to_string(),
            auth_profiles: profiles,
            backend_profiles: backends,
            bindings,
        },
    )
}

pub async fn handle_auth_profile_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: BindingIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    match resolve_binding_identity(
        runtime,
        &parsed.realm_id,
        &parsed.binding_id,
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok((auth_binding, binding, auth_profile)) => RpcResponse::success(
            id,
            meerkat_contracts::WireAuthProfileDetail {
                auth_binding: meerkat_contracts::WireAuthBindingRef::from(auth_binding),
                binding_id: binding.id,
                profile_id: auth_profile.id.clone(),
                auth_profile: WireAuthProfile::from(&auth_profile),
            },
        ),
        Err(r) => r.with_id(id),
    }
}

pub async fn handle_auth_profile_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: CreateProfileParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (auth_binding, _binding, auth_profile) = match resolve_binding_identity(
        runtime,
        &parsed.realm_id,
        &parsed.binding_id,
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    if parsed.auth_method != auth_profile.auth_method {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "binding {} resolves auth_method '{}' not '{}'",
                parsed.binding_id, auth_profile.auth_method, parsed.auth_method,
            ),
        );
    }
    // The persisted-mode mapping + "direct-secret only at create surfaces"
    // predicate are owned by the typed enum (canonical table + createability),
    // not a hand-maintained literal allowlist. Resolve the typed
    // `NormalizedAuthMethod` from the profile's typed provider + declared
    // method, then ask the type. OAuth-login-lifecycle modes and
    // authorizer-backed methods (no persisted secret) are rejected here.
    let auth_mode = match persisted_auth_mode_for_profile(&auth_profile) {
        Some(mode) if meerkat_core::persisted_auth_mode_is_directly_creatable(mode) => mode,
        _ => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!(
                    "auth_method '{}' cannot be created via RPC. \
                     OAuth methods use auth/login/start + auth/login/complete; \
                     managed_store and external_resolver are configured via TOML.",
                    auth_profile.auth_method,
                ),
            );
        }
    };
    if let Some(r) = require_managed_store_source(id.clone(), &parsed.binding_id, &auth_profile) {
        return r;
    }
    if let Err(response) = require_provider_auth_persistence(runtime, id.clone()) {
        return response;
    }
    let tokens = PersistedTokens {
        auth_mode,
        primary_secret: Some(parsed.secret),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(chrono::Utc::now()),
        scopes: Vec::new(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    if let Err(resp) =
        save_tokens_and_publish_lifecycle(id.clone(), runtime, &auth_binding, &tokens).await
    {
        return resp;
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?auth_binding,
        action = "create_profile",
        provider = %auth_profile.provider.as_str(),
        auth_method = %auth_profile.auth_method,
        "binding-scoped auth credentials stored via RPC"
    );
    RpcResponse::success(
        id,
        meerkat_contracts::WireAuthProfileCreated {
            identity: meerkat_contracts::WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            provider: auth_profile.provider.as_str().to_string(),
            auth_method: auth_profile.auth_method.clone(),
            stored: true,
        },
    )
}

pub async fn handle_auth_profile_delete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: BindingIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (auth_binding, _binding, auth_profile) = match resolve_binding_identity(
        runtime,
        &parsed.realm_id,
        &parsed.binding_id,
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    if let Err(response) = require_provider_auth_persistence(runtime, id.clone()) {
        return response;
    }
    if let Err(resp) = clear_tokens_and_publish_lifecycle(id.clone(), runtime, &auth_binding).await
    {
        return resp;
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?auth_binding,
        action = "delete_profile",
        "binding-scoped auth credentials deleted via RPC"
    );
    RpcResponse::success(
        id,
        meerkat_contracts::WireAuthProfileCleared {
            identity: meerkat_contracts::WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            cleared: true,
        },
    )
}

// --- OAuth login ------------------------------------------------------

pub async fn handle_auth_login_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: LoginStartParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let resolved = match resolve_oauth_provider(&parsed.provider, &parsed.redirect_uri) {
        Ok(v) => v,
        Err(e) => {
            return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string());
        }
    };
    let target = match resolve_oauth_target(
        runtime,
        resolved.provider,
        Some(parsed.realm_id.as_str()),
        Some(parsed.binding_id.as_str()),
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    // Strict-owner write guard (decision 5): reject an OAuth login that targets
    // a realm which only INHERITS this binding from an ancestor (e.g. `global`),
    // naming the owning realm. Reads inherit; writes are strict-owner.
    if let Err(r) = guard_strict_owner_write(
        runtime,
        &id,
        parsed.realm_id.as_str(),
        parsed.binding_id.as_str(),
    )
    .await
    {
        return r;
    }
    if let Err(e) =
        validate_oauth_login_binding(&target.backend, &target.auth_profile, resolved.identity)
    {
        return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string());
    }
    let auth_binding = target.auth_binding;
    let pkce = PkcePair::generate_s256();
    let verifier = pkce.verifier.secret().clone();
    let lease_key = LeaseKey::from_auth_binding(&auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    let state_token = match runtime.oauth_flow_authority().start(
        auth_binding,
        resolved.identity,
        parsed.redirect_uri.clone(),
        verifier,
    ) {
        Ok(state) => state,
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("oauth state initialization failed: {e}"),
            );
        }
    };
    let authorize_url = resolved
        .endpoints
        .authorize_url_with_pkce(&pkce.challenge, &state_token);
    RpcResponse::success(
        id,
        meerkat_contracts::WireLoginStart {
            authorize_url,
            state: state_token,
            redirect_uri: parsed.redirect_uri,
            provider: parsed.provider,
        },
    )
}

pub async fn handle_auth_login_complete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: LoginCompleteParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let resolved = match resolve_oauth_provider(&parsed.provider, &parsed.redirect_uri) {
        Ok(v) => v,
        Err(e) => {
            return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string());
        }
    };
    let provider = resolved.provider;
    let target = match resolve_oauth_target(
        runtime,
        provider,
        Some(parsed.realm_id.as_str()),
        Some(parsed.binding_id.as_str()),
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    // Strict-owner write guard (decision 5): reject persisting an OAuth login
    // against a realm that only INHERITS this binding, naming the owner.
    if let Err(r) = guard_strict_owner_write(
        runtime,
        &id,
        parsed.realm_id.as_str(),
        parsed.binding_id.as_str(),
    )
    .await
    {
        return r;
    }
    let auth_binding = target.auth_binding;
    let binding = target.binding;
    let backend_profile = target.backend;
    let auth_profile = target.auth_profile;
    if provider != auth_profile.provider {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "binding {} resolves provider '{}' not '{}'",
                binding.id,
                auth_profile.provider.as_str(),
                parsed.provider,
            ),
        );
    }
    if let Err(e) = validate_oauth_login_binding(&backend_profile, &auth_profile, resolved.identity)
    {
        return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string());
    }

    if let Err(response) = require_provider_auth_persistence(runtime, id.clone()) {
        return response;
    }
    let flow = match runtime.oauth_flow_authority().verify(
        &parsed.state,
        &auth_binding,
        resolved.identity,
        &parsed.redirect_uri,
    ) {
        Ok(flow) => flow,
        Err(OAuthFlowError::Missing) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "oauth state is missing or expired",
            );
        }
        Err(e @ OAuthFlowError::RegistryProjectionMissing { .. }) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("oauth registry projection failed: {e}"),
            );
        }
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("oauth state verification failed: {e}"),
            );
        }
    };

    let http = reqwest::Client::new();
    let result = match exchange_authorization_code_with_state(
        &http,
        &resolved.endpoints,
        &parsed.code,
        &flow.pkce_verifier,
        resolved.client_secret,
        Some(&parsed.state),
    )
    .await
    {
        Ok(r) => r,
        Err(OAuthError::TokenEndpoint { status, body }) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("token endpoint returned {status}: {body}"),
            );
        }
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("Token exchange failed: {e}"),
            );
        }
    };
    let expires_at = match result.expires_at_from(chrono::Utc::now()) {
        Ok(expires_at) => expires_at,
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("token expiry is invalid: {e}"),
            );
        }
    };
    let tokens = PersistedTokens {
        auth_mode: resolved.auth_mode,
        primary_secret: Some(result.access_token),
        refresh_token: result.refresh_token,
        id_token: result.id_token,
        expires_at,
        last_refresh: Some(chrono::Utc::now()),
        scopes: result
            .scope
            .as_deref()
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let authority = runtime.oauth_flow_authority();
    if let Err(resp) = save_tokens_and_consume_browser_flow(
        id.clone(),
        runtime,
        &auth_binding,
        &tokens,
        OwnedBrowserFlowConsume {
            authority,
            state: parsed.state.clone(),
            provider: resolved.identity,
            redirect_uri: parsed.redirect_uri.clone(),
        },
    )
    .await
    {
        return resp;
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?auth_binding,
        action = "login_oauth_complete",
        provider = %parsed.provider,
        has_refresh_token = %tokens.refresh_token.is_some(),
        "OAuth login completed via RPC"
    );
    RpcResponse::success(
        id,
        meerkat_contracts::WireLoginReady {
            state: None,
            identity: meerkat_contracts::WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            provider: parsed.provider,
            expires_at: expires_at.map(|e| e.to_rfc3339()),
            has_refresh_token: tokens.refresh_token.is_some(),
            scopes: tokens.scopes,
        },
    )
}

pub async fn handle_auth_login_device_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: DeviceStartParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let resolved = match resolve_oauth_provider(&parsed.provider, "") {
        Ok(v) => v,
        Err(e) => return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string()),
    };
    if resolved.endpoints.device_code_url.is_none() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "provider '{}' does not support the device-code flow",
                parsed.provider,
            ),
        );
    }
    let target = match resolve_oauth_target(
        runtime,
        resolved.provider,
        Some(parsed.realm_id.as_str()),
        Some(parsed.binding_id.as_str()),
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    // Strict-owner write guard (decision 5): reject a device-flow login that
    // targets a realm which only INHERITS this binding, naming the owner.
    if let Err(r) = guard_strict_owner_write(
        runtime,
        &id,
        parsed.realm_id.as_str(),
        parsed.binding_id.as_str(),
    )
    .await
    {
        return r;
    }
    if let Err(e) =
        validate_oauth_login_binding(&target.backend, &target.auth_profile, resolved.identity)
    {
        return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string());
    }
    let auth_binding = target.auth_binding;
    let http = reqwest::Client::new();
    match request_device_code(&http, &resolved.endpoints).await {
        Ok(resp) => {
            let lease_key = LeaseKey::from_auth_binding(&auth_binding);
            let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
            if let Err(err) = runtime.oauth_flow_authority().admit_device_code(
                auth_binding,
                resolved.identity,
                resp.device_code.clone(),
                std::time::Duration::from_secs(resp.expires_in),
            ) {
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("oauth device state initialization failed: {err}"),
                );
            }
            RpcResponse::success(
                id,
                meerkat_contracts::WireDeviceStart {
                    device_code: resp.device_code,
                    user_code: resp.user_code,
                    verification_uri: resp.verification_uri,
                    verification_uri_complete: resp.verification_uri_complete,
                    expires_in: resp.expires_in,
                    interval: resp.interval,
                    provider: parsed.provider,
                },
            )
        }
        Err(e) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("device-code request failed: {e}"),
        ),
    }
}

/// Plan §1.5r.9 device-flow completion leg. Single-poll semantics — the
/// caller runs the outer retry loop using the `interval` returned from
/// `auth.login.device_start`. Returns one of:
///   * `{ state: "pending" }`     (202-equivalent)
///   * `{ state: "slow_down" }`   (429-equivalent; bump caller's interval)
///   * `{ state: "access_denied" }` / `{ state: "expired" }` (terminal)
///   * `{ state: "ready", realm_id, binding_id, expires_at, ... }`
///     (tokens persisted to TokenStore under the resolved `AuthBindingRef`).
pub async fn handle_auth_login_device_complete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: DeviceCompleteParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let resolved = match resolve_oauth_provider(&parsed.provider, "") {
        Ok(v) => v,
        Err(e) => return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string()),
    };
    let provider = resolved.provider;
    if resolved.endpoints.device_code_url.is_none() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "provider '{}' does not support the device-code flow",
                parsed.provider,
            ),
        );
    }
    let target = match resolve_oauth_target(
        runtime,
        provider,
        Some(parsed.realm_id.as_str()),
        Some(parsed.binding_id.as_str()),
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    // Strict-owner write guard (decision 5): reject persisting an OAuth login
    // against a realm that only INHERITS this binding, naming the owner.
    if let Err(r) = guard_strict_owner_write(
        runtime,
        &id,
        parsed.realm_id.as_str(),
        parsed.binding_id.as_str(),
    )
    .await
    {
        return r;
    }
    let auth_binding = target.auth_binding;
    let binding = target.binding;
    let backend_profile = target.backend;
    let auth_profile = target.auth_profile;
    if provider != auth_profile.provider {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "binding {} resolves provider '{}' not '{}'",
                binding.id,
                auth_profile.provider.as_str(),
                parsed.provider,
            ),
        );
    }
    if let Err(e) = validate_oauth_login_binding(&backend_profile, &auth_profile, resolved.identity)
    {
        return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string());
    }
    let poll_lease = match runtime.oauth_flow_authority().begin_device_code_poll(
        &parsed.device_code,
        &auth_binding,
        resolved.identity,
    ) {
        Ok(lease) => lease,
        Err(err) => return oauth_device_state_error(id, err),
    };
    let http = reqwest::Client::new();
    let outcome = match poll_device_code(
        &http,
        &resolved.endpoints,
        &parsed.device_code,
        resolved.client_secret,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("device-code poll failed: {e}"),
            );
        }
    };
    match outcome {
        DevicePollOutcome::Pending => match finish_device_flow_poll(id.clone(), poll_lease) {
            None => RpcResponse::success(id, WireDeviceCompleteResult::Pending),
            Some(resp) => resp,
        },
        DevicePollOutcome::SlowDown => match finish_device_flow_poll(id.clone(), poll_lease) {
            None => RpcResponse::success(id, WireDeviceCompleteResult::SlowDown),
            Some(resp) => resp,
        },
        DevicePollOutcome::AccessDenied => match consume_terminal_device_flow(
            id.clone(),
            &runtime.generated_auth_lease_handle(),
            &auth_binding,
            poll_lease,
        ) {
            None => RpcResponse::success(id, WireDeviceCompleteResult::AccessDenied),
            Some(resp) => resp,
        },
        DevicePollOutcome::Expired => match consume_terminal_device_flow(
            id.clone(),
            &runtime.generated_auth_lease_handle(),
            &auth_binding,
            poll_lease,
        ) {
            None => RpcResponse::success(id, WireDeviceCompleteResult::Expired),
            Some(resp) => resp,
        },
        DevicePollOutcome::Ready(result) => {
            if let Err(response) = require_provider_auth_persistence(runtime, id.clone()) {
                return response;
            }
            let expires_at = match result.expires_at_from(chrono::Utc::now()) {
                Ok(expires_at) => expires_at,
                Err(e) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("token expiry is invalid: {e}"),
                    );
                }
            };
            let tokens = PersistedTokens {
                auth_mode: resolved.auth_mode,
                primary_secret: Some(result.access_token),
                refresh_token: result.refresh_token,
                id_token: result.id_token,
                expires_at,
                last_refresh: Some(chrono::Utc::now()),
                scopes: result
                    .scope
                    .as_deref()
                    .map(|s| s.split_whitespace().map(String::from).collect())
                    .unwrap_or_default(),
                account_id: None,
                metadata: serde_json::Value::Null,
            };
            if let Some(resp) = save_tokens_and_consume_device_flow(
                id.clone(),
                runtime,
                &auth_binding,
                &tokens,
                poll_lease,
            )
            .await
            {
                return resp;
            }
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?auth_binding,
                action = "login_device_complete",
                provider = %parsed.provider,
                has_refresh_token = %tokens.refresh_token.is_some(),
                "OAuth device-flow login completed via RPC"
            );
            RpcResponse::success(
                id,
                WireDeviceCompleteResult::Ready {
                    identity: Box::new(WireBindingIdentity::from(&auth_binding)),
                    profile_id: auth_profile.id,
                    provider: parsed.provider,
                    expires_at: expires_at.map(|e| e.to_rfc3339()),
                    has_refresh_token: tokens.refresh_token.is_some(),
                    scopes: tokens.scopes,
                },
            )
        }
    }
}

/// Plan §4b.5 closure: Console OAuth → API key provisioning. The
/// caller runs a Console-scope OAuth flow (scope
/// `org:create_api_key user:profile`), then hands the resulting
/// access_token to this method. We POST to
/// `https://api.anthropic.com/api/oauth/claude_cli/create_api_key`
/// and persist the returned API key as a stable credential under the
/// resolved `AuthBindingRef` with auth_mode=OauthToApiKey — so future
/// `resolve_binding` for the `oauth_to_api_key` method reads the
/// provisioned api_key without needing any refresh.
pub async fn handle_auth_login_provision_api_key(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    use meerkat_anthropic::runtime::oauth as a_oauth;
    let parsed: ProvisionApiKeyParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (realm_id, binding_id) = match require_explicit_oauth_identity(
        id.clone(),
        parsed.realm_id.as_deref(),
        parsed.binding_id.as_deref(),
    ) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let target = match resolve_oauth_target(
        runtime,
        Provider::Anthropic,
        Some(realm_id),
        Some(binding_id),
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    // Strict-owner write guard (decision 5): reject provisioning an API key into
    // a realm that only INHERITS this binding, naming the owner.
    if let Err(r) = guard_strict_owner_write(runtime, &id, realm_id, binding_id).await {
        return r;
    }
    let auth_binding = target.auth_binding;
    let backend_profile = target.backend;
    let auth_profile = target.auth_profile;
    if let Err(e) = validate_oauth_target_binding_for_auth_mode(
        &backend_profile,
        &auth_profile,
        Provider::Anthropic,
        PersistedAuthMode::OauthToApiKey,
        meerkat_providers::NormalizedBackendKind::Anthropic(
            meerkat_core::provider_matrix::anthropic::AnthropicBackendKind::AnthropicApi,
        ),
    ) {
        return RpcResponse::error(id, error::INVALID_PARAMS, e.to_string());
    }
    let flow_authority = runtime.oauth_flow_authority();
    let provision_provider = OAuthProviderIdentity::AnthropicConsoleApiKey;
    let provision_redirect_uri = a_oauth::MANUAL_REDIRECT_URL;
    let provision_state = match flow_authority.start(
        auth_binding.clone(),
        provision_provider,
        provision_redirect_uri.to_string(),
        "provision-api-key".to_string(),
    ) {
        Ok(state) => state,
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("oauth provision lifecycle admission failed: {e}"),
            );
        }
    };
    let persistence = match require_provider_auth_persistence(runtime, id.clone()) {
        Ok(persistence) => persistence,
        Err(r) => {
            let _ = flow_authority.consume(
                &provision_state,
                &auth_binding,
                provision_provider,
                provision_redirect_uri,
            );
            return r;
        }
    };
    // Console endpoints drive `scope = org:create_api_key user:profile`.
    // The runtime wrapper's `provision_api_key` POSTs to
    // API_KEY_CREATE_URL with `Authorization: Bearer <access_token>`
    // and returns the API key token bundle; this handler owns the
    // TokenStore/AuthMachine commit and terminal flow consume.
    let endpoints = a_oauth::console_endpoints(a_oauth::MANUAL_REDIRECT_URL);
    let oauth_runtime = a_oauth::AnthropicOAuthRuntime::new(
        persistence,
        endpoints,
        TokenKey::from_auth_binding(&auth_binding),
    );
    match oauth_runtime
        .provision_api_key_tokens(&parsed.access_token)
        .await
    {
        Ok(tokens) => {
            if let Err(resp) = save_tokens_and_consume_browser_flow(
                id.clone(),
                runtime,
                &auth_binding,
                &tokens,
                OwnedBrowserFlowConsume {
                    authority: Arc::clone(&flow_authority),
                    state: provision_state.clone(),
                    provider: provision_provider,
                    redirect_uri: provision_redirect_uri.to_string(),
                },
            )
            .await
            {
                let _ = flow_authority.consume(
                    &provision_state,
                    &auth_binding,
                    provision_provider,
                    provision_redirect_uri,
                );
                return resp;
            }
            RpcResponse::success(
                id,
                WireProvisionApiKeyResult {
                    identity: WireBindingIdentity::from(&auth_binding),
                    profile_id: auth_profile.id,
                    provider: "anthropic".to_string(),
                    auth_mode: "oauth_to_api_key".to_string(),
                    has_api_key: tokens.primary_secret.is_some(),
                    scopes: tokens.scopes,
                },
            )
        }
        Err(e) => {
            let _ = flow_authority.consume(
                &provision_state,
                &auth_binding,
                provision_provider,
                provision_redirect_uri,
            );
            RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("provision_api_key failed: {e}"),
            )
        }
    }
}

pub async fn handle_auth_status_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: BindingIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (auth_binding, _binding, auth_profile) = match resolve_binding_identity(
        runtime,
        &parsed.realm_id,
        &parsed.binding_id,
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let auth_lease = runtime.auth_lease_handle();
    let generated_auth_lease = runtime.generated_auth_lease_handle();
    let lease_key = LeaseKey::from_auth_binding(&auth_binding);
    let now = chrono::Utc::now();
    if let Err(err) = auth_lease.observe_credential_freshness(
        &lease_key,
        now.timestamp().max(0) as u64,
        meerkat_core::handles::AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
    ) {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("AuthMachine freshness observation failed: {err}"),
        );
    }
    let mut snapshot = auth_lease.snapshot(&lease_key);
    let expected_mode = persisted_auth_mode_for_profile(&auth_profile);
    let source_uses_store = credential_source_uses_persisted_store(&auth_profile.source);
    let oauth_mode = expected_mode
        .map(persisted_auth_mode_is_oauth_login)
        .unwrap_or(false);
    let phase = meerkat_core::AuthStatusPhase::from_lease_snapshot(now, &snapshot);
    let token_store = match runtime.token_store() {
        Ok(store) => store,
        Err(err) => {
            // K6: a token-store open failure is a typed fault — report it
            // instead of laundering it into "no persisted credentials".
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("TokenStore unavailable: {err}"),
            );
        }
    };
    let mut stored = None;
    if source_uses_store {
        if phase.is_no_live_lease() {
            if let (Some(expected_mode), Some(store)) = (expected_mode, token_store.as_ref()) {
                // A store fault during rehydration is a real error, not
                // absent credentials: collapsing it would report a store
                // failure as "no credentials"/Unknown status.
                match meerkat_core::rehydrate_marked_tokens_for_status(
                    store.as_ref(),
                    &generated_auth_lease,
                    &auth_binding,
                    expected_mode,
                    now,
                )
                .await
                {
                    Ok(Some(rehydrated)) => {
                        stored = Some(rehydrated);
                        snapshot = auth_lease.snapshot(&lease_key);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        return RpcResponse::error(
                            id,
                            error::INTERNAL_ERROR,
                            format!("TokenStore rehydration failed: {err}"),
                        );
                    }
                }
            }
        } else if let Some(store) = token_store.as_ref() {
            // A store-load fault is a real error, not absent credentials.
            stored = match store
                .load(&TokenKey::from_auth_binding(&auth_binding))
                .await
            {
                Ok(stored) => stored,
                Err(err) => {
                    return RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("TokenStore load failed: {err}"),
                    );
                }
            };
        }
    }
    if stored
        .as_ref()
        .is_some_and(|tokens| Some(tokens.auth_mode) != expected_mode)
    {
        stored = None;
    }
    let oauth_source_rejected = expected_mode
        .map(|mode| persisted_auth_mode_is_oauth_login(mode) && !source_uses_store)
        .unwrap_or(false);
    let token_matches_binding = if source_uses_store {
        match stored.as_ref() {
            Some(tokens) => Some(tokens.auth_mode) == expected_mode,
            None => !oauth_mode,
        }
    } else {
        true
    };
    let marker_projection_snapshot;
    let (projection_tokens, projection_snapshot) = if token_matches_binding {
        marker_projection_snapshot = stored.as_ref().filter(|_| oauth_mode).and_then(|tokens| {
            meerkat_core::oauth_status_projection_snapshot_from_newer_marker(&snapshot, tokens)
        });
        (
            if source_uses_store && !oauth_source_rejected {
                stored.as_ref()
            } else {
                None
            },
            marker_projection_snapshot.as_ref().unwrap_or(&snapshot),
        )
    } else {
        (None, &snapshot)
    };
    let projection =
        meerkat_core::project_published_auth_status(now, projection_tokens, projection_snapshot);
    let tokens = projection.tokens;
    RpcResponse::success(
        id,
        WireAuthStatusDetail {
            identity: WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id,
            provider: auth_profile.provider.as_str().to_string(),
            auth_method: auth_profile.auth_method,
            state: projection.phase,
            expires_at: projection.expires_at.map(|e| e.to_rfc3339()),
            last_refresh_at: tokens.and_then(|t| t.last_refresh.map(|e| e.to_rfc3339())),
            account_id: tokens.and_then(|t| t.account_id.clone()),
            has_refresh_token: tokens.map(|t| t.refresh_token.is_some()).unwrap_or(false),
        },
    )
}

pub async fn handle_auth_logout(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: BindingIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (auth_binding, _binding, auth_profile) = match resolve_binding_identity(
        runtime,
        &parsed.realm_id,
        &parsed.binding_id,
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    if let Err(response) = require_provider_auth_persistence(runtime, id.clone()) {
        return response;
    }
    if let Err(resp) = clear_tokens_and_publish_lifecycle(id.clone(), runtime, &auth_binding).await
    {
        return resp;
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?auth_binding,
        action = "logout",
        "binding-scoped auth credentials logged out via RPC"
    );
    RpcResponse::success(
        id,
        meerkat_contracts::WireAuthProfileCleared {
            identity: meerkat_contracts::WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            cleared: true,
        },
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::handles::{AuthLeaseHandle, AuthLeasePhase, AuthLeaseTransition};
    use meerkat_providers::auth_store::FileTokenStore;
    use meerkat_runtime::RuntimeAuthLeaseHandle;

    fn raw_params(value: serde_json::Value) -> Box<RawValue> {
        serde_json::value::to_raw_value(&value).unwrap()
    }

    fn test_runtime() -> SessionRuntime {
        test_runtime_with_config(meerkat_core::Config::default())
    }

    fn openai_auth_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        }
    }

    fn google_auth_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_google").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        }
    }

    fn generated_auth_transition_for_test(
        lease_key: &LeaseKey,
        expires_at: u64,
    ) -> AuthLeaseTransition {
        let handle = RuntimeAuthLeaseHandle::new();
        handle.acquire_lease(lease_key, expires_at).unwrap()
    }

    fn mark_tokens_lifecycle_published_for_test(
        tokens: &PersistedTokens,
        generation: u64,
        credential_published_at_millis: Option<u64>,
    ) -> PersistedTokens {
        let _ = (generation, credential_published_at_millis);
        let key = TokenKey::from_auth_binding(&openai_auth_binding());
        let lease_key = LeaseKey::from_auth_binding(&openai_auth_binding());
        let transition = generated_auth_transition_for_test(
            &lease_key,
            meerkat_core::persisted_token_expires_at_epoch_secs(tokens),
        );
        meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, tokens, &transition)
            .expect("runtime AuthMachine transition marks fixture tokens")
    }

    fn test_runtime_with_config(config: meerkat_core::Config) -> SessionRuntime {
        let token_store: Arc<dyn TokenStore> =
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new());
        test_runtime_with_config_and_token_store(config, token_store)
    }

    fn test_runtime_with_config_without_provider_auth_persistence(
        config: meerkat_core::Config,
    ) -> SessionRuntime {
        let temp = tempfile::tempdir().unwrap();
        let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
            .without_provider_auth_persistence();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(config.clone(), meerkat_models::canonical()),
        );
        let runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(
                store,
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                blob_store,
            ),
            crate::router::NotificationSink::noop(),
        );
        runtime.set_config_runtime(Arc::new(meerkat_core::ConfigRuntime::new(
            config_store,
            temp.path().join("config_state.json"),
        )));
        runtime
    }

    fn test_runtime_with_config_and_token_store(
        config: meerkat_core::Config,
        token_store: Arc<dyn TokenStore>,
    ) -> SessionRuntime {
        let temp = tempfile::tempdir().unwrap();
        let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
            .with_provider_auth_persistence(
                meerkat_providers::auth_store::ProviderAuthPersistence::new(
                    token_store,
                    Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
                ),
            );
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let config_store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(config.clone(), meerkat_models::canonical()),
        );
        let runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(
                store,
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                blob_store,
            ),
            crate::router::NotificationSink::noop(),
        );
        runtime.set_config_runtime(Arc::new(meerkat_core::ConfigRuntime::new(
            config_store,
            temp.path().join("config_state.json"),
        )));
        runtime
    }

    fn config_with_anthropic_default_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        config.realm.insert(
            "default".to_string(),
            meerkat_core::RealmConfigSection::from_inline_api_keys(&[("anthropic", "secret")]),
        );
        config
    }

    fn config_with_openai_managed_store_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "openai_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: meerkat_core::provider_matrix::OpenAiBackendKind::OpenAiApi
                    .as_str()
                    .into(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_managed".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: meerkat_core::provider_matrix::OpenAiAuthMethod::ApiKey
                    .as_str()
                    .into(),
                source: CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_openai".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "openai_backend".into(),
                auth_profile: "openai_managed".into(),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
        section.default_binding = Some("default_openai".into());
        config.realm.insert("dev".into(), section);
        config
    }

    fn config_with_openai_oauth_binding(source: CredentialSourceSpec) -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "chatgpt_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: meerkat_core::provider_matrix::OpenAiBackendKind::ChatGptBackend
                    .as_str()
                    .into(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_oauth".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: meerkat_core::provider_matrix::OpenAiAuthMethod::ManagedChatGptOauth
                    .as_str()
                    .into(),
                source,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_openai".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "chatgpt_backend".into(),
                auth_profile: "openai_oauth".into(),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
        section.default_binding = Some("default_openai".into());
        config.realm.insert("dev".into(), section);
        config
    }

    fn config_with_openai_oauth_wrong_backend_binding() -> meerkat_core::Config {
        let mut config = config_with_openai_oauth_binding(CredentialSourceSpec::ManagedStore);
        config
            .realm
            .get_mut("dev")
            .unwrap()
            .backend
            .get_mut("chatgpt_backend")
            .unwrap()
            .backend_kind = meerkat_core::provider_matrix::OpenAiBackendKind::OpenAiApi
            .as_str()
            .into();
        config
    }

    fn config_with_openai_external_authorizer_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "openai_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: meerkat_core::provider_matrix::OpenAiBackendKind::OpenAiApi
                    .as_str()
                    .into(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_external".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: meerkat_core::provider_matrix::OpenAiAuthMethod::ExternalAuthorizer
                    .as_str()
                    .into(),
                source: CredentialSourceSpec::ExternalResolver {
                    handle: "external-openai".into(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_openai".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "openai_backend".into(),
                auth_profile: "openai_external".into(),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
        section.default_binding = Some("default_openai".into());
        config.realm.insert("dev".into(), section);
        config
    }

    fn config_with_google_api_key_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "google_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "gemini".into(),
                backend_kind: meerkat_core::provider_matrix::GoogleBackendKind::GoogleGenAi
                    .as_str()
                    .into(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "google_api_key".into(),
            meerkat_core::AuthProfileConfig {
                provider: "gemini".into(),
                auth_method: meerkat_core::provider_matrix::GoogleAuthMethod::ApiKey
                    .as_str()
                    .into(),
                source: CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_google".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "google_backend".into(),
                auth_profile: "google_api_key".into(),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
        section.default_binding = Some("default_google".into());
        config.realm.insert("dev".into(), section);
        config
    }

    fn config_with_google_oauth_wrong_backend_binding() -> meerkat_core::Config {
        let mut config = config_with_google_api_key_binding();
        config
            .realm
            .get_mut("dev")
            .unwrap()
            .auth
            .get_mut("google_api_key")
            .unwrap()
            .auth_method = meerkat_core::provider_matrix::GoogleAuthMethod::GoogleOauth
            .as_str()
            .into();
        config
    }

    fn config_with_anthropic_api_key_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "anthropic_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "anthropic".into(),
                backend_kind: meerkat_core::provider_matrix::AnthropicBackendKind::AnthropicApi
                    .as_str()
                    .into(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "anthropic_api_key".into(),
            meerkat_core::AuthProfileConfig {
                provider: "anthropic".into(),
                auth_method: meerkat_core::provider_matrix::AnthropicAuthMethod::ApiKey
                    .as_str()
                    .into(),
                source: CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_anthropic".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "anthropic_backend".into(),
                auth_profile: "anthropic_api_key".into(),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
        section.default_binding = Some("default_anthropic".into());
        config.realm.insert("dev".into(), section);
        config
    }

    fn config_with_anthropic_oauth_to_api_key_binding() -> meerkat_core::Config {
        let mut config = config_with_anthropic_api_key_binding();
        config
            .realm
            .get_mut("dev")
            .unwrap()
            .auth
            .get_mut("anthropic_api_key")
            .unwrap()
            .auth_method = "oauth_to_api_key".into();
        config
    }

    fn config_with_anthropic_oauth_to_api_key_wrong_backend_binding() -> meerkat_core::Config {
        let mut config = config_with_anthropic_oauth_to_api_key_binding();
        let section = config.realm.get_mut("dev").unwrap();
        section
            .backend
            .get_mut("anthropic_backend")
            .unwrap()
            .backend_kind = "bedrock".into();
        config
    }

    fn auth_status_state(resp: RpcResponse) -> String {
        assert!(
            resp.error.is_none(),
            "status response error: {:?}",
            resp.error
        );
        let value: serde_json::Value =
            serde_json::from_str(resp.result.as_ref().expect("result").get()).unwrap();
        value["state"].as_str().expect("state").to_string()
    }

    fn auth_status_value(resp: RpcResponse) -> serde_json::Value {
        assert!(
            resp.error.is_none(),
            "status response error: {:?}",
            resp.error
        );
        serde_json::from_str(resp.result.as_ref().expect("result").get()).unwrap()
    }

    fn assert_invalid_params_message(resp: RpcResponse, expected: &str) {
        assert!(resp.error.is_some(), "response should be an error");
        let Some(error) = resp.error else {
            return;
        };
        assert_eq!(error.code, crate::error::INVALID_PARAMS);
        assert!(
            error.message.contains(expected),
            "expected error message to contain `{expected}`, got `{}`",
            error.message
        );
    }

    fn chatgpt_oauth_tokens_with_secret(secret: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some(secret.to_string()),
            refresh_token: Some(format!("{secret}-refresh")),
            id_token: None,
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            last_refresh: Some(chrono::Utc::now()),
            scopes: Vec::new(),
            account_id: Some("acct-1".into()),
            metadata: serde_json::Value::Null,
        }
    }

    fn oauth_to_api_key_tokens_with_secret(secret: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: PersistedAuthMode::OauthToApiKey,
            primary_secret: Some(secret.to_string()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: Some(chrono::Utc::now()),
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    struct RejectDeviceConsumeLifecycle;

    impl meerkat_providers::oauth_flow::OAuthDevicePollLifecycle for RejectDeviceConsumeLifecycle {
        fn device_flow_state_is_authmachine_owned(&self) -> bool {
            true
        }

        fn finish_device_poll(
            &self,
            _target: &AuthBindingRef,
            _device_code: &str,
        ) -> Result<(), OAuthFlowError> {
            Ok(())
        }

        fn consume_device_flow(
            &self,
            _target: &AuthBindingRef,
            _device_code: &str,
            _provider: OAuthProviderIdentity,
        ) -> Result<(), OAuthFlowError> {
            Err(OAuthFlowError::LifecycleRejected {
                operation: "consume_oauth_device_flow",
                detail: "test rejection".to_string(),
            })
        }

        fn expire_device_flow(
            &self,
            _target: &AuthBindingRef,
            _device_code: &str,
        ) -> Result<(), OAuthFlowError> {
            Ok(())
        }
    }

    struct RejectBrowserConsumeAuthority;

    impl meerkat_providers::oauth_flow::OAuthFlowAuthority for RejectBrowserConsumeAuthority {
        fn terminal_flow_state_is_authmachine_owned(&self) -> bool {
            true
        }

        fn start(
            &self,
            _target: AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
            _redirect_uri: String,
            _pkce_verifier: String,
        ) -> Result<String, OAuthFlowError> {
            unreachable!("browser consume rollback test only consumes")
        }

        fn verify(
            &self,
            _state: &str,
            _target: &AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
            _redirect_uri: &str,
        ) -> Result<meerkat_providers::oauth_flow::OAuthFlowRecord, OAuthFlowError> {
            unreachable!("browser consume rollback test only consumes")
        }

        fn consume(
            &self,
            _state: &str,
            _target: &AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
            _redirect_uri: &str,
        ) -> Result<meerkat_providers::oauth_flow::OAuthFlowRecord, OAuthFlowError> {
            Err(OAuthFlowError::LifecycleRejected {
                operation: "consume_oauth_browser_flow",
                detail: "test rejection".to_string(),
            })
        }

        fn admit_device_code(
            &self,
            _target: AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
            _device_code: String,
            _expires_in: std::time::Duration,
        ) -> Result<(), OAuthFlowError> {
            unreachable!("browser consume rollback test only consumes")
        }

        fn verify_device_code(
            &self,
            _device_code: &str,
            _target: &AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
        ) -> Result<meerkat_providers::oauth_flow::OAuthDeviceFlowRecord, OAuthFlowError> {
            unreachable!("browser consume rollback test only consumes")
        }

        fn begin_device_code_poll(
            &self,
            _device_code: &str,
            _target: &AuthBindingRef,
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
        ) -> Result<OAuthDevicePollLease, OAuthFlowError> {
            unreachable!("browser consume rollback test only consumes")
        }
    }

    struct LoadFailingTokenStore;

    #[async_trait::async_trait]
    impl TokenStore for LoadFailingTokenStore {
        async fn load(
            &self,
            _key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, meerkat_providers::auth_store::TokenStoreError>
        {
            Err(meerkat_providers::auth_store::TokenStoreError::Serde(
                "malformed token fixture".into(),
            ))
        }

        async fn save(
            &self,
            _key: &TokenKey,
            _tokens: &PersistedTokens,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            Ok(())
        }

        async fn clear(
            &self,
            _key: &TokenKey,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            Ok(())
        }

        async fn list(
            &self,
        ) -> Result<Vec<TokenKey>, meerkat_providers::auth_store::TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "load_failing"
        }
    }

    struct SaveCountingTokenStore {
        inner: meerkat_providers::auth_store::EphemeralTokenStore,
        save_count: std::sync::atomic::AtomicUsize,
    }

    impl SaveCountingTokenStore {
        fn new() -> Self {
            Self {
                inner: meerkat_providers::auth_store::EphemeralTokenStore::new(),
                save_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn save_count(&self) -> usize {
            self.save_count.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl TokenStore for SaveCountingTokenStore {
        async fn load(
            &self,
            key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, meerkat_providers::auth_store::TokenStoreError>
        {
            self.inner.load(key).await
        }

        async fn save(
            &self,
            key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            self.save_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.save(key, tokens).await
        }

        async fn clear(
            &self,
            key: &TokenKey,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            self.inner.clear(key).await
        }

        async fn list(
            &self,
        ) -> Result<Vec<TokenKey>, meerkat_providers::auth_store::TokenStoreError> {
            self.inner.list().await
        }

        fn backend_name(&self) -> &'static str {
            "save_counting"
        }
    }

    /// Store whose durable save always fails while load/clear succeed —
    /// exercises the acquire-first crash shape (lease acquired, durable
    /// commit refused).
    struct SaveFailingTokenStore;

    #[async_trait::async_trait]
    impl TokenStore for SaveFailingTokenStore {
        async fn load(
            &self,
            _key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, meerkat_providers::auth_store::TokenStoreError>
        {
            Ok(None)
        }

        async fn save(
            &self,
            _key: &TokenKey,
            _tokens: &PersistedTokens,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            Err(meerkat_providers::auth_store::TokenStoreError::Io(
                "durable save refused".into(),
            ))
        }

        async fn clear(
            &self,
            _key: &TokenKey,
        ) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
            Ok(())
        }

        async fn list(
            &self,
        ) -> Result<Vec<TokenKey>, meerkat_providers::auth_store::TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "save_failing"
        }
    }

    #[test]
    fn oauth_completion_params_require_explicit_identity() {
        let login = serde_json::from_value::<LoginCompleteParams>(serde_json::json!({
            "provider": "anthropic",
            "code": "code",
            "state": "state",
            "redirect_uri": "http://127.0.0.1:0/callback"
        }))
        .unwrap_err();
        assert!(login.to_string().contains("realm_id"));

        let device = serde_json::from_value::<DeviceCompleteParams>(serde_json::json!({
            "provider": "anthropic",
            "device_code": "device-code"
        }))
        .unwrap_err();
        assert!(device.to_string().contains("realm_id"));

        let provision: ProvisionApiKeyParams = serde_json::from_value(serde_json::json!({
            "access_token": "token"
        }))
        .unwrap();
        assert!(provision.realm_id.is_none());
        assert!(provision.binding_id.is_none());
    }

    #[test]
    fn persisted_auth_mode_resolves_provider_disambiguated_typed_method() {
        // Row 100: the auth-status / create surfaces resolve the persisted mode
        // from the typed `NormalizedAuthMethod` selected by the profile's typed
        // `Provider` — not the provider-agnostic string-guessing shim. An
        // `api_key` method must resolve to `PersistedAuthMode::ApiKey` against
        // the OpenAI matrix.
        let profile = meerkat_core::AuthProfile {
            id: "p".to_string(),
            provider: Provider::OpenAI,
            auth_method: meerkat_core::provider_matrix::OpenAiAuthMethod::ApiKey
                .as_str()
                .to_string(),
            source: CredentialSourceSpec::ManagedStore,
            constraints: meerkat_core::auth::AuthConstraints::default(),
            metadata_defaults: meerkat_core::auth::AuthMetadataDefaults::default(),
        };
        assert_eq!(
            persisted_auth_mode_for_profile(&profile),
            Some(PersistedAuthMode::ApiKey),
            "api_key on the OpenAI provider matrix must resolve to ApiKey"
        );
        let typed = normalized_auth_method(&profile).expect("api_key parses on OpenAI matrix");
        assert!(matches!(
            typed,
            meerkat_providers::NormalizedAuthMethod::OpenAi(_)
        ));
    }

    #[test]
    fn persisted_auth_mode_rejects_method_outside_provider_matrix() {
        // Row 100: an auth_method that is not a member of the profile
        // provider's typed matrix must resolve to `None` — the typed enum is
        // the single owner and refuses to fabricate a mode for a method the
        // provider does not support (fail closed instead of cross-provider
        // string guessing).
        let profile = meerkat_core::AuthProfile {
            id: "p".to_string(),
            provider: Provider::Anthropic,
            auth_method: "azure_api_key".to_string(),
            source: CredentialSourceSpec::ManagedStore,
            constraints: meerkat_core::auth::AuthConstraints::default(),
            metadata_defaults: meerkat_core::auth::AuthMetadataDefaults::default(),
        };
        assert_eq!(
            normalized_auth_method(&profile),
            None,
            "azure_api_key is not in the Anthropic matrix and must not resolve"
        );
        assert_eq!(persisted_auth_mode_for_profile(&profile), None);
    }

    #[tokio::test]
    async fn login_complete_requires_explicit_realm_and_binding() {
        let runtime = test_runtime();
        let params = raw_params(serde_json::json!({
            "provider": "anthropic",
            "code": "code",
            "state": "state",
            "redirect_uri": "http://127.0.0.1:0/callback"
        }));

        let resp =
            handle_auth_login_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert_invalid_params_message(resp, "missing field `realm_id`");
    }

    #[tokio::test]
    async fn login_complete_does_not_fall_through_to_default_binding() {
        let runtime = test_runtime_with_config(config_with_anthropic_default_binding());
        let params = raw_params(serde_json::json!({
            "provider": "anthropic",
            "code": "code",
            "state": "state",
            "redirect_uri": "http://127.0.0.1:0/callback"
        }));

        let resp =
            handle_auth_login_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert_invalid_params_message(resp, "missing field `realm_id`");
    }

    #[tokio::test]
    async fn login_start_records_flow_on_runtime_authority() {
        let runtime = test_runtime_with_config(config_with_openai_oauth_binding(
            CredentialSourceSpec::ManagedStore,
        ));
        let unrelated_runtime = test_runtime_with_config(config_with_openai_oauth_binding(
            CredentialSourceSpec::ManagedStore,
        ));
        let redirect_uri = "http://127.0.0.1:0/callback";
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "redirect_uri": redirect_uri,
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_start(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert!(resp.error.is_none(), "login start error: {:?}", resp.error);
        let result: serde_json::Value =
            serde_json::from_str(resp.result.as_ref().expect("result").get()).unwrap();
        let state = result["state"].as_str().expect("state");
        assert!(matches!(
            unrelated_runtime.oauth_flow_authority().consume(
                state,
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "verify_oauth_browser_flow",
                ..
            })
        ));
        let flow = runtime
            .oauth_flow_authority()
            .consume(
                state,
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("starting runtime owns the flow");
        assert!(!flow.pkce_verifier.is_empty());
    }

    #[tokio::test]
    async fn login_start_records_flow_on_runtime_adapter_authority() {
        let runtime = test_runtime_with_config(config_with_openai_oauth_binding(
            CredentialSourceSpec::ManagedStore,
        ));
        let redirect_uri = "http://127.0.0.1:0/callback";
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "redirect_uri": redirect_uri,
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_start(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert!(resp.error.is_none(), "login start error: {:?}", resp.error);
        let result: serde_json::Value =
            serde_json::from_str(resp.result.as_ref().expect("result").get()).unwrap();
        let state = result["state"].as_str().expect("state");
        let flow = runtime
            .runtime_adapter()
            .oauth_flow_authority()
            .consume(
                state,
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("runtime AuthMachine authority owns the RPC login flow");
        assert!(!flow.pkce_verifier.is_empty());
    }

    /// In-test [`meerkat_core::RealmConfigSource`] backed by an in-memory
    /// realm→doc map, mirroring the filesystem source's per-realm projection
    /// without touching disk. `None` for an absent realm preserves the
    /// absent-ancestor semantics the composition fold relies on.
    struct MapRealmConfigSource {
        docs: std::collections::HashMap<String, meerkat_core::Config>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::RealmConfigSource for MapRealmConfigSource {
        async fn config_for_realm(
            &self,
            realm: &meerkat_core::connection::RealmId,
        ) -> Result<Option<meerkat_core::Config>, meerkat_core::config::ConfigError> {
            Ok(self.docs.get(realm.as_str()).cloned())
        }
    }

    /// Build a runtime whose active realm is `dev` and whose composed config
    /// inherits an OpenAI OAuth binding from the reserved `global` realm — `dev`
    /// itself defines no binding (`parent = global`). The strict-owner write
    /// guard must therefore reject an OAuth login targeting `dev`, naming
    /// `global` as the owner.
    fn test_runtime_with_inherited_global_oauth_binding() -> SessionRuntime {
        // `global` OWNS the binding; reuse the standard oauth section but key it
        // under the reserved `global` realm.
        let mut global_doc = config_with_openai_oauth_binding(CredentialSourceSpec::ManagedStore);
        let global_section = global_doc
            .realm
            .remove("dev")
            .expect("oauth fixture defines a dev section");
        global_doc.realm.insert("global".into(), global_section);

        // `dev` defines NO binding; it only links to `global` as its parent so
        // reads inherit down the chain while writes stay strict-owner.
        let mut dev_doc = meerkat_core::Config::default();
        let dev_section = meerkat_core::RealmConfigSection {
            parent: Some(meerkat_core::connection::RealmId::global()),
            ..Default::default()
        };
        dev_doc.realm.insert("dev".into(), dev_section);

        let source: Arc<dyn meerkat_core::RealmConfigSource> = Arc::new(MapRealmConfigSource {
            docs: std::collections::HashMap::from([
                ("global".to_string(), global_doc),
                ("dev".to_string(), dev_doc.clone()),
            ]),
        });

        // The runtime's raw config_runtime holds the head (`dev`) doc only —
        // composition is what surfaces the inherited binding to the guard.
        let runtime = test_runtime_with_config(dev_doc);
        runtime.set_realm_config_source(source);
        runtime.set_realm_context(
            Some(meerkat_core::connection::RealmId::parse("dev").unwrap()),
            None,
            None,
        );
        runtime
    }

    // RCT-28: an OAuth login targeting a realm that only INHERITS the binding
    // from `global` must be rejected by the strict-owner write guard (naming the
    // owning realm) and must NOT admit any OAuth flow state.
    #[tokio::test]
    async fn login_start_rejects_inherited_binding_naming_owner_without_persisting() {
        let runtime = test_runtime_with_inherited_global_oauth_binding();
        let redirect_uri = "http://127.0.0.1:0/callback";
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "redirect_uri": redirect_uri,
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_start(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let err = resp
            .error
            .expect("inherited-binding login must be rejected");
        assert_eq!(err.code, error::INVALID_PARAMS, "error: {err:?}");
        assert!(
            err.message.contains("global"),
            "rejection must name the owning realm 'global', got: {}",
            err.message
        );
        assert!(
            resp.result.is_none(),
            "rejected login must not return a flow result"
        );

        // The flow authority must hold NO admitted state for the inherited
        // binding — the guard fired before any state token was minted, so the
        // owning-realm binding is unconsumable here.
        let consumed = runtime.oauth_flow_authority().consume(
            "any-state",
            &openai_auth_binding(),
            meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            redirect_uri,
        );
        assert!(
            consumed.is_err(),
            "rejected login must not admit any OAuth flow state"
        );
    }

    #[tokio::test]
    async fn login_start_rejects_same_provider_non_oauth_target_before_state_admission() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "redirect_uri": "http://127.0.0.1:0/callback",
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_start(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert_invalid_params_message(resp, "auth_method 'api_key'");
        let snapshot = runtime
            .auth_lease_handle()
            .snapshot(&LeaseKey::from_auth_binding(&openai_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn login_start_rejects_oauth_method_with_external_source_before_state_admission() {
        let runtime = test_runtime_with_config(config_with_openai_oauth_binding(
            CredentialSourceSpec::ExternalResolver {
                handle: "external-chatgpt".into(),
            },
        ));
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "redirect_uri": "http://127.0.0.1:0/callback",
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_start(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert_invalid_params_message(resp, "source 'external_resolver'");
        let snapshot = runtime
            .auth_lease_handle()
            .snapshot(&LeaseKey::from_auth_binding(&openai_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn login_start_rejects_oauth_method_with_wrong_backend_before_state_admission() {
        let runtime = test_runtime_with_config(config_with_openai_oauth_wrong_backend_binding());
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "redirect_uri": "http://127.0.0.1:0/callback",
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_start(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert_invalid_params_message(resp, "backend_kind 'openai_api'");
        let snapshot = runtime
            .auth_lease_handle()
            .snapshot(&LeaseKey::from_auth_binding(&openai_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn device_start_rejects_same_provider_non_oauth_target_before_state_admission() {
        let runtime = test_runtime_with_config(config_with_google_api_key_binding());
        let params = raw_params(serde_json::json!({
            "provider": "google",
            "realm_id": "dev",
            "binding_id": "default_google"
        }));

        let resp =
            handle_auth_login_device_start(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime)
                .await;

        assert_invalid_params_message(resp, "auth_method 'api_key'");
        let snapshot = runtime
            .auth_lease_handle()
            .snapshot(&LeaseKey::from_auth_binding(&google_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn device_start_rejects_oauth_method_with_wrong_backend_before_state_admission() {
        let runtime = test_runtime_with_config(config_with_google_oauth_wrong_backend_binding());
        let params = raw_params(serde_json::json!({
            "provider": "google",
            "realm_id": "dev",
            "binding_id": "default_google"
        }));

        let resp =
            handle_auth_login_device_start(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime)
                .await;

        assert_invalid_params_message(resp, "backend_kind 'google_genai'");
        let snapshot = runtime
            .auth_lease_handle()
            .snapshot(&LeaseKey::from_auth_binding(&google_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn login_complete_rejects_same_provider_non_oauth_target_before_state_lookup() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "code": "provider-code",
            "state": "missing-state",
            "redirect_uri": "http://127.0.0.1:0/callback",
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert_invalid_params_message(resp, "auth_method 'api_key'");
    }

    #[tokio::test]
    async fn login_complete_rejects_oauth_method_with_wrong_backend_before_state_lookup() {
        let runtime = test_runtime_with_config(config_with_openai_oauth_wrong_backend_binding());
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "code": "provider-code",
            "state": "missing-state",
            "redirect_uri": "http://127.0.0.1:0/callback",
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert_invalid_params_message(resp, "backend_kind 'openai_api'");
    }

    #[tokio::test]
    async fn device_complete_rejects_same_provider_non_oauth_target_before_state_lookup() {
        let runtime = test_runtime_with_config(config_with_google_api_key_binding());
        let params = raw_params(serde_json::json!({
            "provider": "google",
            "device_code": "missing-device-code",
            "realm_id": "dev",
            "binding_id": "default_google"
        }));

        let resp =
            handle_auth_login_device_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime)
                .await;

        assert_invalid_params_message(resp, "auth_method 'api_key'");
    }

    #[tokio::test]
    async fn device_complete_rejects_oauth_method_with_wrong_backend_before_state_lookup() {
        let runtime = test_runtime_with_config(config_with_google_oauth_wrong_backend_binding());
        let params = raw_params(serde_json::json!({
            "provider": "google",
            "device_code": "missing-device-code",
            "realm_id": "dev",
            "binding_id": "default_google"
        }));

        let resp =
            handle_auth_login_device_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime)
                .await;

        assert_invalid_params_message(resp, "backend_kind 'google_genai'");
    }

    #[tokio::test]
    async fn provision_api_key_rejects_same_provider_non_oauth_target_before_token_store() {
        let runtime = test_runtime_with_config_without_provider_auth_persistence(
            config_with_anthropic_api_key_binding(),
        );
        let params = raw_params(serde_json::json!({
            "access_token": "console-access-token",
            "realm_id": "dev",
            "binding_id": "default_anthropic"
        }));

        let resp = handle_auth_login_provision_api_key(
            Some(RpcId::Num(1)),
            Some(params.as_ref()),
            &runtime,
        )
        .await;

        assert_invalid_params_message(resp, "auth_method 'api_key'");
    }

    #[tokio::test]
    async fn provision_api_key_rejects_oauth_method_with_wrong_backend_before_token_store() {
        let runtime = test_runtime_with_config_without_provider_auth_persistence(
            config_with_anthropic_oauth_to_api_key_wrong_backend_binding(),
        );
        let params = raw_params(serde_json::json!({
            "access_token": "console-access-token",
            "realm_id": "dev",
            "binding_id": "default_anthropic"
        }));

        let resp = handle_auth_login_provision_api_key(
            Some(RpcId::Num(1)),
            Some(params.as_ref()),
            &runtime,
        )
        .await;

        assert_invalid_params_message(resp, "backend_kind 'bedrock'");
    }

    #[tokio::test]
    async fn provision_api_key_requires_runtime_oauth_flow_admission_before_token_store()
    -> Result<(), String> {
        let runtime = test_runtime_with_config_without_provider_auth_persistence(
            config_with_anthropic_oauth_to_api_key_binding(),
        );
        let authority = runtime.oauth_flow_authority();
        let mut admitted = 0usize;
        loop {
            match authority.start(
                openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                format!("http://127.0.0.1:{admitted}/callback"),
                format!("verifier-{admitted}"),
            ) {
                Ok(_) => {
                    admitted += 1;
                    assert!(
                        admitted < 2048,
                        "test should reach the default OAuth flow capacity before 2048 starts"
                    );
                }
                Err(OAuthFlowError::LifecycleRejected {
                    operation: "admit_oauth_browser_flow",
                    ..
                }) => break,
                Err(err) => return Err(format!("unexpected OAuth flow admission error: {err}")),
            }
        }
        let params = raw_params(serde_json::json!({
            "access_token": "console-access-token",
            "realm_id": "dev",
            "binding_id": "default_anthropic"
        }));

        let resp = handle_auth_login_provision_api_key(
            Some(RpcId::Num(1)),
            Some(params.as_ref()),
            &runtime,
        )
        .await;

        let error = resp
            .error
            .expect("provision should fail at runtime OAuth flow admission");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("oauth flow lifecycle transition rejected")
                || error
                    .message
                    .contains("oauth state registry is at capacity"),
            "expected runtime OAuth authority failure before TokenStore access, got `{}`",
            error.message
        );
        Ok(())
    }

    #[tokio::test]
    async fn browser_login_token_store_preflight_failure_does_not_consume_state() {
        let runtime = test_runtime_with_config_without_provider_auth_persistence(
            config_with_openai_oauth_binding(CredentialSourceSpec::ManagedStore),
        );
        let redirect_uri = "http://127.0.0.1:0/callback";
        let state = runtime
            .oauth_flow_authority()
            .start(
                openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "verifier".to_string(),
            )
            .expect("state generation succeeds");
        let params = raw_params(serde_json::json!({
            "provider": "openai",
            "code": "provider-code",
            "state": state.clone(),
            "redirect_uri": redirect_uri,
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_login_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let error = resp
            .error
            .expect("missing provider auth persistence should fail");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("provider auth persistence is not configured")
        );
        let flow = runtime
            .oauth_flow_authority()
            .consume(
                &state,
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("local preflight failure must leave browser state retryable");
        assert_eq!(flow.pkce_verifier, "verifier");
    }

    #[tokio::test]
    async fn device_completion_poll_drop_releases_runtime_authority() {
        let runtime = test_runtime();
        runtime
            .oauth_flow_authority()
            .admit_device_code(
                openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                "device-code".to_string(),
                std::time::Duration::from_secs(600),
            )
            .expect("device code admitted");

        let poll = runtime
            .oauth_flow_authority()
            .begin_device_code_poll(
                "device-code",
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("device completion poll begins");
        assert!(matches!(
            runtime.oauth_flow_authority().begin_device_code_poll(
                "device-code",
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "begin_oauth_device_poll",
                ..
            })
        ));

        drop(poll);

        runtime
            .oauth_flow_authority()
            .begin_device_code_poll(
                "device-code",
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("dropped RPC completion poll releases runtime authority");
    }

    #[tokio::test]
    async fn device_completion_poll_abort_releases_runtime_authority() {
        let runtime = test_runtime();
        runtime
            .oauth_flow_authority()
            .admit_device_code(
                openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                "device-code".to_string(),
                std::time::Duration::from_secs(600),
            )
            .expect("device code admitted");

        let authority = runtime.oauth_flow_authority();
        let (begun_tx, begun_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _poll = authority
                .begin_device_code_poll(
                    "device-code",
                    &openai_auth_binding(),
                    meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                )
                .expect("device completion poll begins");
            begun_tx.send(()).expect("signal poll start");
            std::future::pending::<()>().await;
        });
        begun_rx.await.expect("poll lease was acquired");
        assert!(matches!(
            runtime.oauth_flow_authority().begin_device_code_poll(
                "device-code",
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "begin_oauth_device_poll",
                ..
            })
        ));

        task.abort();
        let _ = task.await;

        runtime
            .oauth_flow_authority()
            .begin_device_code_poll(
                "device-code",
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("aborted RPC completion poll releases runtime authority");
    }

    #[tokio::test]
    async fn device_complete_requires_explicit_realm_and_binding() {
        let runtime = test_runtime();
        let params = raw_params(serde_json::json!({
            "provider": "anthropic",
            "device_code": "device-code"
        }));

        let resp =
            handle_auth_login_device_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime)
                .await;

        assert_invalid_params_message(resp, "missing field `realm_id`");
    }

    #[tokio::test]
    async fn provision_api_key_requires_explicit_realm_and_binding() {
        let runtime = test_runtime();
        let params = raw_params(serde_json::json!({
            "access_token": "token"
        }));

        let resp = handle_auth_login_provision_api_key(
            Some(RpcId::Num(1)),
            Some(params.as_ref()),
            &runtime,
        )
        .await;

        assert_invalid_params_message(resp, "realm_id is required for OAuth login completion");
    }

    #[tokio::test]
    async fn auth_status_ignores_stale_token_without_auth_machine_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &PersistedTokens {
                    auth_mode: PersistedAuthMode::ApiKey,
                    primary_secret: Some("sk-stale".into()),
                    refresh_token: Some("refresh-stale".into()),
                    id_token: None,
                    expires_at: Some(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap()),
                    last_refresh: Some(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
                    scopes: Vec::new(),
                    account_id: Some("acct-stale".into()),
                    metadata: serde_json::Value::Null,
                },
            )
            .await
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_status_get(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let status = auth_status_value(resp);
        assert_eq!(status["state"], "missing_credential");
        assert!(status.get("expires_at").is_none());
        assert!(status.get("last_refresh_at").is_none());
        assert!(status.get("account_id").is_none());
        assert_eq!(status["has_refresh_token"], false);
    }

    #[tokio::test]
    async fn auth_status_rehydrates_marked_oauth_token_after_restart() {
        let runtime = test_runtime_with_config(config_with_openai_oauth_binding(
            CredentialSourceSpec::ManagedStore,
        ));
        let auth_binding = openai_auth_binding();
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let tokens = chatgpt_oauth_tokens_with_secret("fresh-chatgpt-access");
        store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &mark_tokens_lifecycle_published_for_test(&tokens, 1, None),
            )
            .await
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_status_get(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let status = auth_status_value(resp);
        assert_eq!(status["state"], "valid");
        assert!(status.get("expires_at").is_some());
        assert_eq!(status["account_id"], "acct-1");
        assert_eq!(status["has_refresh_token"], true);
        let snapshot = runtime.auth_lease_handle().snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        assert!(snapshot.credential_present);
    }

    #[tokio::test]
    async fn auth_status_reports_lease_phase_when_token_is_missing() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_status_get(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let status = auth_status_value(resp);
        assert_eq!(status["state"], "valid");
        assert!(status.get("expires_at").is_some());
        assert_eq!(status["has_refresh_token"], false);
    }

    #[tokio::test]
    async fn auth_status_hides_wrong_mode_token_without_hiding_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_oauth_binding(
            CredentialSourceSpec::ManagedStore,
        ));
        let auth_binding = openai_auth_binding();
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &PersistedTokens::api_key("sk-stale-api-key"),
            )
            .await
            .unwrap();
        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_status_get(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let status = auth_status_value(resp);
        assert_eq!(status["state"], "valid");
        assert!(status.get("expires_at").is_some());
        assert!(status.get("last_refresh_at").is_none());
        assert!(status.get("account_id").is_none());
        assert_eq!(status["has_refresh_token"], false);
    }

    #[tokio::test]
    async fn auth_status_hides_wrong_source_oauth_token_without_hiding_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_oauth_binding(
            CredentialSourceSpec::ExternalResolver {
                handle: "external-chatgpt".into(),
            },
        ));
        let auth_binding = openai_auth_binding();
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &PersistedTokens {
                    auth_mode: PersistedAuthMode::ChatgptOauth,
                    primary_secret: Some("fresh-chatgpt-access".into()),
                    refresh_token: Some("rt".into()),
                    id_token: None,
                    expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                    last_refresh: Some(chrono::Utc::now()),
                    scopes: Vec::new(),
                    account_id: Some("acct-stale".into()),
                    metadata: serde_json::Value::Null,
                },
            )
            .await
            .unwrap();
        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_status_get(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let status = auth_status_value(resp);
        assert_eq!(status["state"], "valid");
        assert!(status.get("expires_at").is_some());
        assert!(status.get("last_refresh_at").is_none());
        assert!(status.get("account_id").is_none());
        assert_eq!(status["has_refresh_token"], false);
    }

    #[tokio::test]
    async fn auth_status_ignores_stale_token_for_non_persisted_source_without_hiding_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_external_authorizer_binding());
        let auth_binding = openai_auth_binding();
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &PersistedTokens {
                    auth_mode: PersistedAuthMode::ApiKey,
                    primary_secret: Some("sk-stale".into()),
                    refresh_token: Some("refresh-stale".into()),
                    id_token: None,
                    expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                    last_refresh: Some(chrono::Utc::now()),
                    scopes: Vec::new(),
                    account_id: Some("acct-stale".into()),
                    metadata: serde_json::Value::Null,
                },
            )
            .await
            .unwrap();
        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_status_get(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let status = auth_status_value(resp);
        assert_eq!(status["state"], "valid");
        assert!(status.get("expires_at").is_some());
        assert!(status.get("last_refresh_at").is_none());
        assert!(status.get("account_id").is_none());
        assert_eq!(status["has_refresh_token"], false);
    }

    #[tokio::test]
    async fn auth_status_fails_closed_when_token_load_fails() {
        let runtime = test_runtime_with_config_and_token_store(
            config_with_openai_managed_store_binding(),
            Arc::new(LoadFailingTokenStore),
        );
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp =
            handle_auth_status_get(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        // A token-store load fault is auth truth the surface cannot vouch
        // for: it must propagate as a typed failure, never be laundered into
        // a lease-phase success.
        let error = resp.error.expect("token load fault must fail closed");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error.message.contains("TokenStore load failed"),
            "error must carry the token-store fault, got: {}",
            error.message
        );
        assert!(
            resp.result.is_none(),
            "fail-closed status must not carry a result"
        );
    }

    #[tokio::test]
    async fn auth_profile_create_publishes_auth_machine_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai",
            "auth_method": "api_key",
            "secret": "sk-test"
        }));

        let create =
            handle_auth_profile_create(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;
        assert!(create.error.is_none(), "create error: {:?}", create.error);

        let status =
            handle_auth_status_get(Some(RpcId::Num(2)), Some(params.as_ref()), &runtime).await;
        assert_eq!(auth_status_state(status), "valid");
    }

    #[tokio::test]
    async fn ready_device_consume_failure_does_not_commit_token() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_lease = runtime.generated_auth_lease_handle();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let registry = meerkat_providers::oauth_flow::OAuthFlowRegistry::new(
            std::time::Duration::from_secs(600),
        );
        registry
            .admit_device_code(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                "device-code".to_string(),
                std::time::Duration::from_secs(600),
            )
            .expect("device code admitted");
        let poll_lease = registry
            .begin_device_code_poll(
                "device-code",
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("device poll lease begins")
            .with_lifecycle(Arc::new(RejectDeviceConsumeLifecycle));

        let resp = save_tokens_and_consume_device_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            poll_lease,
        )
        .await
        .expect("consume failure returns an RPC response");

        let error = resp.error.expect("consume should fail");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(error.message.contains("consume_oauth_device_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn browser_consume_failure_does_not_commit_token() {
        let temp = tempfile::tempdir().unwrap();
        let file_store: Arc<dyn TokenStore> =
            Arc::new(FileTokenStore::new(temp.path().join("tokens")));
        let runtime = test_runtime_with_config_and_token_store(
            config_with_openai_managed_store_binding(),
            file_store,
        );
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_lease = runtime.auth_lease_handle();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let authority: Arc<dyn meerkat_providers::oauth_flow::OAuthFlowAuthority> =
            Arc::new(RejectBrowserConsumeAuthority);

        let resp = save_tokens_and_consume_browser_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            OwnedBrowserFlowConsume {
                authority,
                state: "browser-state".to_string(),
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback".to_string(),
            },
        )
        .await
        .expect_err("consume should fail");

        let error = resp.error.expect("consume should return RPC error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(error.message.contains("consume_oauth_browser_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn browser_consume_failure_does_not_save_before_durable_claim() {
        let counting_store = Arc::new(SaveCountingTokenStore::new());
        let store: Arc<dyn TokenStore> = counting_store.clone();
        let runtime = test_runtime_with_config_and_token_store(
            config_with_openai_managed_store_binding(),
            store.clone(),
        );
        let auth_lease = runtime.auth_lease_handle();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };

        let resp = save_tokens_and_consume_browser_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            OwnedBrowserFlowConsume {
                authority: Arc::new(RejectBrowserConsumeAuthority),
                state: "browser-state".to_string(),
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback".to_string(),
            },
        )
        .await
        .expect_err("consume should fail");

        let error = resp.error.expect("consume should return RPC error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(error.message.contains("consume_oauth_browser_flow"));
        assert_eq!(
            counting_store.save_count(),
            0,
            "token material must not be saved before winning the durable OAuth consume claim"
        );
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn token_commit_is_single_marked_save() {
        // Acquire-first vault property: exactly one durable write, and that
        // write already carries the lifecycle marker. No unmarked token bytes
        // ever reach the store.
        let counting_store = Arc::new(SaveCountingTokenStore::new());
        let store: Arc<dyn TokenStore> = counting_store.clone();
        let runtime = test_runtime_with_config_and_token_store(
            config_with_openai_managed_store_binding(),
            store.clone(),
        );
        let auth_lease = runtime.generated_auth_lease_handle();
        let auth_binding = openai_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);

        save_tokens_and_publish_lifecycle(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
        )
        .await
        .expect("acquire-first commit succeeds");

        assert_eq!(
            counting_store.save_count(),
            1,
            "acquire-first commit must persist the credential in a single marked save"
        );
        let stored = store.load(&key).await.unwrap().unwrap();
        assert!(
            meerkat_core::tokens_lifecycle_published(&stored),
            "the only durable write must already carry the proof-of-acquisition marker"
        );
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
    }

    #[tokio::test]
    async fn save_failure_after_acquire_releases_lease() {
        // Acquire-first crash shape: the AuthMachine lease acquisition
        // succeeds, the durable save fails, the surface returns the error and
        // the acquired lease is rolled back — no half-state survives.
        let store: Arc<dyn TokenStore> = Arc::new(SaveFailingTokenStore);
        let runtime = test_runtime_with_config_and_token_store(
            config_with_openai_managed_store_binding(),
            store.clone(),
        );
        let auth_lease = runtime.generated_auth_lease_handle();
        let auth_binding = openai_auth_binding();

        let resp = save_tokens_and_publish_lifecycle(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
        )
        .await
        .expect_err("durable save failure must surface");

        let error = resp.error.expect("save failure should return RPC error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error.message.contains("TokenStore save failed"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("acquired lease rolled back"),
            "{}",
            error.message
        );
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(
            snapshot.phase, None,
            "save failure after acquire must release the freshly acquired lease"
        );
        assert!(!snapshot.credential_present);
    }

    #[tokio::test]
    async fn raw_registry_browser_success_cannot_commit_tokens() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_lease = runtime.auth_lease_handle();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        let registry = Arc::new(meerkat_providers::oauth_flow::OAuthFlowRegistry::new(
            std::time::Duration::from_secs(600),
        ));
        let state = registry
            .start(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "registry-only-verifier".to_string(),
            )
            .expect("raw registry admits browser flow");

        let resp = save_tokens_and_consume_browser_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            OwnedBrowserFlowConsume {
                authority: registry.clone(),
                state: state.clone(),
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: redirect_uri.to_string(),
            },
        )
        .await
        .expect_err("raw registry must not commit tokens");

        let error = resp.error.expect("consume should return RPC error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("AuthMachine-owned OAuth flow authority")
        );
        assert!(store.load(&key).await.unwrap().is_none());
        assert_eq!(
            auth_lease
                .snapshot(&LeaseKey::from_auth_binding(&auth_binding))
                .phase,
            None
        );
        registry
            .verify(
                &state,
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("raw registry success path is inert and remains unconsumed");
    }

    #[tokio::test]
    async fn raw_registry_device_success_cannot_commit_tokens() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_lease = runtime.auth_lease_handle();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let registry = meerkat_providers::oauth_flow::OAuthFlowRegistry::new(
            std::time::Duration::from_secs(600),
        );
        registry
            .admit_device_code(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                "registry-only-device-code".to_string(),
                std::time::Duration::from_secs(600),
            )
            .expect("raw registry admits device flow");
        let poll_lease = registry
            .begin_device_code_poll(
                "registry-only-device-code",
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("raw registry begins device poll");

        let resp = save_tokens_and_consume_device_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            poll_lease,
        )
        .await
        .expect("raw registry must not commit tokens");

        let error = resp.error.expect("consume should return RPC error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("AuthMachine-owned OAuth device poll lease")
        );
        assert!(store.load(&key).await.unwrap().is_none());
        assert_eq!(
            auth_lease
                .snapshot(&LeaseKey::from_auth_binding(&auth_binding))
                .phase,
            None
        );
        registry
            .verify_device_code(
                "registry-only-device-code",
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("raw registry device success path is inert and remains unconsumed");
    }

    #[tokio::test]
    async fn browser_consume_failure_does_not_reauthorize_stale_previous_tokens() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_lease = runtime.auth_lease_handle();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        store
            .save(&key, &PersistedTokens::api_key("sk-stale"))
            .await
            .unwrap();

        let resp = save_tokens_and_consume_browser_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            OwnedBrowserFlowConsume {
                authority: Arc::new(RejectBrowserConsumeAuthority),
                state: "browser-state".to_string(),
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback".to_string(),
            },
        )
        .await
        .expect_err("consume should fail");

        let error = resp.error.expect("consume should return RPC error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(error.message.contains("consume_oauth_browser_flow"));
        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-stale"));
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
        let projection = meerkat_core::project_published_auth_status(
            chrono::Utc::now(),
            Some(&stored),
            &snapshot,
        );
        assert_eq!(
            projection.phase,
            meerkat_core::AuthStatusPhase::MissingCredential
        );
        assert!(projection.tokens.is_none());
    }

    #[tokio::test]
    async fn stale_previous_rollback_preserves_newer_oauth_flow() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_lease = runtime.generated_auth_lease_handle();
        let authority = runtime.oauth_flow_authority();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        store
            .save(&key, &PersistedTokens::api_key("sk-stale"))
            .await
            .unwrap();
        let mut failed_tokens = PersistedTokens::api_key("sk-new");
        failed_tokens.expires_at =
            Some(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap());
        let commit = save_tokens_and_publish_lifecycle_commit_unlocked(
            Some(RpcId::Num(1)),
            &store,
            &auth_lease,
            &auth_binding,
            &failed_tokens,
        )
        .await
        .unwrap();
        let state = authority
            .start(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "new-verifier".to_string(),
            )
            .expect("newer OAuth flow admitted after rollback snapshot");

        rollback_token_commit(store.as_ref(), &auth_lease, &commit)
            .await
            .expect("rollback clears credential lifecycle without clobbering OAuth flow");

        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-stale"));
        let record = authority
            .verify(
                &state,
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("newer OAuth flow remains authoritative");
        assert_eq!(record.pkce_verifier, "new-verifier");
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::ReauthRequired));
        assert_eq!(snapshot.expires_at, None);
        let projection = meerkat_core::project_published_auth_status(
            chrono::Utc::now(),
            Some(&stored),
            &snapshot,
        );
        assert_eq!(
            projection.phase,
            meerkat_core::AuthStatusPhase::MissingCredential
        );
        assert!(projection.tokens.is_none());
    }

    #[tokio::test]
    async fn oauth_only_previous_rollback_does_not_reauthorize_stale_tokens() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_lease = runtime.generated_auth_lease_handle();
        let authority = runtime.oauth_flow_authority();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        store
            .save(&key, &PersistedTokens::api_key("sk-stale"))
            .await
            .unwrap();
        let state = authority
            .start(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "existing-flow-verifier".to_string(),
            )
            .expect("existing OAuth flow admitted before rollback snapshot");
        let previous_snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(
            previous_snapshot.phase,
            Some(AuthLeasePhase::ReauthRequired)
        );
        assert!(!previous_snapshot.credential_present);

        let mut failed_tokens = PersistedTokens::api_key("sk-new");
        failed_tokens.expires_at =
            Some(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap());
        let commit = save_tokens_and_publish_lifecycle_commit_unlocked(
            Some(RpcId::Num(1)),
            &store,
            &auth_lease,
            &auth_binding,
            &failed_tokens,
        )
        .await
        .unwrap();

        rollback_token_commit(store.as_ref(), &auth_lease, &commit)
            .await
            .expect("rollback restores token bytes without restoring credential authority");

        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-stale"));
        let record = authority
            .verify(
                &state,
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("existing OAuth flow remains authoritative");
        assert_eq!(record.pkce_verifier, "existing-flow-verifier");
        let snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::ReauthRequired));
        assert!(!snapshot.credential_present);
        let projection = meerkat_core::project_published_auth_status(
            chrono::Utc::now(),
            Some(&stored),
            &snapshot,
        );
        assert_eq!(
            projection.phase,
            meerkat_core::AuthStatusPhase::MissingCredential
        );
        assert!(projection.tokens.is_none());
    }

    #[tokio::test]
    async fn real_device_consume_failure_releases_credential_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        runtime
            .runtime_adapter()
            .set_runtime_auth_lease_handle(Arc::clone(&auth_lease));
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let authority = runtime.oauth_flow_authority();
        authority
            .admit_device_code(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                "device-code".to_string(),
                std::time::Duration::from_secs(600),
            )
            .expect("device flow admitted by runtime authority");
        let poll_lease = authority
            .begin_device_code_poll(
                "device-code",
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("device poll lease begins through runtime authority");
        meerkat_providers::oauth_flow::OAuthDevicePollLifecycle::expire_device_flow(
            auth_lease.as_ref(),
            &auth_binding,
            "device-code",
        )
        .expect("test removes the AuthMachine flow membership");

        let resp = save_tokens_and_consume_device_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            poll_lease,
        )
        .await
        .expect("consume failure returns an RPC response");

        let error = resp.error.expect("consume should fail");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(error.message.contains("consume_oauth_device_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn real_browser_missing_consume_releases_credential_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        runtime
            .runtime_adapter()
            .set_runtime_auth_lease_handle(Arc::clone(&auth_lease));
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        let authority = runtime.oauth_flow_authority();
        let state = authority
            .start(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "verifier".to_string(),
            )
            .expect("browser flow admitted by runtime authority");
        authority
            .verify(
                &state,
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("browser flow verifies through runtime authority");
        authority
            .consume(
                &state,
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("test pre-consumes the browser flow");

        let resp = save_tokens_and_consume_browser_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            OwnedBrowserFlowConsume {
                authority: Arc::clone(&authority),
                state: state.clone(),
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: redirect_uri.to_string(),
            },
        )
        .await
        .expect_err("consume should fail");

        let error = resp.error.expect("consume should return RPC error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("oauth state terminal consume failed")
        );
        assert!(error.message.contains("verify_oauth_browser_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn oauth_to_api_key_browser_consume_marks_current_lifecycle_snapshot() {
        let runtime = test_runtime_with_config(config_with_anthropic_oauth_to_api_key_binding());
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_lease = runtime.auth_lease_handle();
        let authority = runtime.oauth_flow_authority();
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_anthropic").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        let state = authority
            .start(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::AnthropicConsoleApiKey,
                redirect_uri.to_string(),
                "console-verifier".to_string(),
            )
            .expect("console OAuth flow admitted");
        let tokens = oauth_to_api_key_tokens_with_secret("sk-ant-api03-provisioned");

        save_tokens_and_consume_browser_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &tokens,
            OwnedBrowserFlowConsume {
                authority: Arc::clone(&authority),
                state: state.clone(),
                provider:
                    meerkat_providers::oauth_flow::OAuthProviderIdentity::AnthropicConsoleApiKey,
                redirect_uri: redirect_uri.to_string(),
            },
        )
        .await
        .expect("provisioned API key commit should consume its OAuth flow");

        let stored = store.load(&key).await.unwrap().unwrap();
        let marker = meerkat_core::tokens_lifecycle_publication(&stored)
            .expect("committed token should have a lifecycle marker");
        let snapshot = auth_lease.snapshot(&lease_key);
        assert_eq!(marker.generation, Some(snapshot.generation));
        assert_eq!(
            marker.credential_published_at_millis,
            snapshot.credential_published_at_millis
        );
        let projection = meerkat_core::project_published_auth_status(
            chrono::Utc::now(),
            Some(&stored),
            &snapshot,
        );
        assert_eq!(projection.phase, meerkat_core::AuthStatusPhase::Valid);
        assert!(projection.tokens.is_some());
    }

    #[tokio::test]
    async fn terminal_consume_failure_preserves_other_browser_flow() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        runtime
            .runtime_adapter()
            .set_runtime_auth_lease_handle(Arc::clone(&auth_lease));
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        let authority = runtime.oauth_flow_authority();
        let old_state = authority
            .start(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "old-verifier".to_string(),
            )
            .expect("old browser flow admitted");
        let new_state = authority
            .start(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "new-verifier".to_string(),
            )
            .expect("new browser flow admitted");
        let rejecting: Arc<dyn meerkat_providers::oauth_flow::OAuthFlowAuthority> =
            Arc::new(RejectBrowserConsumeAuthority);

        let resp = save_tokens_and_consume_browser_flow(
            Some(RpcId::Num(1)),
            &runtime,
            &auth_binding,
            &PersistedTokens::api_key("sk-new"),
            OwnedBrowserFlowConsume {
                authority: rejecting,
                state: old_state.clone(),
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: redirect_uri.to_string(),
            },
        )
        .await
        .expect_err("consume should fail");

        let error = resp.error.expect("consume should return RPC error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(error.message.contains("consume_oauth_browser_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::ReauthRequired));
        let record = authority
            .verify(
                &new_state,
                &auth_binding,
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("rollback preserves other admitted browser flow");
        assert_eq!(record.pkce_verifier, "new-verifier");
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::ReauthRequired));
    }

    #[tokio::test]
    async fn auth_status_observes_session_runtime_binding_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let auth_binding = AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));
        let store = runtime
            .token_store()
            .expect("token store open")
            .expect("test token store");
        store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &PersistedTokens::api_key("sk-live"),
            )
            .await
            .unwrap();

        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();
        let status =
            handle_auth_status_get(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;
        assert_eq!(auth_status_state(status), "valid");

        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings.auth_lease().begin_refresh(&lease_key).unwrap();
        let status =
            handle_auth_status_get(Some(RpcId::Num(2)), Some(params.as_ref()), &runtime).await;
        assert_eq!(auth_status_state(status), "expiring");

        bindings
            .auth_lease()
            .complete_refresh(&lease_key, now + 7200, now)
            .unwrap();
        bindings
            .auth_lease()
            .mark_reauth_required(&lease_key)
            .unwrap();
        let status =
            handle_auth_status_get(Some(RpcId::Num(3)), Some(params.as_ref()), &runtime).await;
        assert_eq!(auth_status_state(status), "reauth_required");
    }

    #[tokio::test]
    async fn oauth_completion_requires_binding_identity() {
        let runtime = test_runtime();
        let params = raw_params(serde_json::json!({
            "provider": "anthropic",
            "code": "code",
            "state": "state",
            "redirect_uri": "http://127.0.0.1:0/callback",
            "realm_id": "dev"
        }));

        let resp =
            handle_auth_login_complete(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        assert_invalid_params_message(resp, "missing field `binding_id`");
    }
}
