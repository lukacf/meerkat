//! REST endpoints for `/auth/*` + `/realms*`.
//!
//! Read-side endpoints resolve against the active `Config.realm` map.
//! Write-side endpoints (login start / complete, profile delete,
//! logout) use the shared `AppState.token_store` so persisted
//! credentials are visible to the AgentFactory's `resolve_binding`
//! path.
//!
//! OAuth state, PKCE verifier, and device-code correlation is owned
//! server-side by the runtime-scoped OAuth flow authority; complete consumes
//! terminal flow state through that authority.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use meerkat_contracts::{
    WireAuthProfile, WireAuthProfileCleared, WireAuthProfileCreated, WireAuthProfileDetail,
    WireAuthProfilesList, WireAuthStatusDetail, WireBackendProfile, WireBindingIdentity,
    WireDeviceStart, WireLoginReady, WireLoginStart, WireProviderBinding, WireRealmConnectionSet,
    WireRealmList, WireRealmSummary,
};
use meerkat_core::connection::{BindingId, ConnectionTargetError, ProfileId, RealmId};
use meerkat_core::handles::LeaseKey;
use meerkat_core::{
    AuthBindingRef, CredentialSourceSpec, Provider, RealmConnectionSet, ResolvedConnectionTarget,
};
use meerkat_providers::auth_oauth::{
    DevicePollOutcome, OAuthError, PkcePair, exchange_authorization_code_with_state,
    poll_device_code, request_device_code,
};
use meerkat_providers::auth_store::{
    PersistedAuthMode, PersistedTokens, TokenKey, TokenStore,
    credential_source_uses_persisted_store, persisted_auth_mode_for_auth_method,
    persisted_auth_mode_is_oauth_login,
};
use meerkat_providers::oauth_flow::{
    OAuthDevicePollLease, OAuthFlowError, resolve_oauth_provider, validate_oauth_login_binding,
};

use crate::AppState;

async fn load_config(state: &AppState) -> Result<meerkat_core::Config, (StatusCode, String)> {
    state
        .config_runtime
        .get()
        .await
        .map(|snap| snap.config)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load config: {e}"),
            )
        })
}

async fn resolve_realm(
    state: &AppState,
    realm_id: &RealmId,
) -> Result<RealmConnectionSet, (StatusCode, String)> {
    let config = load_config(state).await?;
    let section = config
        .realm
        .get(realm_id.as_str())
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Unknown realm: {realm_id}")))?;
    RealmConnectionSet::from_config(realm_id.as_str(), section).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Realm config invalid: {e}"),
        )
    })
}

async fn resolve_binding_identity(
    state: &AppState,
    realm_id: &RealmId,
    binding_id: &BindingId,
    profile_id: Option<&ProfileId>,
) -> Result<
    (
        AuthBindingRef,
        meerkat_core::ProviderBinding,
        meerkat_core::AuthProfile,
    ),
    (StatusCode, String),
> {
    let realm = resolve_realm(state, realm_id).await?;
    let auth_binding = AuthBindingRef {
        realm: realm_id.clone(),
        binding: binding_id.clone(),
        profile: profile_id.cloned(),
    };
    let (binding, _, auth_profile) = realm.lookup_auth_binding(&auth_binding).map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            format!("Unknown auth identity {realm_id}:{binding_id}: {e}"),
        )
    })?;
    Ok((auth_binding, binding.clone(), auth_profile.clone()))
}

fn target_error_status(error: &ConnectionTargetError) -> StatusCode {
    match error {
        ConnectionTargetError::UnknownRealm(_)
        | ConnectionTargetError::MissingDefaultBinding { .. }
        | ConnectionTargetError::BindingInvalid { .. } => StatusCode::NOT_FOUND,
        ConnectionTargetError::RealmConfigInvalid { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        ConnectionTargetError::MissingRealm
        | ConnectionTargetError::InvalidRealmId { .. }
        | ConnectionTargetError::InvalidBindingId { .. }
        | ConnectionTargetError::ProviderMismatch { .. } => StatusCode::BAD_REQUEST,
    }
}

async fn resolve_oauth_target(
    state: &AppState,
    provider: Provider,
    realm_id: Option<&RealmId>,
    binding_id: Option<&BindingId>,
    profile_id: Option<&ProfileId>,
) -> Result<ResolvedConnectionTarget, (StatusCode, String)> {
    let config = load_config(state).await?;
    meerkat_core::resolve_realm_binding_target_for_provider(
        &config, provider, realm_id, binding_id, profile_id, None, false,
    )
    .map_err(|error| (target_error_status(&error), error.to_string()))
}

fn require_managed_store_source(
    binding_id: &BindingId,
    auth_profile: &meerkat_core::AuthProfile,
) -> Result<(), (StatusCode, String)> {
    if matches!(&auth_profile.source, CredentialSourceSpec::ManagedStore) {
        return Ok(());
    }
    Err((
        StatusCode::BAD_REQUEST,
        format!(
            "binding {binding_id} resolves auth profile '{}' with source '{}'; \
             POST /auth/profiles can only persist credentials for source.kind = 'managed_store'",
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

fn oauth_device_state_error(err: OAuthFlowError) -> (StatusCode, String) {
    match err {
        OAuthFlowError::Missing => (
            StatusCode::BAD_REQUEST,
            "oauth device code is missing or expired".to_string(),
        ),
        OAuthFlowError::RegistryProjectionMissing { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("oauth device registry projection failed: {err}"),
        ),
        other => (
            StatusCode::BAD_REQUEST,
            format!("oauth device state verification failed: {other}"),
        ),
    }
}

fn oauth_terminal_device_consume_error(err: OAuthFlowError) -> (StatusCode, String) {
    match err {
        OAuthFlowError::LifecycleRejected { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("oauth device terminal consume failed: {err}"),
        ),
        other => oauth_device_state_error(other),
    }
}

fn release_uncredentialed_terminal_oauth_lifecycle(
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) {
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    if !auth_lease.snapshot(&lease_key).credential_present {
        let _ = auth_lease.release_credential_lifecycle(&lease_key);
    }
}

fn consume_terminal_device_flow(
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    poll_lease: OAuthDevicePollLease,
) -> Result<(), (StatusCode, String)> {
    poll_lease.consume().map(|_| ()).map_err(|err| {
        release_uncredentialed_terminal_oauth_lifecycle(auth_lease, auth_binding);
        oauth_terminal_device_consume_error(err)
    })
}

fn finish_device_flow_poll(poll_lease: OAuthDevicePollLease) -> Result<(), (StatusCode, String)> {
    poll_lease.finish().map_err(oauth_device_state_error)
}

fn verify_terminal_device_flow(
    poll_lease: &OAuthDevicePollLease,
) -> Result<(), (StatusCode, String)> {
    poll_lease
        .verify()
        .map(|_| ())
        .map_err(oauth_device_state_error)
}

struct PreparedTokenCommitSnapshot {
    key: TokenKey,
    lease_key: meerkat_core::handles::LeaseKey,
    previous: Option<PersistedTokens>,
}

struct TokenCommitSnapshot {
    key: TokenKey,
    lease_key: meerkat_core::handles::LeaseKey,
    previous: Option<PersistedTokens>,
    previous_lifecycle: meerkat_core::handles::AuthLeaseSnapshot,
    lifecycle_transition: meerkat_core::handles::AuthLeaseTransition,
}

async fn prepare_token_commit_unlocked(
    token_store: &dyn TokenStore,
    auth_binding: &AuthBindingRef,
) -> Result<PreparedTokenCommitSnapshot, (StatusCode, String)> {
    let key = TokenKey::from_auth_binding(auth_binding);
    let previous = token_store.load(&key).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("TokenStore load failed: {e}"),
        )
    })?;
    Ok(PreparedTokenCommitSnapshot {
        key,
        lease_key: meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding),
        previous,
    })
}

async fn save_tokens_and_publish_lifecycle_commit_unlocked(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    mark_for_rehydration: bool,
) -> Result<TokenCommitSnapshot, (StatusCode, String)> {
    let key = TokenKey::from_auth_binding(auth_binding);
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    let previous_lifecycle = auth_lease.snapshot(&lease_key);
    let previous = token_store.load(&key).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("TokenStore load failed: {e}"),
        )
    })?;
    token_store.save(&key, tokens).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("TokenStore save failed: {e}"),
        )
    })?;
    let transition = match meerkat_core::publish_token_lifecycle_acquired(
        auth_lease,
        auth_binding,
        tokens,
    ) {
        Ok(transition) => transition,
        Err(e) => {
            if let Err(rollback_error) =
                restore_tokens_after_lifecycle_failure(token_store, &key, previous.as_ref()).await
            {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "AuthMachine lifecycle acquire failed: {e}; TokenStore rollback failed: {rollback_error}"
                    ),
                ));
            }
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("AuthMachine lifecycle acquire failed: {e}"),
            ));
        }
    };
    let commit = TokenCommitSnapshot {
        key,
        lease_key,
        previous,
        previous_lifecycle,
        lifecycle_transition: transition,
    };
    if mark_for_rehydration {
        mark_token_commit_lifecycle_published_unlocked(token_store, auth_lease, &commit, tokens)
            .await?;
    }
    Ok(commit)
}

async fn mark_token_commit_lifecycle_published_unlocked(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    commit: &TokenCommitSnapshot,
    tokens: &PersistedTokens,
) -> Result<(), (StatusCode, String)> {
    let current_lifecycle = auth_lease.snapshot(&commit.lease_key);
    let committed_tokens = if current_lifecycle.credential_present {
        meerkat_core::mark_tokens_lifecycle_published_for_snapshot(tokens, &current_lifecycle)
    } else {
        meerkat_core::mark_tokens_lifecycle_published_for_transition(
            tokens,
            commit.lifecycle_transition,
        )
    };
    if let Err(e) = token_store.save(&commit.key, &committed_tokens).await {
        let message = match rollback_token_commit(token_store, auth_lease, commit).await {
            Ok(()) => {
                format!("TokenStore lifecycle marker save failed: {e}; token commit rolled back")
            }
            Err(rollback_error) => {
                format!(
                    "TokenStore lifecycle marker save failed: {e}; token commit rollback failed: {rollback_error}"
                )
            }
        };
        return Err((StatusCode::INTERNAL_SERVER_ERROR, message));
    }
    Ok(())
}

fn publish_resolved_auth_lease(
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    connection: &meerkat_providers::ResolvedConnection,
) -> Result<(), meerkat_core::handles::DslTransitionError> {
    if matches!(
        connection.auth_lease.kind(),
        meerkat_core::ResolvedAuthKind::None
    ) {
        return Ok(());
    }
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    if auth_lease.snapshot(&lease_key).credential_present {
        return Ok(());
    }
    let expires_at = connection
        .auth_lease
        .expires_at()
        .map(|ts| ts.timestamp().max(0) as u64)
        .unwrap_or(u64::MAX);
    auth_lease.acquire_lease(&lease_key, expires_at)?;
    Ok(())
}

async fn save_tokens_and_publish_lifecycle(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
) -> Result<(), (StatusCode, String)> {
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    save_tokens_and_publish_lifecycle_commit_unlocked(
        token_store,
        auth_lease,
        auth_binding,
        tokens,
        true,
    )
    .await
    .map(|_| ())
}

async fn rollback_token_commit(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    commit: &TokenCommitSnapshot,
) -> Result<(), String> {
    match &commit.previous {
        Some(previous) => match commit.previous_lifecycle.phase {
            Some(phase) if phase != meerkat_core::handles::AuthLeasePhase::Released => {
                auth_lease
                    .release_credential_lifecycle(&commit.lease_key)
                    .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
                token_store
                    .save(&commit.key, previous)
                    .await
                    .map_err(|e| format!("TokenStore rollback save failed: {e}"))?;
                meerkat_core::restore_token_lifecycle_snapshot(
                    auth_lease,
                    &commit.lease_key,
                    &commit.previous_lifecycle,
                    Some(previous),
                )
                .map_err(|e| format!("AuthMachine lifecycle rollback failed: {e}"))?;
                let restored_snapshot = auth_lease.snapshot(&commit.lease_key);
                if restored_snapshot.credential_present {
                    let restored_previous =
                        meerkat_core::mark_tokens_lifecycle_published_for_snapshot(
                            previous,
                            &restored_snapshot,
                        );
                    token_store
                        .save(&commit.key, &restored_previous)
                        .await
                        .map_err(|e| format!("TokenStore rollback marker save failed: {e}"))?;
                }
            }
            _ => {
                auth_lease
                    .release_credential_lifecycle(&commit.lease_key)
                    .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
                token_store
                    .save(&commit.key, previous)
                    .await
                    .map_err(|e| format!("TokenStore rollback save failed: {e}"))?;
            }
        },
        None => {
            auth_lease
                .release_credential_lifecycle(&commit.lease_key)
                .map_err(|e| format!("AuthMachine lifecycle rollback release failed: {e}"))?;
            token_store
                .clear(&commit.key)
                .await
                .map_err(|e| format!("TokenStore rollback clear failed: {e}"))?;
        }
    }
    Ok(())
}

async fn save_prepared_tokens_after_terminal_consume_unlocked(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    prepared: PreparedTokenCommitSnapshot,
) -> Result<(), (StatusCode, String)> {
    let previous_lifecycle = auth_lease.snapshot(&prepared.lease_key);
    let transition =
        match meerkat_core::publish_token_lifecycle_acquired(auth_lease, auth_binding, tokens) {
            Ok(transition) => transition,
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("AuthMachine lifecycle acquire failed after OAuth consume: {e}"),
                ));
            }
        };
    let committed_tokens =
        meerkat_core::mark_tokens_lifecycle_published_for_transition(tokens, transition);
    let commit = TokenCommitSnapshot {
        key: prepared.key,
        lease_key: prepared.lease_key,
        previous: prepared.previous,
        previous_lifecycle,
        lifecycle_transition: transition,
    };
    if let Err(e) = token_store.save(&commit.key, &committed_tokens).await {
        let message = match rollback_token_commit(token_store, auth_lease, &commit).await {
            Ok(()) => {
                format!(
                    "TokenStore save failed after OAuth consume: {e}; AuthMachine lifecycle rolled back"
                )
            }
            Err(rollback_error) => {
                format!(
                    "TokenStore save failed after OAuth consume: {e}; AuthMachine lifecycle rollback failed: {rollback_error}"
                )
            }
        };
        return Err((StatusCode::INTERNAL_SERVER_ERROR, message));
    }
    Ok(())
}

async fn save_tokens_and_consume_device_flow_unlocked(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    poll_lease: OAuthDevicePollLease,
) -> Result<(), (StatusCode, String)> {
    if !poll_lease.terminal_flow_state_is_authmachine_owned() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "consume_oauth_device_flow requires an AuthMachine-owned OAuth device poll lease"
                .to_string(),
        ));
    }
    verify_terminal_device_flow(&poll_lease)?;
    let prepared = prepare_token_commit_unlocked(token_store, auth_binding).await?;
    consume_terminal_device_flow(auth_lease, auth_binding, poll_lease)?;
    save_prepared_tokens_after_terminal_consume_unlocked(
        token_store,
        auth_lease,
        auth_binding,
        tokens,
        prepared,
    )
    .await?;
    Ok(())
}

async fn save_tokens_and_consume_device_flow(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    poll_lease: OAuthDevicePollLease,
) -> Result<(), (StatusCode, String)> {
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    save_tokens_and_consume_device_flow_unlocked(
        token_store,
        auth_lease,
        auth_binding,
        tokens,
        poll_lease,
    )
    .await
}

struct BrowserFlowConsume<'a> {
    authority: &'a dyn meerkat_providers::oauth_flow::OAuthFlowAuthority,
    state: &'a str,
    provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
    redirect_uri: &'a str,
}

async fn save_tokens_and_consume_browser_flow_unlocked(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    flow: BrowserFlowConsume<'_>,
) -> Result<(), (StatusCode, String)> {
    if !flow.authority.terminal_flow_state_is_authmachine_owned() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "consume_oauth_browser_flow requires an AuthMachine-owned OAuth flow authority"
                .to_string(),
        ));
    }
    let prepared = prepare_token_commit_unlocked(token_store, auth_binding).await?;
    flow.authority
        .consume(flow.state, auth_binding, flow.provider, flow.redirect_uri)
        .map_err(|err| {
            release_uncredentialed_terminal_oauth_lifecycle(auth_lease, auth_binding);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("oauth state terminal consume failed: {err}"),
            )
        })?;
    save_prepared_tokens_after_terminal_consume_unlocked(
        token_store,
        auth_lease,
        auth_binding,
        tokens,
        prepared,
    )
    .await?;
    Ok(())
}

async fn save_tokens_and_consume_browser_flow(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
    tokens: &PersistedTokens,
    flow: BrowserFlowConsume<'_>,
) -> Result<(), (StatusCode, String)> {
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    save_tokens_and_consume_browser_flow_unlocked(
        token_store,
        auth_lease,
        auth_binding,
        tokens,
        flow,
    )
    .await
}

async fn restore_tokens_after_lifecycle_failure(
    token_store: &dyn TokenStore,
    key: &TokenKey,
    previous: Option<&PersistedTokens>,
) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
    match previous {
        Some(tokens) => token_store.save(key, tokens).await,
        None => token_store.clear(key).await,
    }
}

async fn clear_tokens_and_publish_lifecycle(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) -> Result<(), (StatusCode, String)> {
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    meerkat_core::clear_tokens_and_publish_lifecycle_released(token_store, auth_lease, auth_binding)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

// --- Realm endpoints -------------------------------------------------

pub async fn list_realms(State(state): State<AppState>) -> impl IntoResponse {
    match load_config(&state).await {
        Ok(config) => {
            let realms: Vec<WireRealmSummary> = config
                .realm
                .iter()
                .map(|(realm_id, section)| WireRealmSummary {
                    realm_id: realm_id.clone(),
                    default_binding: section.default_binding.clone(),
                    backend_count: section.backend.len(),
                    auth_profile_count: section.auth.len(),
                    binding_count: section.binding.len(),
                })
                .collect();
            (StatusCode::OK, Json(WireRealmList { realms })).into_response()
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

pub async fn get_realm(
    State(state): State<AppState>,
    Path(realm_id): Path<RealmId>,
) -> impl IntoResponse {
    match resolve_realm(&state, &realm_id).await {
        Ok(realm) => {
            let wire = WireRealmConnectionSet::from(&realm);
            (StatusCode::OK, Json(wire)).into_response()
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

// --- Auth profile endpoints ------------------------------------------

#[derive(serde::Deserialize)]
pub struct RealmQuery {
    pub realm_id: RealmId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

pub async fn list_auth_profiles(
    State(state): State<AppState>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    match resolve_realm(&state, &query.realm_id).await {
        Ok(realm) => {
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
            (
                StatusCode::OK,
                Json(WireAuthProfilesList {
                    realm_id: realm.realm_id,
                    auth_profiles: profiles,
                    backend_profiles: backends,
                    bindings,
                }),
            )
                .into_response()
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct CreateAuthProfileBody {
    pub realm_id: RealmId,
    pub binding_id: BindingId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
    pub provider: String,
    pub auth_method: String,
    pub secret: String,
}

/// Create an auth credential entry by writing the secret into the
/// TokenStore under the binding-scoped `AuthBindingRef`. The resolved
/// auth profile must declare `source.kind = "managed_store"`; otherwise
/// the provider runtime would read a different credential source.
pub async fn create_auth_profile(
    State(state): State<AppState>,
    Json(body): Json<CreateAuthProfileBody>,
) -> impl IntoResponse {
    let (auth_binding, _binding, auth_profile) = match resolve_binding_identity(
        &state,
        &body.realm_id,
        &body.binding_id,
        body.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    if body.provider != auth_profile.provider.as_str() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "binding {} resolves provider '{}' not '{}'",
                    body.binding_id,
                    auth_profile.provider.as_str(),
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    if body.auth_method != auth_profile.auth_method {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "binding {} resolves auth_method '{}' not '{}'",
                    body.binding_id,
                    auth_profile.auth_method,
                    body.auth_method,
                ),
            })),
        )
            .into_response();
    }
    if let Err((status, msg)) = require_managed_store_source(&body.binding_id, &auth_profile) {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    let auth_mode = match body.auth_method.as_str() {
        "api_key" => PersistedAuthMode::ApiKey,
        "static_bearer" => PersistedAuthMode::StaticBearer,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "auth_method '{other}' cannot be created via the REST endpoint. \
                         OAuth methods use /auth/login/start + /auth/login/complete; \
                         managed_store and external_resolver are configured via TOML."
                    ),
                })),
            )
                .into_response();
        }
    };
    let tokens = PersistedTokens {
        auth_mode,
        primary_secret: Some(body.secret),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(chrono::Utc::now()),
        scopes: Vec::new(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    if let Err((status, msg)) = save_tokens_and_publish_lifecycle(
        state.token_store.as_ref(),
        state.auth_lease.as_ref(),
        &auth_binding,
        &tokens,
    )
    .await
    {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?auth_binding,
        action = "create_profile",
        provider = %auth_profile.provider.as_str(),
        auth_method = %auth_profile.auth_method,
        "binding-scoped auth credentials stored via REST"
    );
    (
        StatusCode::CREATED,
        Json(WireAuthProfileCreated {
            identity: WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            provider: auth_profile.provider.as_str().to_string(),
            auth_method: auth_profile.auth_method.clone(),
            stored: true,
        }),
    )
        .into_response()
}

pub async fn get_auth_profile(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    match resolve_binding_identity(
        &state,
        &query.realm_id,
        &binding_id,
        query.profile_id.as_ref(),
    )
    .await
    {
        Ok((auth_binding, binding, auth_profile)) => (
            StatusCode::OK,
            Json(WireAuthProfileDetail {
                auth_binding: auth_binding.into(),
                binding_id: binding.id.clone(),
                profile_id: auth_profile.id.clone(),
                auth_profile: WireAuthProfile::from(&auth_profile),
            }),
        )
            .into_response(),
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

pub async fn delete_auth_profile(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    let (auth_binding, _binding, auth_profile) = match resolve_binding_identity(
        &state,
        &query.realm_id,
        &binding_id,
        query.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    if let Err((status, msg)) = clear_tokens_and_publish_lifecycle(
        state.token_store.as_ref(),
        state.auth_lease.as_ref(),
        &auth_binding,
    )
    .await
    {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?auth_binding,
        action = "delete_profile",
        "binding-scoped auth credentials deleted via REST"
    );
    (
        StatusCode::NO_CONTENT,
        Json(WireAuthProfileCleared {
            identity: WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            cleared: true,
        }),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct TestBindingBody {
    pub realm_id: RealmId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

pub async fn test_auth_binding(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Json(body): Json<TestBindingBody>,
) -> impl IntoResponse {
    match resolve_realm(&state, &body.realm_id).await {
        Ok(realm) => {
            let env = meerkat_providers::ResolverEnvironment::with_process_env()
                .with_token_store(Arc::clone(&state.token_store))
                .with_auth_lease_handle(Arc::clone(&state.auth_lease));
            let auth_binding = AuthBindingRef {
                realm: body.realm_id.clone(),
                binding: binding_id.clone(),
                profile: body.profile_id.clone(),
            };
            match state
                .provider_registry
                .resolve(&realm, &auth_binding, &env)
                .await
            {
                Ok(conn) => {
                    let (auth_binding, binding, auth_profile) = match resolve_binding_identity(
                        &state,
                        &body.realm_id,
                        &binding_id,
                        body.profile_id.as_ref(),
                    )
                    .await
                    {
                        Ok(v) => v,
                        Err((status, msg)) => {
                            return (status, Json(serde_json::json!({ "error": msg })))
                                .into_response();
                        }
                    };
                    if let Err(e) =
                        publish_resolved_auth_lease(state.auth_lease.as_ref(), &auth_binding, &conn)
                    {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "state": "error",
                                "error": format!("AuthMachine lifecycle acquire failed: {e}"),
                            })),
                        )
                            .into_response();
                    }
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "state": "valid",
                            "auth_binding": &auth_binding,
                            "binding_id": &binding.id,
                            "profile_id": &auth_profile.id,
                            "provider": conn.provider.as_str(),
                            "backend_profile_id": &conn.backend_profile.id,
                            "has_credential": conn.resolved_secret().is_some()
                                || conn.resolved_authorizer().is_some(),
                        })),
                    )
                        .into_response()
                }
                Err(e) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "state": "error",
                        "error": format!("Binding resolution failed: {e}"),
                    })),
                )
                    .into_response(),
            }
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

// --- OAuth login flow -----------------------------------------------
//
// The server owns the state -> PKCE verifier correlation. The client receives
// only the authorize URL and state, then posts the provider code with that state.

#[derive(Debug, serde::Deserialize)]
pub struct LoginStartBody {
    pub provider: String,
    /// Client-provided redirect URI (typically a loopback binding that
    /// the caller has already bound). The authorize URL will embed this.
    pub redirect_uri: String,
    pub realm_id: RealmId,
    pub binding_id: BindingId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

pub async fn start_login(
    State(state): State<AppState>,
    Json(body): Json<LoginStartBody>,
) -> impl IntoResponse {
    let resolved = match resolve_oauth_provider(&body.provider, &body.redirect_uri) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let target = match resolve_oauth_target(
        &state,
        resolved.provider,
        Some(&body.realm_id),
        Some(&body.binding_id),
        body.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    if let Err(e) =
        validate_oauth_login_binding(&target.backend, &target.auth_profile, resolved.identity)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let auth_binding = target.auth_binding;
    let pkce = PkcePair::generate_s256();
    let verifier = pkce.verifier.secret().to_string();
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    let state_token = match state.oauth_flow_authority().start(
        auth_binding,
        resolved.identity,
        body.redirect_uri.clone(),
        verifier,
    ) {
        Ok(state) => state,
        Err(OAuthFlowError::CapacityExceeded { .. }) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "oauth state registry is at capacity" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("oauth state initialization failed: {e}") })),
            )
                .into_response();
        }
    };
    let authorize_url = resolved
        .endpoints
        .authorize_url_with_pkce(&pkce.challenge, &state_token);
    (
        StatusCode::OK,
        Json(WireLoginStart {
            authorize_url,
            state: state_token,
            redirect_uri: body.redirect_uri,
            provider: body.provider,
        }),
    )
        .into_response()
}

#[derive(Debug, serde::Deserialize)]
pub struct LoginCompleteBody {
    pub provider: String,
    pub code: String,
    pub state: String,
    pub redirect_uri: String,
    pub realm_id: RealmId,
    pub binding_id: BindingId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

pub async fn complete_login(
    State(state): State<AppState>,
    Json(body): Json<LoginCompleteBody>,
) -> impl IntoResponse {
    let resolved = match resolve_oauth_provider(&body.provider, &body.redirect_uri) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let provider = resolved.provider;
    let target = match resolve_oauth_target(
        &state,
        provider,
        Some(&body.realm_id),
        Some(&body.binding_id),
        body.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let auth_binding = target.auth_binding;
    let binding = target.binding;
    let backend_profile = target.backend;
    let auth_profile = target.auth_profile;
    if provider != auth_profile.provider {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "binding {} resolves provider '{}' not '{}'",
                    binding.id,
                    auth_profile.provider.as_str(),
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    if let Err(e) = validate_oauth_login_binding(&backend_profile, &auth_profile, resolved.identity)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    let flow = match state.oauth_flow_authority().verify(
        &body.state,
        &auth_binding,
        resolved.identity,
        &body.redirect_uri,
    ) {
        Ok(flow) => flow,
        Err(OAuthFlowError::Missing) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "oauth state is missing or expired" })),
            )
                .into_response();
        }
        Err(e @ OAuthFlowError::RegistryProjectionMissing { .. }) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::json!({ "error": format!("oauth registry projection failed: {e}") }),
                ),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({ "error": format!("oauth state verification failed: {e}") }),
                ),
            )
                .into_response();
        }
    };

    let http = reqwest::Client::new();
    let result = match exchange_authorization_code_with_state(
        &http,
        &resolved.endpoints,
        &body.code,
        &flow.pkce_verifier,
        resolved.client_secret,
        Some(&body.state),
    )
    .await
    {
        Ok(r) => r,
        Err(OAuthError::TokenEndpoint { status, body }) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": "token endpoint rejected code",
                    "upstream_status": status,
                    "upstream_body": body,
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Token exchange failed: {e}") })),
            )
                .into_response();
        }
    };

    let expires_at = match result.expires_at_from(chrono::Utc::now()) {
        Ok(expires_at) => expires_at,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("token expiry is invalid: {e}") })),
            )
                .into_response();
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

    let authority = state.oauth_flow_authority();
    if let Err((status, msg)) = save_tokens_and_consume_browser_flow_unlocked(
        state.token_store.as_ref(),
        state.auth_lease.as_ref(),
        &auth_binding,
        &tokens,
        BrowserFlowConsume {
            authority: authority.as_ref(),
            state: &body.state,
            provider: resolved.identity,
            redirect_uri: &body.redirect_uri,
        },
    )
    .await
    {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }

    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?auth_binding,
        action = "login_oauth_complete",
        provider = %body.provider,
        has_refresh_token = %tokens.refresh_token.is_some(),
        "OAuth login completed via REST"
    );

    (
        StatusCode::OK,
        Json(WireLoginReady {
            state: None,
            identity: WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            provider: body.provider,
            expires_at: expires_at.map(|e| e.to_rfc3339()),
            has_refresh_token: tokens.refresh_token.is_some(),
            scopes: tokens.scopes.clone(),
        }),
    )
        .into_response()
}

#[derive(Debug, serde::Deserialize)]
pub struct DeviceStartBody {
    pub provider: String,
    pub realm_id: RealmId,
    pub binding_id: BindingId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

pub async fn start_device_login(
    State(state): State<AppState>,
    Json(body): Json<DeviceStartBody>,
) -> impl IntoResponse {
    let resolved = match resolve_oauth_provider(&body.provider, "") {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    if resolved.endpoints.device_code_url.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "provider '{}' does not support the device-code flow",
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    let target = match resolve_oauth_target(
        &state,
        resolved.provider,
        Some(&body.realm_id),
        Some(&body.binding_id),
        body.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    if let Err(e) =
        validate_oauth_login_binding(&target.backend, &target.auth_profile, resolved.identity)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let auth_binding = target.auth_binding;
    let http = reqwest::Client::new();
    match request_device_code(&http, &resolved.endpoints).await {
        Ok(resp) => {
            let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&auth_binding);
            let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
            if let Err(err) = state.oauth_flow_authority().admit_device_code(
                auth_binding,
                resolved.identity,
                resp.device_code.clone(),
                std::time::Duration::from_secs(resp.expires_in),
            ) {
                let (status, message) = match err {
                    OAuthFlowError::CapacityExceeded { .. } => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "oauth state registry is at capacity".to_string(),
                    ),
                    other => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("oauth device state initialization failed: {other}"),
                    ),
                };
                return (status, Json(serde_json::json!({ "error": message }))).into_response();
            }
            (
                StatusCode::OK,
                Json(WireDeviceStart {
                    device_code: resp.device_code,
                    user_code: resp.user_code,
                    verification_uri: resp.verification_uri,
                    verification_uri_complete: resp.verification_uri_complete,
                    expires_in: resp.expires_in,
                    interval: resp.interval,
                    provider: body.provider,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": format!("device-code request failed: {e}"),
            })),
        )
            .into_response(),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct DeviceCompleteBody {
    pub provider: String,
    pub device_code: String,
    pub realm_id: RealmId,
    pub binding_id: BindingId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

/// Device-code flow completion leg. Single-poll semantics — the caller
/// runs the outer retry loop using the `interval` returned from
/// `POST /auth/login/device/start`. Returns:
///   * `{ "state": "pending" }`     (202)
///   * `{ "state": "slow_down" }`   (429)
///   * `{ "state": "access_denied" }` / `{ "state": "expired" }` (400)
///   * `{ "state": "ready", realm_id, binding_id, ... }` (200, tokens
///     persisted to TokenStore under the resolved `AuthBindingRef`).
pub async fn complete_device_login(
    State(state): State<AppState>,
    Json(body): Json<DeviceCompleteBody>,
) -> impl IntoResponse {
    let resolved = match resolve_oauth_provider(&body.provider, "") {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let provider = resolved.provider;
    if resolved.endpoints.device_code_url.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "provider '{}' does not support the device-code flow",
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    let target = match resolve_oauth_target(
        &state,
        provider,
        Some(&body.realm_id),
        Some(&body.binding_id),
        body.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let auth_binding = target.auth_binding;
    let binding = target.binding;
    let backend_profile = target.backend;
    let auth_profile = target.auth_profile;
    if provider != auth_profile.provider {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "binding {} resolves provider '{}' not '{}'",
                    binding.id,
                    auth_profile.provider.as_str(),
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    if let Err(e) = validate_oauth_login_binding(&backend_profile, &auth_profile, resolved.identity)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&auth_binding);
    let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
    let poll_lease = match state.oauth_flow_authority().begin_device_code_poll(
        &body.device_code,
        &auth_binding,
        resolved.identity,
    ) {
        Ok(lease) => lease,
        Err(err) => {
            let (status, message) = oauth_device_state_error(err);
            return (status, Json(serde_json::json!({ "error": message }))).into_response();
        }
    };
    let http = reqwest::Client::new();
    let outcome = match poll_device_code(
        &http,
        &resolved.endpoints,
        &body.device_code,
        resolved.client_secret,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!("device-code poll failed: {e}"),
                })),
            )
                .into_response();
        }
    };
    match outcome {
        DevicePollOutcome::Pending => match finish_device_flow_poll(poll_lease) {
            Ok(()) => (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "state": "pending" })),
            )
                .into_response(),
            Err((status, message)) => {
                (status, Json(serde_json::json!({ "error": message }))).into_response()
            }
        },
        DevicePollOutcome::SlowDown => match finish_device_flow_poll(poll_lease) {
            Ok(()) => (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({ "state": "slow_down" })),
            )
                .into_response(),
            Err((status, message)) => {
                (status, Json(serde_json::json!({ "error": message }))).into_response()
            }
        },
        DevicePollOutcome::AccessDenied => {
            match consume_terminal_device_flow(state.auth_lease.as_ref(), &auth_binding, poll_lease)
            {
                Ok(()) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "state": "access_denied" })),
                )
                    .into_response(),
                Err((status, message)) => {
                    (status, Json(serde_json::json!({ "error": message }))).into_response()
                }
            }
        }
        DevicePollOutcome::Expired => {
            match consume_terminal_device_flow(state.auth_lease.as_ref(), &auth_binding, poll_lease)
            {
                Ok(()) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "state": "expired" })),
                )
                    .into_response(),
                Err((status, message)) => {
                    (status, Json(serde_json::json!({ "error": message }))).into_response()
                }
            }
        }
        DevicePollOutcome::Ready(result) => {
            let expires_at = match result.expires_at_from(chrono::Utc::now()) {
                Ok(expires_at) => expires_at,
                Err(e) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "error": format!("token expiry is invalid: {e}")
                        })),
                    )
                        .into_response();
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
            if let Err((status, msg)) = save_tokens_and_consume_device_flow_unlocked(
                state.token_store.as_ref(),
                state.auth_lease.as_ref(),
                &auth_binding,
                &tokens,
                poll_lease,
            )
            .await
            {
                return (status, Json(serde_json::json!({ "error": msg }))).into_response();
            }
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?auth_binding,
                action = "login_device_complete",
                provider = %body.provider,
                has_refresh_token = %tokens.refresh_token.is_some(),
                "OAuth device-flow login completed via REST"
            );
            (
                StatusCode::OK,
                Json(WireLoginReady {
                    state: Some("ready".to_string()),
                    identity: WireBindingIdentity::from(&auth_binding),
                    profile_id: auth_profile.id.clone(),
                    provider: body.provider,
                    expires_at: expires_at.map(|e| e.to_rfc3339()),
                    has_refresh_token: tokens.refresh_token.is_some(),
                    scopes: tokens.scopes.clone(),
                }),
            )
                .into_response()
        }
    }
}

pub async fn get_auth_status(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    let (auth_binding, _binding, auth_profile) = match resolve_binding_identity(
        &state,
        &query.realm_id,
        &binding_id,
        query.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let lease_key = LeaseKey::from_auth_binding(&auth_binding);
    let mut snapshot = state.auth_lease.snapshot(&lease_key);
    let now = chrono::Utc::now();
    let expected_mode = persisted_auth_mode_for_auth_method(&auth_profile.auth_method);
    let source_uses_store = credential_source_uses_persisted_store(&auth_profile.source);
    let oauth_mode = expected_mode
        .map(persisted_auth_mode_is_oauth_login)
        .unwrap_or(false);
    let mut phase = meerkat_core::AuthStatusPhase::from_lease_snapshot(now, &snapshot);
    let rehydrated = if phase == meerkat_core::AuthStatusPhase::Unknown && source_uses_store {
        if let Some(expected_mode) = expected_mode {
            meerkat_core::rehydrate_marked_oauth_tokens_for_status(
                state.token_store.as_ref(),
                state.auth_lease.as_ref(),
                &auth_binding,
                expected_mode,
                now,
            )
            .await
            .ok()
            .flatten()
        } else {
            None
        }
    } else {
        None
    };
    if rehydrated.is_some() {
        snapshot = state.auth_lease.snapshot(&lease_key);
        phase = meerkat_core::AuthStatusPhase::from_lease_snapshot(now, &snapshot);
    }
    let stored = if rehydrated.is_some() {
        rehydrated
    } else if phase == meerkat_core::AuthStatusPhase::Unknown {
        None
    } else {
        state
            .token_store
            .load(&TokenKey::from_auth_binding(&auth_binding))
            .await
            .ok()
            .flatten()
    };
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
    let unknown_snapshot;
    let marker_projection_snapshot;
    let (projection_tokens, projection_snapshot) = if oauth_source_rejected {
        unknown_snapshot = meerkat_core::handles::AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            credential_present: false,
            generation: snapshot.generation,
            credential_published_at_millis: None,
        };
        (None, &unknown_snapshot)
    } else if token_matches_binding {
        marker_projection_snapshot = stored.as_ref().filter(|_| oauth_mode).and_then(|tokens| {
            meerkat_core::oauth_status_projection_snapshot_from_newer_marker(&snapshot, tokens)
        });
        (
            if source_uses_store {
                stored.as_ref()
            } else {
                None
            },
            marker_projection_snapshot.as_ref().unwrap_or(&snapshot),
        )
    } else {
        unknown_snapshot = meerkat_core::handles::AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
            credential_present: false,
            generation: snapshot.generation,
            credential_published_at_millis: None,
        };
        (None, &unknown_snapshot)
    };
    let projection =
        meerkat_core::project_published_auth_status(now, projection_tokens, projection_snapshot);
    let tokens = projection.tokens;
    (
        StatusCode::OK,
        Json(WireAuthStatusDetail {
            identity: WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            provider: auth_profile.provider.as_str().to_string(),
            auth_method: auth_profile.auth_method.clone(),
            state: projection.phase,
            expires_at: projection.expires_at.map(|e| e.to_rfc3339()),
            last_refresh_at: tokens.and_then(|t| t.last_refresh.map(|e| e.to_rfc3339())),
            account_id: tokens.and_then(|t| t.account_id.clone()),
            has_refresh_token: tokens.map(|t| t.refresh_token.is_some()).unwrap_or(false),
        }),
    )
        .into_response()
}

pub async fn logout(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    let (auth_binding, _binding, auth_profile) = match resolve_binding_identity(
        &state,
        &query.realm_id,
        &binding_id,
        query.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    if let Err((status, msg)) = clear_tokens_and_publish_lifecycle(
        state.token_store.as_ref(),
        state.auth_lease.as_ref(),
        &auth_binding,
    )
    .await
    {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?auth_binding,
        action = "logout",
        "binding-scoped auth credentials logged out via REST"
    );
    (
        StatusCode::OK,
        Json(WireAuthProfileCleared {
            identity: WireBindingIdentity::from(&auth_binding),
            profile_id: auth_profile.id.clone(),
            cleared: true,
        }),
    )
        .into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use meerkat_core::handles::{
        AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition,
        DslTransitionError, LeaseKey,
    };
    use meerkat_providers::auth_store::{EphemeralTokenStore, FileTokenStore};
    use meerkat_runtime::RuntimeAuthLeaseHandle;
    use meerkat_runtime::handles::RuntimeOAuthFlowHandle;

    fn managed_auth_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse("rest-test").unwrap(),
            binding: BindingId::parse("managed").unwrap(),
            profile: None,
        }
    }

    fn openai_auth_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        }
    }

    fn google_auth_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_google").unwrap(),
            profile: None,
        }
    }

    fn api_key_tokens() -> PersistedTokens {
        api_key_tokens_with_secret("sk-test")
    }

    fn api_key_tokens_with_secret(secret: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: PersistedAuthMode::ApiKey,
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

    struct RejectingAuthLeaseHandle;

    impl AuthLeaseHandle for RejectingAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            Err(DslTransitionError::guard_rejected(
                "acquire_lease",
                "test rejection",
            ))
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
                generation: 1,
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
            AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                credential_present: false,
                generation: 0,
                credential_published_at_millis: None,
            }
        }
    }

    struct ReleaseRejectingAuthLeaseHandle;

    impl AuthLeaseHandle for ReleaseRejectingAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            Ok(AuthLeaseTransition {
                generation: 1,
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
                generation: 1,
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
            Err(DslTransitionError::guard_rejected(
                "release_lease",
                "test rejection",
            ))
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(1_800_000_000),
                credential_present: true,
                generation: 1,
                credential_published_at_millis: None,
            }
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
            _provider: meerkat_providers::oauth_flow::OAuthProviderIdentity,
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
        inner: EphemeralTokenStore,
        save_count: std::sync::atomic::AtomicUsize,
    }

    impl SaveCountingTokenStore {
        fn new() -> Self {
            Self {
                inner: EphemeralTokenStore::new(),
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

    fn config_with_openai_managed_store_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "openai_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: "openai_api".into(),
                base_url: None,
                options: serde_json::json!({}),
            },
        );
        section.auth.insert(
            "openai_managed".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: "api_key".into(),
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
                backend_kind: "chatgpt_backend".into(),
                base_url: None,
                options: serde_json::json!({}),
            },
        );
        section.auth.insert(
            "openai_oauth".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: "managed_chatgpt_oauth".into(),
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
            .backend_kind = "openai_api".into();
        config
    }

    fn config_with_openai_external_authorizer_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "openai_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: "openai_api".into(),
                base_url: None,
                options: serde_json::json!({}),
            },
        );
        section.auth.insert(
            "openai_external".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: "external_authorizer".into(),
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
                backend_kind: "google_genai".into(),
                base_url: None,
                options: serde_json::json!({}),
            },
        );
        section.auth.insert(
            "google_api_key".into(),
            meerkat_core::AuthProfileConfig {
                provider: "gemini".into(),
                auth_method: "api_key".into(),
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
            .auth_method = "google_oauth".into();
        config
    }

    async fn auth_status_detail(response: impl IntoResponse) -> WireAuthStatusDetail {
        let response = response.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn install_ephemeral_auth_state(state: &mut AppState) {
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        state
            .runtime_adapter
            .set_runtime_auth_lease_handle(Arc::clone(&auth_lease));
        let auth_lease: Arc<dyn AuthLeaseHandle> = auth_lease;
        state.auth_lease = auth_lease;
        state.token_store = Arc::new(EphemeralTokenStore::new());
    }

    #[tokio::test]
    async fn rest_test_auth_binding_publishes_resolved_lease_to_auth_machine() {
        use meerkat_providers::{
            ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
            ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
        };

        struct ResolvingOpenAiRuntime {
            expires_at: chrono::DateTime<chrono::Utc>,
            saw_auth_lease_handle: Arc<std::sync::atomic::AtomicBool>,
        }

        #[async_trait::async_trait]
        impl ProviderRuntime for ResolvingOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                self.saw_auth_lease_handle.store(
                    env.auth_lease_handle.is_some(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(ResolvedConnection {
                    provider: Provider::OpenAI,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        "sk-rest-test".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        Some(self.expires_at),
                        "test",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn meerkat_client::LlmClient>, ProviderClientError> {
                Err(ProviderClientError::MissingFeature("test"))
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let saw_auth_lease_handle = Arc::new(std::sync::atomic::AtomicBool::new(false));
        state.provider_registry = Arc::new(ProviderRuntimeRegistry::empty().with_runtime(
            Arc::new(ResolvingOpenAiRuntime {
                expires_at,
                saw_auth_lease_handle: Arc::clone(&saw_auth_lease_handle),
            }),
        ));

        let response = test_auth_binding(
            State(state.clone()),
            Path(BindingId::parse("default_openai").unwrap()),
            Json(TestBindingBody {
                realm_id: RealmId::parse("dev").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            saw_auth_lease_handle.load(std::sync::atomic::Ordering::SeqCst),
            "auth test resolution must receive the AuthMachine authority handle"
        );
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let snapshot = state
            .auth_lease
            .snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        assert!(snapshot.credential_present);
        assert_eq!(snapshot.expires_at, Some(expires_at.timestamp() as u64));
    }

    #[tokio::test]
    async fn rest_login_start_is_isolated_between_runtime_authorities() {
        let temp = tempfile::tempdir().unwrap();
        let unrelated_temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let unrelated_state = AppState::load_from(unrelated_temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(
                config_with_openai_oauth_binding(CredentialSourceSpec::ManagedStore),
                None,
            )
            .await
            .unwrap();
        unrelated_state
            .config_runtime
            .set(
                config_with_openai_oauth_binding(CredentialSourceSpec::ManagedStore),
                None,
            )
            .await
            .unwrap();
        let redirect_uri = "http://127.0.0.1:0/callback";

        let response = start_login(
            State(state.clone()),
            Json(LoginStartBody {
                provider: "openai".to_string(),
                redirect_uri: redirect_uri.to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_openai").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let state_token = result["state"].as_str().expect("state");
        assert!(matches!(
            unrelated_state.oauth_flow_authority().consume(
                state_token,
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "verify_oauth_browser_flow",
                ..
            })
        ));
        let flow = state
            .oauth_flow_authority()
            .consume(
                state_token,
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("starting runtime owns the flow");
        assert!(!flow.pkce_verifier.is_empty());
    }

    #[tokio::test]
    async fn rest_login_start_records_flow_on_runtime_authority() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(
                config_with_openai_oauth_binding(CredentialSourceSpec::ManagedStore),
                None,
            )
            .await
            .unwrap();
        let redirect_uri = "http://127.0.0.1:0/callback";

        let response = start_login(
            State(state.clone()),
            Json(LoginStartBody {
                provider: "openai".to_string(),
                redirect_uri: redirect_uri.to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_openai").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let state_token = result["state"].as_str().expect("state");
        let flow = state
            .runtime_adapter
            .oauth_flow_authority()
            .consume(
                state_token,
                &openai_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            )
            .expect("runtime AuthMachine authority owns the REST login flow");
        assert!(!flow.pkce_verifier.is_empty());
    }

    #[tokio::test]
    async fn rest_login_start_rejects_same_provider_non_oauth_target_before_state_admission() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();

        let response = start_login(
            State(state.clone()),
            Json(LoginStartBody {
                provider: "openai".to_string(),
                redirect_uri: "http://127.0.0.1:0/callback".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_openai").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("auth_method 'api_key'")
        );
        let snapshot = state
            .auth_lease
            .snapshot(&LeaseKey::from_auth_binding(&openai_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_login_start_rejects_oauth_method_with_external_source_before_state_admission() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(
                config_with_openai_oauth_binding(CredentialSourceSpec::ExternalResolver {
                    handle: "external-chatgpt".into(),
                }),
                None,
            )
            .await
            .unwrap();

        let response = start_login(
            State(state.clone()),
            Json(LoginStartBody {
                provider: "openai".to_string(),
                redirect_uri: "http://127.0.0.1:0/callback".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_openai").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("source 'external_resolver'")
        );
        let snapshot = state
            .auth_lease
            .snapshot(&LeaseKey::from_auth_binding(&openai_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_login_start_rejects_oauth_method_with_wrong_backend_before_state_admission() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(config_with_openai_oauth_wrong_backend_binding(), None)
            .await
            .unwrap();

        let response = start_login(
            State(state.clone()),
            Json(LoginStartBody {
                provider: "openai".to_string(),
                redirect_uri: "http://127.0.0.1:0/callback".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_openai").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("backend_kind 'openai_api'")
        );
        let snapshot = state
            .auth_lease
            .snapshot(&LeaseKey::from_auth_binding(&openai_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_device_start_rejects_same_provider_non_oauth_target_before_state_admission() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(config_with_google_api_key_binding(), None)
            .await
            .unwrap();

        let response = start_device_login(
            State(state.clone()),
            Json(DeviceStartBody {
                provider: "google".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_google").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("auth_method 'api_key'")
        );
        let snapshot = state
            .auth_lease
            .snapshot(&LeaseKey::from_auth_binding(&google_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_device_start_rejects_oauth_method_with_wrong_backend_before_state_admission() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(config_with_google_oauth_wrong_backend_binding(), None)
            .await
            .unwrap();

        let response = start_device_login(
            State(state.clone()),
            Json(DeviceStartBody {
                provider: "google".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_google").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("backend_kind 'google_genai'")
        );
        let snapshot = state
            .auth_lease
            .snapshot(&LeaseKey::from_auth_binding(&google_auth_binding()));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_login_complete_rejects_same_provider_non_oauth_target_before_state_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();

        let response = complete_login(
            State(state),
            Json(LoginCompleteBody {
                provider: "openai".to_string(),
                code: "provider-code".to_string(),
                state: "missing-state".to_string(),
                redirect_uri: "http://127.0.0.1:0/callback".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_openai").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("auth_method 'api_key'")
        );
    }

    #[tokio::test]
    async fn rest_login_complete_rejects_oauth_method_with_wrong_backend_before_state_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(config_with_openai_oauth_wrong_backend_binding(), None)
            .await
            .unwrap();

        let response = complete_login(
            State(state),
            Json(LoginCompleteBody {
                provider: "openai".to_string(),
                code: "provider-code".to_string(),
                state: "missing-state".to_string(),
                redirect_uri: "http://127.0.0.1:0/callback".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_openai").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("backend_kind 'openai_api'")
        );
    }

    #[tokio::test]
    async fn rest_device_complete_rejects_same_provider_non_oauth_target_before_state_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(config_with_google_api_key_binding(), None)
            .await
            .unwrap();

        let response = complete_device_login(
            State(state),
            Json(DeviceCompleteBody {
                provider: "google".to_string(),
                device_code: "missing-device-code".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_google").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("auth_method 'api_key'")
        );
    }

    #[tokio::test]
    async fn rest_device_complete_rejects_oauth_method_with_wrong_backend_before_state_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .config_runtime
            .set(config_with_google_oauth_wrong_backend_binding(), None)
            .await
            .unwrap();

        let response = complete_device_login(
            State(state),
            Json(DeviceCompleteBody {
                provider: "google".to_string(),
                device_code: "missing-device-code".to_string(),
                realm_id: RealmId::parse("dev").unwrap(),
                binding_id: BindingId::parse("default_google").unwrap(),
                profile_id: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("backend_kind 'google_genai'")
        );
    }

    #[tokio::test]
    async fn rest_device_completion_poll_drop_releases_runtime_authority() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .oauth_flow_authority()
            .admit_device_code(
                managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                "device-code".to_string(),
                std::time::Duration::from_secs(600),
            )
            .expect("device code admitted");

        let poll = state
            .oauth_flow_authority()
            .begin_device_code_poll(
                "device-code",
                &managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("device completion poll begins");
        assert!(matches!(
            state.oauth_flow_authority().begin_device_code_poll(
                "device-code",
                &managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "begin_oauth_device_poll",
                ..
            })
        ));

        drop(poll);

        state
            .oauth_flow_authority()
            .begin_device_code_poll(
                "device-code",
                &managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("dropped REST completion poll releases runtime authority");
    }

    #[tokio::test]
    async fn rest_device_completion_poll_abort_releases_runtime_authority() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state
            .oauth_flow_authority()
            .admit_device_code(
                managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                "device-code".to_string(),
                std::time::Duration::from_secs(600),
            )
            .expect("device code admitted");

        let authority = state.oauth_flow_authority();
        let (begun_tx, begun_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _poll = authority
                .begin_device_code_poll(
                    "device-code",
                    &managed_auth_binding(),
                    meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                )
                .expect("device completion poll begins");
            begun_tx.send(()).expect("signal poll start");
            std::future::pending::<()>().await;
        });
        begun_rx.await.expect("poll lease was acquired");
        assert!(matches!(
            state.oauth_flow_authority().begin_device_code_poll(
                "device-code",
                &managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            ),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "begin_oauth_device_poll",
                ..
            })
        ));

        task.abort();
        let _ = task.await;

        state
            .oauth_flow_authority()
            .begin_device_code_poll(
                "device-code",
                &managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("aborted REST completion poll releases runtime authority");
    }

    #[tokio::test]
    async fn rest_token_write_helpers_publish_auth_machine_lifecycle() {
        let store = EphemeralTokenStore::new();
        let auth_lease = RuntimeAuthLeaseHandle::new();
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let tokens = api_key_tokens();

        save_tokens_and_publish_lifecycle(&store, &auth_lease, &auth_binding, &tokens)
            .await
            .unwrap();

        assert!(store.load(&key).await.unwrap().is_some());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));

        clear_tokens_and_publish_lifecycle(&store, &auth_lease, &auth_binding)
            .await
            .unwrap();

        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
        assert_eq!(snapshot.expires_at, None);
    }

    #[tokio::test]
    async fn rest_token_save_lifecycle_failure_restores_existing_credentials() {
        let store = EphemeralTokenStore::new();
        let auth_lease = RejectingAuthLeaseHandle;
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let previous = api_key_tokens_with_secret("sk-old");
        let replacement = api_key_tokens_with_secret("sk-new");
        store.save(&key, &previous).await.unwrap();

        let err =
            save_tokens_and_publish_lifecycle(&store, &auth_lease, &auth_binding, &replacement)
                .await
                .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("AuthMachine lifecycle acquire failed"));
        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-old"));
    }

    #[tokio::test]
    async fn rest_ready_device_commit_failure_does_not_save_before_consume_claim() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let auth_lease: Arc<dyn AuthLeaseHandle> = Arc::new(RejectingAuthLeaseHandle);
        let oauth_lifecycle = Arc::new(meerkat_runtime::handles::RuntimeAuthLeaseHandle::new());
        let oauth_authority = Arc::new(
            meerkat_runtime::handles::RuntimeOAuthFlowHandle::new_with_auth_lease(
                std::time::Duration::from_secs(600),
                oauth_lifecycle,
            ),
        );
        state
            .runtime_adapter
            .set_auth_lease_handle_with_oauth_flow_authority(
                Arc::clone(&auth_lease),
                oauth_authority,
            );
        state.auth_lease = auth_lease;
        state.token_store = Arc::new(EphemeralTokenStore::new());
        let authority = state.oauth_flow_authority();
        authority
            .admit_device_code(
                managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
                "device-code".to_string(),
                std::time::Duration::from_secs(600),
            )
            .expect("device code admitted");
        let poll_lease = authority
            .begin_device_code_poll(
                "device-code",
                &managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("device poll lease begins");

        let err = save_tokens_and_consume_device_flow(
            state.token_store.as_ref(),
            state.auth_lease.as_ref(),
            &managed_auth_binding(),
            &api_key_tokens(),
            poll_lease,
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            err.1
                .contains("AuthMachine lifecycle acquire failed after OAuth consume")
        );
        assert!(
            state
                .token_store
                .load(&TokenKey::from_auth_binding(&managed_auth_binding()))
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            authority.begin_device_code_poll(
                "device-code",
                &managed_auth_binding(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            ),
            Err(
                meerkat_providers::oauth_flow::OAuthFlowError::LifecycleRejected {
                    operation: "begin_oauth_device_poll",
                    ..
                }
            )
        ));
    }

    #[tokio::test]
    async fn rest_ready_device_consume_failure_does_not_commit_token() {
        let store = EphemeralTokenStore::new();
        let auth_lease = RuntimeAuthLeaseHandle::new();
        let auth_binding = managed_auth_binding();
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

        let err = save_tokens_and_consume_device_flow(
            &store,
            &auth_lease,
            &auth_binding,
            &api_key_tokens(),
            poll_lease,
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("consume_oauth_device_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_browser_consume_failure_does_not_commit_token() {
        let temp = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(temp.path().join("tokens"));
        let auth_lease = RuntimeAuthLeaseHandle::new();
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let authority = RejectBrowserConsumeAuthority;

        let err = save_tokens_and_consume_browser_flow(
            &store,
            &auth_lease,
            &auth_binding,
            &api_key_tokens(),
            BrowserFlowConsume {
                authority: &authority,
                state: "browser-state",
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback",
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("consume_oauth_browser_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_browser_consume_failure_does_not_save_before_durable_claim() {
        let store = SaveCountingTokenStore::new();
        let auth_lease = RuntimeAuthLeaseHandle::new();
        let auth_binding = managed_auth_binding();

        let err = save_tokens_and_consume_browser_flow(
            &store,
            &auth_lease,
            &auth_binding,
            &api_key_tokens(),
            BrowserFlowConsume {
                authority: &RejectBrowserConsumeAuthority,
                state: "browser-state",
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback",
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("consume_oauth_browser_flow"));
        assert_eq!(
            store.save_count(),
            0,
            "token material must not be saved before winning the durable OAuth consume claim"
        );
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_raw_registry_browser_success_cannot_commit_tokens() {
        let store = EphemeralTokenStore::new();
        let auth_lease = RuntimeAuthLeaseHandle::new();
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        let registry = meerkat_providers::oauth_flow::OAuthFlowRegistry::new(
            std::time::Duration::from_secs(600),
        );
        let state = registry
            .start(
                auth_binding.clone(),
                meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri.to_string(),
                "registry-only-verifier".to_string(),
            )
            .expect("raw registry admits browser flow");

        let err = save_tokens_and_consume_browser_flow(
            &store,
            &auth_lease,
            &auth_binding,
            &api_key_tokens(),
            BrowserFlowConsume {
                authority: &registry,
                state: &state,
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("AuthMachine-owned OAuth flow authority"));
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
    async fn rest_raw_registry_device_success_cannot_commit_tokens() {
        let store = EphemeralTokenStore::new();
        let auth_lease = RuntimeAuthLeaseHandle::new();
        let auth_binding = managed_auth_binding();
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

        let err = save_tokens_and_consume_device_flow(
            &store,
            &auth_lease,
            &auth_binding,
            &api_key_tokens(),
            poll_lease,
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("AuthMachine-owned OAuth device poll lease"));
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
    async fn rest_browser_consume_failure_does_not_reauthorize_stale_previous_tokens() {
        let store = EphemeralTokenStore::new();
        let auth_lease = RuntimeAuthLeaseHandle::new();
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let stale = api_key_tokens_with_secret("sk-stale");
        store.save(&key, &stale).await.unwrap();

        let err = save_tokens_and_consume_browser_flow(
            &store,
            &auth_lease,
            &auth_binding,
            &api_key_tokens_with_secret("sk-new"),
            BrowserFlowConsume {
                authority: &RejectBrowserConsumeAuthority,
                state: "browser-state",
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback",
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("consume_oauth_browser_flow"));
        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-stale"));
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
        let projection = meerkat_core::project_published_auth_status(
            chrono::Utc::now(),
            Some(&stored),
            &snapshot,
        );
        assert_eq!(projection.phase, meerkat_core::AuthStatusPhase::Unknown);
        assert!(projection.tokens.is_none());
    }

    #[tokio::test]
    async fn rest_stale_previous_rollback_preserves_newer_oauth_flow() {
        let store = EphemeralTokenStore::new();
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(600),
            Arc::clone(&auth_lease),
        );
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        store
            .save(&key, &api_key_tokens_with_secret("sk-stale"))
            .await
            .unwrap();
        let mut failed_tokens = api_key_tokens_with_secret("sk-new");
        failed_tokens.expires_at =
            Some(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap());
        let commit = save_tokens_and_publish_lifecycle_commit_unlocked(
            &store,
            auth_lease.as_ref(),
            &auth_binding,
            &failed_tokens,
            true,
        )
        .await
        .unwrap();
        let state = meerkat_providers::oauth_flow::OAuthFlowAuthority::start(
            &authority,
            auth_binding.clone(),
            meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            redirect_uri.to_string(),
            "new-verifier".to_string(),
        )
        .expect("newer OAuth flow admitted after rollback snapshot");

        rollback_token_commit(&store, auth_lease.as_ref(), &commit)
            .await
            .expect("rollback clears credential lifecycle without clobbering OAuth flow");

        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-stale"));
        let record = meerkat_providers::oauth_flow::OAuthFlowAuthority::verify(
            &authority,
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
        assert_eq!(projection.phase, meerkat_core::AuthStatusPhase::Unknown);
        assert!(projection.tokens.is_none());
    }

    #[tokio::test]
    async fn rest_oauth_only_previous_rollback_does_not_reauthorize_stale_tokens() {
        let store = EphemeralTokenStore::new();
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(600),
            Arc::clone(&auth_lease),
        );
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        store
            .save(&key, &api_key_tokens_with_secret("sk-stale"))
            .await
            .unwrap();
        let state = meerkat_providers::oauth_flow::OAuthFlowAuthority::start(
            &authority,
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

        let mut failed_tokens = api_key_tokens_with_secret("sk-new");
        failed_tokens.expires_at =
            Some(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap());
        let commit = save_tokens_and_publish_lifecycle_commit_unlocked(
            &store,
            auth_lease.as_ref(),
            &auth_binding,
            &failed_tokens,
            true,
        )
        .await
        .unwrap();

        rollback_token_commit(&store, auth_lease.as_ref(), &commit)
            .await
            .expect("rollback restores token bytes without restoring credential authority");

        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-stale"));
        let record = meerkat_providers::oauth_flow::OAuthFlowAuthority::verify(
            &authority,
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
        assert_eq!(projection.phase, meerkat_core::AuthStatusPhase::Unknown);
        assert!(projection.tokens.is_none());
    }

    #[tokio::test]
    async fn rest_real_device_consume_failure_releases_credential_lifecycle() {
        let store = EphemeralTokenStore::new();
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(600),
            Arc::clone(&auth_lease),
        );
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        meerkat_providers::oauth_flow::OAuthFlowAuthority::admit_device_code(
            &authority,
            auth_binding.clone(),
            meerkat_providers::oauth_flow::OAuthProviderIdentity::GoogleCodeAssist,
            "device-code".to_string(),
            std::time::Duration::from_secs(600),
        )
        .expect("device flow admitted by runtime authority");
        let poll_lease = meerkat_providers::oauth_flow::OAuthFlowAuthority::begin_device_code_poll(
            &authority,
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

        let err = save_tokens_and_consume_device_flow(
            &store,
            auth_lease.as_ref(),
            &auth_binding,
            &api_key_tokens(),
            poll_lease,
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("consume_oauth_device_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_real_browser_missing_consume_releases_credential_lifecycle() {
        let store = EphemeralTokenStore::new();
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(600),
            Arc::clone(&auth_lease),
        );
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        let state = meerkat_providers::oauth_flow::OAuthFlowAuthority::start(
            &authority,
            auth_binding.clone(),
            meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            redirect_uri.to_string(),
            "verifier".to_string(),
        )
        .expect("browser flow admitted by runtime authority");
        meerkat_providers::oauth_flow::OAuthFlowAuthority::verify(
            &authority,
            &state,
            &auth_binding,
            meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            redirect_uri,
        )
        .expect("browser flow verifies through runtime authority");
        meerkat_providers::oauth_flow::OAuthFlowAuthority::consume(
            &authority,
            &state,
            &auth_binding,
            meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            redirect_uri,
        )
        .expect("test pre-consumes the browser flow");

        let err = save_tokens_and_consume_browser_flow(
            &store,
            auth_lease.as_ref(),
            &auth_binding,
            &api_key_tokens(),
            BrowserFlowConsume {
                authority: &authority,
                state: &state,
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("oauth state terminal consume failed"));
        assert!(err.1.contains("verify_oauth_browser_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, None);
    }

    #[tokio::test]
    async fn rest_terminal_consume_failure_preserves_other_browser_flow() {
        let store = EphemeralTokenStore::new();
        let auth_lease = Arc::new(RuntimeAuthLeaseHandle::new());
        let authority = RuntimeOAuthFlowHandle::new_with_auth_lease(
            std::time::Duration::from_secs(600),
            Arc::clone(&auth_lease),
        );
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        let redirect_uri = "http://127.0.0.1/callback";
        let old_state = meerkat_providers::oauth_flow::OAuthFlowAuthority::start(
            &authority,
            auth_binding.clone(),
            meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            redirect_uri.to_string(),
            "old-verifier".to_string(),
        )
        .expect("old browser flow admitted");
        let new_state = meerkat_providers::oauth_flow::OAuthFlowAuthority::start(
            &authority,
            auth_binding.clone(),
            meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
            redirect_uri.to_string(),
            "new-verifier".to_string(),
        )
        .expect("new browser flow admitted");
        let rejecting = RejectBrowserConsumeAuthority;

        let err = save_tokens_and_consume_browser_flow(
            &store,
            auth_lease.as_ref(),
            &auth_binding,
            &api_key_tokens(),
            BrowserFlowConsume {
                authority: &rejecting,
                state: &old_state,
                provider: meerkat_providers::oauth_flow::OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("consume_oauth_browser_flow"));
        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&auth_binding));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::ReauthRequired));
        let record = meerkat_providers::oauth_flow::OAuthFlowAuthority::verify(
            &authority,
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
    async fn rest_token_clear_release_failure_keeps_credentials() {
        let store = EphemeralTokenStore::new();
        let auth_lease = ReleaseRejectingAuthLeaseHandle;
        let auth_binding = managed_auth_binding();
        let key = TokenKey::from_auth_binding(&auth_binding);
        store.save(&key, &api_key_tokens()).await.unwrap();

        let err = clear_tokens_and_publish_lifecycle(&store, &auth_lease, &auth_binding)
            .await
            .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("AuthMachine lifecycle release failed"));
        assert!(
            store.load(&key).await.unwrap().is_some(),
            "token clear must not commit when AuthMachine release fails"
        );
    }

    #[tokio::test]
    async fn rest_auth_status_hides_stale_token_without_auth_machine_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        state
            .token_store
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

        let detail = auth_status_detail(
            get_auth_status(
                State(state),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Unknown);
        assert_eq!(detail.expires_at, None);
        assert_eq!(detail.last_refresh_at, None);
        assert_eq!(detail.account_id, None);
        assert!(!detail.has_refresh_token);
    }

    #[tokio::test]
    async fn rest_auth_status_does_not_rehydrate_marked_oauth_token_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(
                config_with_openai_oauth_binding(CredentialSourceSpec::ManagedStore),
                None,
            )
            .await
            .unwrap();
        let auth_binding = openai_auth_binding();
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let tokens = chatgpt_oauth_tokens_with_secret("fresh-chatgpt-access");
        state
            .token_store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &meerkat_core::mark_tokens_lifecycle_published_for_generation(&tokens, 1),
            )
            .await
            .unwrap();

        let detail = auth_status_detail(
            get_auth_status(
                State(state.clone()),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Unknown);
        assert_eq!(detail.expires_at, None);
        assert_eq!(detail.account_id, None);
        assert!(!detail.has_refresh_token);
        let snapshot = state.auth_lease.snapshot(&lease_key);
        assert_eq!(snapshot.phase, None);
        assert!(!snapshot.credential_present);
    }

    #[tokio::test]
    async fn rest_auth_status_reports_lease_phase_when_token_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();

        let detail = auth_status_detail(
            get_auth_status(
                State(state),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Valid);
        assert!(detail.expires_at.is_some());
        assert!(!detail.has_refresh_token);
    }

    #[tokio::test]
    async fn rest_auth_status_hides_wrong_mode_token_even_with_auth_machine_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(
                config_with_openai_oauth_binding(CredentialSourceSpec::ManagedStore),
                None,
            )
            .await
            .unwrap();
        let auth_binding = openai_auth_binding();
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        state
            .token_store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &PersistedTokens::api_key("sk-stale-api-key"),
            )
            .await
            .unwrap();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();

        let detail = auth_status_detail(
            get_auth_status(
                State(state),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Unknown);
        assert_eq!(detail.expires_at, None);
        assert_eq!(detail.last_refresh_at, None);
        assert_eq!(detail.account_id, None);
        assert!(!detail.has_refresh_token);
    }

    #[tokio::test]
    async fn rest_auth_status_hides_wrong_source_oauth_token_even_with_auth_machine_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(
                config_with_openai_oauth_binding(CredentialSourceSpec::ExternalResolver {
                    handle: "external-chatgpt".into(),
                }),
                None,
            )
            .await
            .unwrap();
        let auth_binding = openai_auth_binding();
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        state
            .token_store
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
        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();

        let detail = auth_status_detail(
            get_auth_status(
                State(state),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Unknown);
        assert_eq!(detail.expires_at, None);
        assert_eq!(detail.last_refresh_at, None);
        assert_eq!(detail.account_id, None);
        assert!(!detail.has_refresh_token);
    }

    #[tokio::test]
    async fn rest_auth_status_ignores_stale_token_for_non_persisted_source_without_hiding_lifecycle()
     {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(config_with_openai_external_authorizer_binding(), None)
            .await
            .unwrap();
        let auth_binding = openai_auth_binding();
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        state
            .token_store
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
        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();

        let detail = auth_status_detail(
            get_auth_status(
                State(state),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Valid);
        assert!(detail.expires_at.is_some());
        assert_eq!(detail.last_refresh_at, None);
        assert_eq!(detail.account_id, None);
        assert!(!detail.has_refresh_token);
    }

    #[tokio::test]
    async fn rest_auth_status_reports_lease_phase_when_token_load_fails() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state.token_store = Arc::new(LoadFailingTokenStore);
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();

        let detail = auth_status_detail(
            get_auth_status(
                State(state),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Valid);
        assert!(detail.expires_at.is_some());
        assert_eq!(detail.account_id, None);
        assert!(!detail.has_refresh_token);
    }

    #[tokio::test]
    async fn rest_auth_status_observes_session_runtime_binding_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let now = chrono::Utc::now().timestamp() as u64;
        let query = || RealmQuery {
            realm_id: RealmId::parse("dev").unwrap(),
            profile_id: None,
        };
        let binding_id = || BindingId::parse("default_openai").unwrap();
        state
            .token_store
            .save(
                &TokenKey::from_auth_binding(&auth_binding),
                &PersistedTokens::api_key("sk-live"),
            )
            .await
            .unwrap();

        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease()
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();
        let detail = auth_status_detail(
            get_auth_status(State(state.clone()), Path(binding_id()), Query(query())).await,
        )
        .await;
        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Valid);

        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings.auth_lease().begin_refresh(&lease_key).unwrap();
        let detail = auth_status_detail(
            get_auth_status(State(state.clone()), Path(binding_id()), Query(query())).await,
        )
        .await;
        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::Expiring);

        bindings
            .auth_lease()
            .complete_refresh(&lease_key, now + 7200, now)
            .unwrap();
        bindings
            .auth_lease()
            .mark_reauth_required(&lease_key)
            .unwrap();
        let detail = auth_status_detail(
            get_auth_status(State(state), Path(binding_id()), Query(query())).await,
        )
        .await;
        assert_eq!(detail.state, meerkat_core::AuthStatusPhase::ReauthRequired);
    }

    #[test]
    fn login_complete_body_requires_explicit_identity() {
        let err = serde_json::from_value::<LoginCompleteBody>(serde_json::json!({
            "provider": "anthropic",
            "code": "code",
            "state": "state",
            "redirect_uri": "http://127.0.0.1:0/callback"
        }))
        .unwrap_err();

        assert!(err.to_string().contains("realm_id"));
    }

    #[test]
    fn device_complete_body_requires_explicit_identity() {
        let err = serde_json::from_value::<DeviceCompleteBody>(serde_json::json!({
            "provider": "anthropic",
            "device_code": "device-code"
        }))
        .unwrap_err();

        assert!(err.to_string().contains("realm_id"));
    }
}
