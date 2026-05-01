//! `auth/*` + `realm/*` method handlers.
//!
//! Real implementations using the shared `SessionRuntime.token_store()`.
//! OAuth login is split across two calls (server keeps state -> PKCE verifier):
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
    ConnectionRef, ConnectionTargetError, CredentialSourceSpec, Provider, RealmConnectionSet,
    ResolvedConnectionTarget,
};
use meerkat_providers::auth_oauth::{
    DevicePollOutcome, OAuthError, PkcePair, exchange_authorization_code, poll_device_code,
    request_device_code,
};
use meerkat_providers::auth_store::{PersistedAuthMode, PersistedTokens, TokenKey, TokenStore};
use meerkat_providers::oauth_flow::{
    OAuthFlowError, global_oauth_flow_registry, resolve_oauth_provider,
};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

async fn load_config(runtime: &SessionRuntime) -> Result<meerkat_core::Config, RpcResponse> {
    if let Some(cfg_runtime) = runtime.config_runtime() {
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
            })
    } else {
        Ok(meerkat_core::Config::default())
    }
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
        ConnectionRef,
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
    let connection_ref = ConnectionRef {
        realm: realm_typed,
        binding: binding_typed,
        profile: profile_typed,
    };
    let (binding, _, auth_profile) = realm.lookup_connection_ref(&connection_ref).map_err(|e| {
        RpcResponse::error(
            None,
            error::INVALID_PARAMS,
            format!("Unknown auth identity {realm_id}:{binding_id}: {e}"),
        )
    })?;
    Ok((connection_ref, binding.clone(), auth_profile.clone()))
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
    meerkat_core::resolve_realm_binding_target_for_provider(
        &config,
        provider,
        explicit_realm.as_ref(),
        explicit_binding.as_ref(),
        explicit_profile.as_ref(),
        runtime.realm_id(),
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
    runtime.token_store().ok_or_else(|| {
        RpcResponse::error(
            id.clone(),
            error::INTERNAL_ERROR,
            "TokenStore not configured for this runtime",
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

async fn save_tokens_and_publish_lifecycle(
    id: Option<RpcId>,
    store: &Arc<dyn TokenStore>,
    runtime: &SessionRuntime,
    connection_ref: &ConnectionRef,
    tokens: &PersistedTokens,
) -> Result<(), RpcResponse> {
    let key = TokenKey::from_connection_ref(connection_ref);
    let previous = match store.load(&key).await {
        Ok(previous) => previous,
        Err(e) => {
            return Err(RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("TokenStore load failed: {e}"),
            ));
        }
    };
    if let Err(e) = store.save(&key, tokens).await {
        return Err(RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("TokenStore save failed: {e}"),
        ));
    }
    let auth_lease = runtime.auth_lease_handle();
    publish_saved_tokens_and_restore_on_lifecycle_failure(
        id,
        store.as_ref(),
        auth_lease.as_ref(),
        connection_ref,
        &key,
        tokens,
        previous.as_ref(),
    )
    .await
}

async fn publish_saved_tokens_and_restore_on_lifecycle_failure(
    id: Option<RpcId>,
    store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    connection_ref: &ConnectionRef,
    key: &TokenKey,
    tokens: &PersistedTokens,
    previous: Option<&PersistedTokens>,
) -> Result<(), RpcResponse> {
    if let Err(e) =
        meerkat_core::publish_token_lifecycle_acquired(auth_lease, connection_ref, tokens)
    {
        if let Err(rollback_error) =
            restore_tokens_after_lifecycle_failure(store, key, previous).await
        {
            return Err(RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!(
                    "AuthMachine lifecycle acquire failed: {e}; TokenStore rollback failed: {rollback_error}"
                ),
            ));
        }
        return Err(RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("AuthMachine lifecycle acquire failed: {e}"),
        ));
    }
    Ok(())
}

async fn restore_tokens_after_lifecycle_failure(
    store: &dyn TokenStore,
    key: &TokenKey,
    previous: Option<&PersistedTokens>,
) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
    match previous {
        Some(tokens) => store.save(key, tokens).await,
        None => store.clear(key).await,
    }
}

async fn clear_tokens_and_publish_lifecycle(
    id: Option<RpcId>,
    store: &Arc<dyn TokenStore>,
    runtime: &SessionRuntime,
    connection_ref: &ConnectionRef,
) -> Result<(), RpcResponse> {
    let auth_lease = runtime.auth_lease_handle();
    meerkat_core::clear_tokens_and_publish_lifecycle_released(
        store.as_ref(),
        auth_lease.as_ref(),
        connection_ref,
    )
    .await
    .map_err(|e| RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()))
}

// --- Realm projection -------------------------------------------------

pub async fn handle_realm_list(id: Option<RpcId>, runtime: &SessionRuntime) -> RpcResponse {
    let config = match load_config(runtime).await {
        Ok(c) => c,
        Err(r) => return r.with_id(id),
    };
    let realms: Vec<serde_json::Value> = config
        .realm
        .iter()
        .map(|(realm_id, section)| {
            serde_json::json!({
                "realm_id": realm_id,
                "default_binding": section.default_binding,
                "backend_count": section.backend.len(),
                "auth_profile_count": section.auth.len(),
                "binding_count": section.binding.len(),
            })
        })
        .collect();
    RpcResponse::success(id, serde_json::json!({ "realms": realms }))
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
        serde_json::json!({
            "realm_id": realm.realm_id,
            "auth_profiles": profiles,
            "backend_profiles": backends,
            "bindings": bindings,
        }),
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
        Ok((connection_ref, binding, auth_profile)) => RpcResponse::success(
            id,
            serde_json::json!({
                "connection_ref": &connection_ref,
                "binding_id": &binding.id,
                "profile_id": &auth_profile.id,
                "auth_profile": WireAuthProfile::from(&auth_profile),
            }),
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
    let auth_mode = match parsed.auth_method.as_str() {
        "api_key" => PersistedAuthMode::ApiKey,
        "static_bearer" => PersistedAuthMode::StaticBearer,
        other => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!(
                    "auth_method '{other}' cannot be created via RPC. \
                     OAuth methods use auth/login/start + auth/login/complete; \
                     managed_store and external_resolver are configured via TOML."
                ),
            );
        }
    };
    let (connection_ref, _binding, auth_profile) = match resolve_binding_identity(
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
    if let Some(r) = require_managed_store_source(id.clone(), &parsed.binding_id, &auth_profile) {
        return r;
    }
    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
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
        save_tokens_and_publish_lifecycle(id.clone(), &store, runtime, &connection_ref, &tokens)
            .await
    {
        return resp;
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?connection_ref,
        action = "create_profile",
        provider = %auth_profile.provider.as_str(),
        auth_method = %auth_profile.auth_method,
        "binding-scoped auth credentials stored via RPC"
    );
    RpcResponse::success(
        id,
        serde_json::json!({
            "realm_id": connection_ref.realm.as_str(),
            "binding_id": connection_ref.binding.as_str(),
            "connection_ref": &connection_ref,
            "profile_id": &auth_profile.id,
            "provider": auth_profile.provider.as_str(),
            "auth_method": &auth_profile.auth_method,
            "stored": true,
        }),
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
    let (connection_ref, _binding, auth_profile) = match resolve_binding_identity(
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
    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(resp) =
        clear_tokens_and_publish_lifecycle(id.clone(), &store, runtime, &connection_ref).await
    {
        return resp;
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?connection_ref,
        action = "delete_profile",
        "binding-scoped auth credentials deleted via RPC"
    );
    RpcResponse::success(
        id,
        serde_json::json!({
            "realm_id": connection_ref.realm.as_str(),
            "binding_id": connection_ref.binding.as_str(),
            "connection_ref": &connection_ref,
            "profile_id": &auth_profile.id,
            "cleared": true,
        }),
    )
}

// --- OAuth login ------------------------------------------------------

pub async fn handle_auth_login_start(id: Option<RpcId>, params: Option<&RawValue>) -> RpcResponse {
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
    let pkce = PkcePair::generate_s256();
    let verifier = pkce.verifier.secret().clone();
    let state_token = match global_oauth_flow_registry().start(
        resolved.identity,
        parsed.redirect_uri.clone(),
        verifier,
    ) {
        Ok(state) => state,
        Err(OAuthFlowError::CapacityExceeded { .. }) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                "oauth state registry is at capacity",
            );
        }
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
        serde_json::json!({
            "authorize_url": authorize_url,
            "state": state_token,
            "redirect_uri": parsed.redirect_uri,
            "provider": parsed.provider,
        }),
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
    let (realm_id, binding_id) = match require_explicit_oauth_identity(
        id.clone(),
        parsed.realm_id.as_deref(),
        parsed.binding_id.as_deref(),
    ) {
        Ok(v) => v,
        Err(r) => return r,
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
        Some(realm_id),
        Some(binding_id),
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let connection_ref = target.connection_ref;
    let binding = target.binding;
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

    let flow = match global_oauth_flow_registry().consume(
        &parsed.state,
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
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("oauth state verification failed: {e}"),
            );
        }
    };

    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let http = reqwest::Client::new();
    let result = match exchange_authorization_code(
        &http,
        &resolved.endpoints,
        &parsed.code,
        &flow.pkce_verifier,
        resolved.client_secret,
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
    let expires_at = result
        .expires_in_secs
        .map(|s| chrono::Utc::now() + chrono::Duration::seconds(s as i64));
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
    if let Err(resp) =
        save_tokens_and_publish_lifecycle(id.clone(), &store, runtime, &connection_ref, &tokens)
            .await
    {
        return resp;
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?connection_ref,
        action = "login_oauth_complete",
        provider = %parsed.provider,
        has_refresh_token = %tokens.refresh_token.is_some(),
        "OAuth login completed via RPC"
    );
    RpcResponse::success(
        id,
        serde_json::json!({
            "realm_id": connection_ref.realm.as_str(),
            "binding_id": connection_ref.binding.as_str(),
            "connection_ref": &connection_ref,
            "profile_id": &auth_profile.id,
            "provider": parsed.provider,
            "expires_at": expires_at.map(|e| e.to_rfc3339()),
            "has_refresh_token": tokens.refresh_token.is_some(),
            "scopes": tokens.scopes,
        }),
    )
}

pub async fn handle_auth_login_device_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
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
    let http = reqwest::Client::new();
    match request_device_code(&http, &resolved.endpoints).await {
        Ok(resp) => RpcResponse::success(
            id,
            serde_json::json!({
                "device_code": resp.device_code,
                "user_code": resp.user_code,
                "verification_uri": resp.verification_uri,
                "verification_uri_complete": resp.verification_uri_complete,
                "expires_in": resp.expires_in,
                "interval": resp.interval,
                "provider": parsed.provider,
            }),
        ),
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
///     (tokens persisted to TokenStore under the resolved `ConnectionRef`).
pub async fn handle_auth_login_device_complete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: DeviceCompleteParams = match parse_params(params) {
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
        Some(realm_id),
        Some(binding_id),
        parsed.profile_id.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let connection_ref = target.connection_ref;
    let binding = target.binding;
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
        DevicePollOutcome::Pending => RpcResponse::success(id, WireDeviceCompleteResult::Pending),
        DevicePollOutcome::SlowDown => RpcResponse::success(id, WireDeviceCompleteResult::SlowDown),
        DevicePollOutcome::AccessDenied => {
            RpcResponse::success(id, WireDeviceCompleteResult::AccessDenied)
        }
        DevicePollOutcome::Expired => RpcResponse::success(id, WireDeviceCompleteResult::Expired),
        DevicePollOutcome::Ready(result) => {
            let store = match require_token_store(runtime, id.clone()) {
                Ok(s) => s,
                Err(r) => return r,
            };
            let expires_at = result
                .expires_in_secs
                .map(|s| chrono::Utc::now() + chrono::Duration::seconds(s as i64));
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
            if let Err(resp) = save_tokens_and_publish_lifecycle(
                id.clone(),
                &store,
                runtime,
                &connection_ref,
                &tokens,
            )
            .await
            {
                return resp;
            }
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "login_device_complete",
                provider = %parsed.provider,
                has_refresh_token = %tokens.refresh_token.is_some(),
                "OAuth device-flow login completed via RPC"
            );
            RpcResponse::success(
                id,
                WireDeviceCompleteResult::Ready {
                    identity: Box::new(WireBindingIdentity::from(&connection_ref)),
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
/// resolved `ConnectionRef` with auth_mode=OauthToApiKey — so future
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
    let connection_ref = target.connection_ref;
    let auth_profile = target.auth_profile;
    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let key = TokenKey::from_connection_ref(&connection_ref);
    let previous = match store.load(&key).await {
        Ok(previous) => previous,
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("TokenStore load failed: {e}"),
            );
        }
    };
    // Console endpoints drive `scope = org:create_api_key user:profile`.
    // The runtime wrapper's `provision_api_key` POSTs to
    // API_KEY_CREATE_URL with `Authorization: Bearer <access_token>`
    // and persists the returned api_key via `save_persisted`.
    let endpoints = a_oauth::console_endpoints(a_oauth::MANUAL_REDIRECT_URL);
    let oauth_runtime = a_oauth::AnthropicOAuthRuntime::new_with_default_coordinator(
        Arc::clone(&store),
        endpoints,
        key.clone(),
    );
    match oauth_runtime.provision_api_key(&parsed.access_token).await {
        Ok(tokens) => {
            let auth_lease = runtime.auth_lease_handle();
            if let Err(resp) = publish_saved_tokens_and_restore_on_lifecycle_failure(
                id.clone(),
                store.as_ref(),
                auth_lease.as_ref(),
                &connection_ref,
                &key,
                &tokens,
                previous.as_ref(),
            )
            .await
            {
                return resp;
            }
            RpcResponse::success(
                id,
                WireProvisionApiKeyResult {
                    identity: WireBindingIdentity::from(&connection_ref),
                    profile_id: auth_profile.id,
                    provider: "anthropic".to_string(),
                    auth_mode: "oauth_to_api_key".to_string(),
                    has_api_key: tokens.primary_secret.is_some(),
                    scopes: tokens.scopes,
                },
            )
        }
        Err(e) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("provision_api_key failed: {e}"),
        ),
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
    let (connection_ref, _binding, auth_profile) = match resolve_binding_identity(
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
    let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&connection_ref));
    let now = chrono::Utc::now();
    let phase = meerkat_core::AuthStatusPhase::from_lease_snapshot(now, &snapshot);
    let stored = if phase == meerkat_core::AuthStatusPhase::Unknown {
        None
    } else if let Some(store) = runtime.token_store() {
        store
            .load(&TokenKey::from_connection_ref(&connection_ref))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let projection = meerkat_core::project_published_auth_status(now, stored.as_ref(), &snapshot);
    let tokens = projection.tokens;
    RpcResponse::success(
        id,
        WireAuthStatusDetail {
            identity: WireBindingIdentity::from(&connection_ref),
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
    let (connection_ref, _binding, auth_profile) = match resolve_binding_identity(
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
    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(resp) =
        clear_tokens_and_publish_lifecycle(id.clone(), &store, runtime, &connection_ref).await
    {
        return resp;
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?connection_ref,
        action = "logout",
        "binding-scoped auth credentials logged out via RPC"
    );
    RpcResponse::success(
        id,
        serde_json::json!({
            "realm_id": connection_ref.realm.as_str(),
            "binding_id": connection_ref.binding.as_str(),
            "connection_ref": &connection_ref,
            "profile_id": &auth_profile.id,
            "cleared": true,
        }),
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::handles::{
        AuthLeaseHandle, AuthLeaseSnapshot, AuthLeaseTransition, DslTransitionError,
    };

    fn raw_params(value: serde_json::Value) -> Box<RawValue> {
        serde_json::value::to_raw_value(&value).unwrap()
    }

    fn test_runtime() -> SessionRuntime {
        test_runtime_with_config(meerkat_core::Config::default())
    }

    fn test_runtime_with_config(config: meerkat_core::Config) -> SessionRuntime {
        let token_store: Arc<dyn TokenStore> =
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new());
        test_runtime_with_config_and_token_store(config, token_store)
    }

    fn test_runtime_with_config_and_token_store(
        config: meerkat_core::Config,
        token_store: Arc<dyn TokenStore>,
    ) -> SessionRuntime {
        let temp = tempfile::tempdir().unwrap();
        let factory =
            meerkat::AgentFactory::new(temp.path().join("sessions")).with_token_store(token_store);
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let config_store: Arc<dyn meerkat_core::ConfigStore> =
            Arc::new(meerkat_core::MemoryConfigStore::new(config.clone()));
        let mut runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(store, None, blob_store),
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
                backend_kind: "openai_api".into(),
                base_url: None,
                options: serde_json::Value::Null,
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
            Ok(AuthLeaseTransition { generation: 1 })
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
                generation: 0,
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
            Ok(AuthLeaseTransition { generation: 1 })
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
            Ok(AuthLeaseTransition { generation: 1 })
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
                phase: Some(meerkat_core::handles::AuthLeasePhase::Valid),
                expires_at: Some(1_800_000_000),
                generation: 1,
            }
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

    #[test]
    fn oauth_completion_params_keep_missing_identity_unowned_by_surface() {
        let login: LoginCompleteParams = serde_json::from_value(serde_json::json!({
            "provider": "anthropic",
            "code": "code",
            "state": "state",
            "redirect_uri": "http://127.0.0.1:0/callback"
        }))
        .unwrap();
        assert!(login.realm_id.is_none());
        assert!(login.binding_id.is_none());

        let device: DeviceCompleteParams = serde_json::from_value(serde_json::json!({
            "provider": "anthropic",
            "device_code": "device-code"
        }))
        .unwrap();
        assert!(device.realm_id.is_none());
        assert!(device.binding_id.is_none());

        let provision: ProvisionApiKeyParams = serde_json::from_value(serde_json::json!({
            "access_token": "token"
        }))
        .unwrap();
        assert!(provision.realm_id.is_none());
        assert!(provision.binding_id.is_none());
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

        assert_invalid_params_message(resp, "realm_id is required for OAuth login completion");
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

        assert_invalid_params_message(resp, "realm_id is required for OAuth login completion");
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

        assert_invalid_params_message(resp, "realm_id is required for OAuth login completion");
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
        let connection_ref = ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let store = runtime.token_store().expect("test token store");
        store
            .save(
                &TokenKey::from_connection_ref(&connection_ref),
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
        assert_eq!(status["state"], "unknown");
        assert!(status.get("expires_at").is_none());
        assert!(status.get("last_refresh_at").is_none());
        assert!(status.get("account_id").is_none());
        assert_eq!(status["has_refresh_token"], false);
    }

    #[tokio::test]
    async fn auth_status_reports_lease_phase_when_token_is_missing() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let connection_ref = ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        let now = chrono::Utc::now().timestamp() as u64;
        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease
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
    async fn auth_status_reports_lease_phase_when_token_load_fails() {
        let runtime = test_runtime_with_config_and_token_store(
            config_with_openai_managed_store_binding(),
            Arc::new(LoadFailingTokenStore),
        );
        let connection_ref = ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        let now = chrono::Utc::now().timestamp() as u64;
        let bindings = runtime
            .runtime_adapter()
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease
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
        assert!(status.get("account_id").is_none());
        assert_eq!(status["has_refresh_token"], false);
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
    async fn auth_profile_create_lifecycle_failure_restores_existing_credentials() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        runtime
            .runtime_adapter()
            .set_auth_lease_handle(Arc::new(RejectingAuthLeaseHandle));
        let connection_ref = ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let key = TokenKey::from_connection_ref(&connection_ref);
        let store = runtime.token_store().expect("test token store");
        store
            .save(&key, &PersistedTokens::api_key("sk-old"))
            .await
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai",
            "auth_method": "api_key",
            "secret": "sk-new"
        }));

        let resp =
            handle_auth_profile_create(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let error = resp.error.expect("create should fail");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("AuthMachine lifecycle acquire failed")
        );
        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-old"));
    }

    #[tokio::test]
    async fn auth_logout_release_failure_does_not_clear_tokens() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        runtime
            .runtime_adapter()
            .set_auth_lease_handle(Arc::new(ReleaseRejectingAuthLeaseHandle));
        let connection_ref = ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let key = TokenKey::from_connection_ref(&connection_ref);
        let store = runtime.token_store().expect("test token store");
        store
            .save(&key, &PersistedTokens::api_key("sk-old"))
            .await
            .unwrap();
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));

        let resp = handle_auth_logout(Some(RpcId::Num(1)), Some(params.as_ref()), &runtime).await;

        let error = resp.error.expect("logout should fail");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("AuthMachine lifecycle release failed")
        );
        let stored = store.load(&key).await.unwrap();
        assert!(
            stored.is_some(),
            "token clear must not commit when AuthMachine release fails"
        );
    }

    #[tokio::test]
    async fn provisioned_api_key_lifecycle_failure_restores_existing_credentials() {
        let store: Arc<dyn TokenStore> =
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new());
        let auth_lease = RejectingAuthLeaseHandle;
        let connection_ref = ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let key = TokenKey::from_connection_ref(&connection_ref);
        let previous = PersistedTokens::api_key("sk-old");
        let replacement = PersistedTokens::api_key("sk-new");
        store.save(&key, &previous).await.unwrap();
        let previous_snapshot = store.load(&key).await.unwrap();
        store.save(&key, &replacement).await.unwrap();

        let err = publish_saved_tokens_and_restore_on_lifecycle_failure(
            Some(RpcId::Num(1)),
            store.as_ref(),
            &auth_lease,
            &connection_ref,
            &key,
            &replacement,
            previous_snapshot.as_ref(),
        )
        .await
        .unwrap_err();

        let error = err.error.expect("publish should fail");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error
                .message
                .contains("AuthMachine lifecycle acquire failed")
        );
        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-old"));
    }

    #[tokio::test]
    async fn auth_status_observes_session_runtime_binding_lifecycle() {
        let runtime = test_runtime_with_config(config_with_openai_managed_store_binding());
        let connection_ref = ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        let now = chrono::Utc::now().timestamp() as u64;
        let params = raw_params(serde_json::json!({
            "realm_id": "dev",
            "binding_id": "default_openai"
        }));
        let store = runtime.token_store().expect("test token store");
        store
            .save(
                &TokenKey::from_connection_ref(&connection_ref),
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
            .auth_lease
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
        bindings.auth_lease.begin_refresh(&lease_key).unwrap();
        let status =
            handle_auth_status_get(Some(RpcId::Num(2)), Some(params.as_ref()), &runtime).await;
        assert_eq!(auth_status_state(status), "expiring");

        bindings
            .auth_lease
            .complete_refresh(&lease_key, now + 7200, now)
            .unwrap();
        bindings
            .auth_lease
            .mark_reauth_required(&lease_key)
            .unwrap();
        let status =
            handle_auth_status_get(Some(RpcId::Num(3)), Some(params.as_ref()), &runtime).await;
        assert_eq!(auth_status_state(status), "reauth_required");
    }

    #[tokio::test]
    async fn oauth_completion_requires_binding_when_realm_is_present() {
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

        assert_invalid_params_message(resp, "binding_id is required for OAuth login completion");
    }
}
