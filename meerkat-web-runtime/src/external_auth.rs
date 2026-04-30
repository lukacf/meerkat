//! External-auth resolver seam for the browser runtime (Phase 4d.wasm.1).
//!
//! The browser cannot persist OAuth refresh tokens safely — Service
//! Worker / IndexedDB storage is exposed to any extension or
//! XSS-hijacked script. Instead, the host page owns the OAuth flow
//! (browser redirect, PKCE, token endpoint exchange) and hands the
//! meerkat WASM runtime a *resolved auth envelope* via a JS callback.
//!
//! This module defines:
//!
//! - JS callback slot — wasm_bindgen-exposed registration that stores a
//!   host callback returning a Promise of a typed auth envelope. The callback
//!   survives across WASM calls (registered once, consulted
//!   per-session).
//! - [`register_external_auth_resolver`] — wasm_bindgen entry point
//!   that the host page calls to install the resolver.
//! - [`build_session_request_with_connection_ref`] — the post-§6.14
//!   session-request builder that routes through the provider-runtime
//!   registry. When the resolved binding uses an `ExternalResolver`
//!   credential source, the registry looks up this resolver id's callback,
//!   awaits it, and wires the returned typed lease into the LLM client's
//!   Authorization header.
//!
//! The callback shape matches plan §Phase 4d.wasm:
//!
//! ```js
//! // Host-page registration:
//! meerkat.register_external_auth_resolver(async (connectionRef) => {
//!   // host-owned OAuth flow; returns a typed lease envelope
//!   const token = await my_oauth_client.fresh_token_for(connectionRef);
//!   return {
//!     kind: "inline_secret",
//!     secret: token.accessToken,
//!     metadata: { account_id: token.accountId },
//!     expires_at: token.expiresAt,
//!   };
//! });
//! ```
//!
//! Hosts must resolve the Promise to a structured auth envelope so lease
//! metadata and expiration remain explicit. To preserve typed failures,
//! reject the Promise with a wire auth error object such as
//! `{ kind: "interactive_login_required" }` or
//! `{ kind: "refresh_failed", detail: "token endpoint returned 401" }`.

#[cfg(target_arch = "wasm32")]
use js_sys::{Function, Promise};
#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// Single host-registered resolver for this WASM instance. The
    /// browser's single-threaded model makes `thread_local` sufficient;
    /// all meerkat runtime calls happen on the main JS thread.
    static EXTERNAL_AUTH_RESOLVER: RefCell<Option<Function>> = const { RefCell::new(None) };
}

/// Canonical external-auth resolver id installed by the WASM runtime.
/// Realm configs that want host-owned browser auth use
/// `CredentialSourceSpec::ExternalResolver { handle: "wasm_host" }`.
pub const WASM_EXTERNAL_AUTH_RESOLVER_ID: &str = "wasm_host";

/// Back-compat alias for SDKs/examples that still refer to the
/// credential-source discriminator as a handle.
pub const WASM_EXTERNAL_AUTH_RESOLVER_HANDLE: &str = WASM_EXTERNAL_AUTH_RESOLVER_ID;

/// Register a JS-side external-auth resolver. The callback receives a
/// structural connection reference argument (`{ realm, binding, profile? }`)
/// and must return a Promise that resolves to a JSON-serializable
/// `ResolvedAuthEnvelope` object.
///
/// Lease-bearing credentials must use the typed object form:
/// `{ kind: "inline_secret", secret, metadata, expires_at? }`.
/// Structured failures should reject the Promise with a wire auth error
/// object, e.g. `{ kind: "interactive_login_required" }` or
/// `{ kind: "refresh_failed", detail }`.
///
/// Subsequent registrations overwrite the previous one. Passing
/// `undefined` clears the registration.
///
/// Plan §Phase 4d.wasm.1.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn register_external_auth_resolver(callback: JsValue) -> Result<(), JsValue> {
    if callback.is_undefined() || callback.is_null() {
        EXTERNAL_AUTH_RESOLVER.with(|slot| *slot.borrow_mut() = None);
        return Ok(());
    }
    let function: Function = callback
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str("external_auth: callback must be a function"))?;
    EXTERNAL_AUTH_RESOLVER.with(|slot| *slot.borrow_mut() = Some(function));
    Ok(())
}

/// Returns `true` if a JS-side external-auth resolver has been
/// registered. Exposed to the browser so host pages can verify their
/// registration before creating a session.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn has_external_auth_resolver() -> bool {
    EXTERNAL_AUTH_RESOLVER.with(|slot| slot.borrow().is_some())
}

/// Invoke the registered external-auth resolver for a connection reference.
/// Returns the JS Promise the callback produced, or an error if no
/// resolver is registered.
///
/// Caller awaits the Promise via wasm-bindgen-futures to obtain the
/// typed auth envelope.
#[cfg(target_arch = "wasm32")]
#[allow(dead_code)] // wired in when the provider-runtime-registry WASM path consumes it
pub(crate) fn invoke_external_auth_resolver(
    connection_ref: &meerkat_core::ConnectionRef,
) -> Result<Promise, ExternalAuthInvokeError> {
    EXTERNAL_AUTH_RESOLVER.with(|slot| {
        let slot = slot.borrow();
        let callback = slot.as_ref().ok_or(ExternalAuthInvokeError::NoResolver)?;
        let this = JsValue::NULL;
        let result = callback
            .call1(&this, &connection_ref_js_value(connection_ref))
            .map_err(ExternalAuthInvokeError::Callback)?;
        result
            .dyn_into::<Promise>()
            .map_err(|_| ExternalAuthInvokeError::NonPromise)
    })
}

#[cfg(target_arch = "wasm32")]
pub(crate) enum ExternalAuthInvokeError {
    NoResolver,
    Callback(JsValue),
    NonPromise,
}

#[cfg(target_arch = "wasm32")]
fn connection_ref_js_value(connection_ref: &meerkat_core::ConnectionRef) -> JsValue {
    let object = js_sys::Object::new();
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("realm"),
        &JsValue::from_str(connection_ref.realm.as_str()),
    )
    .expect("setting property on fresh object should succeed");
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("binding"),
        &JsValue::from_str(connection_ref.binding.as_str()),
    )
    .expect("setting property on fresh object should succeed");
    if let Some(profile) = &connection_ref.profile {
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("profile"),
            &JsValue::from_str(profile.as_str()),
        )
        .expect("setting property on fresh object should succeed");
    }
    object.into()
}

/// Rust-side bridge that implements `ExternalAuthResolverHandle` by
/// delegating to the JS callback registered via
/// `register_external_auth_resolver`. Registered on `AgentFactory` with
/// [`WASM_EXTERNAL_AUTH_RESOLVER_ID`] during WASM runtime init. Realm
/// bindings configured with
/// `CredentialSourceSpec::ExternalResolver { handle: "wasm_host" }`
/// therefore delegate credential resolution to the JS host's OAuth
/// flow.
#[cfg(target_arch = "wasm32")]
pub struct WasmExternalAuthResolver;

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl meerkat_providers::ExternalAuthResolverHandle for WasmExternalAuthResolver {
    async fn resolve(
        &self,
        binding: &meerkat_providers::ValidatedBinding,
    ) -> Result<meerkat_core::ResolvedAuthEnvelope, meerkat_core::AuthError> {
        let promise = invoke_external_auth_resolver(&binding.connection_ref)
            .map_err(auth_error_from_invoke_error)?;
        let js_value = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(auth_error_from_rejection)?;
        auth_envelope_from_js_value(js_value)
    }
}

#[cfg(target_arch = "wasm32")]
fn js_value_display(v: &JsValue) -> String {
    v.as_string()
        .or_else(|| v.dyn_ref::<js_sys::Error>().map(|e| e.message().into()))
        .unwrap_or_else(|| format!("{v:?}"))
}

#[cfg(target_arch = "wasm32")]
fn auth_error_from_invoke_error(error: ExternalAuthInvokeError) -> meerkat_core::AuthError {
    match error {
        ExternalAuthInvokeError::NoResolver => meerkat_core::AuthError::MissingSecret,
        ExternalAuthInvokeError::Callback(e) => meerkat_core::AuthError::Other(format!(
            "{WASM_EXTERNAL_AUTH_RESOLVER_ID} resolver callback failed: {}",
            js_value_display(&e),
        )),
        ExternalAuthInvokeError::NonPromise => meerkat_core::AuthError::Other(format!(
            "{WASM_EXTERNAL_AUTH_RESOLVER_ID} resolver callback must return a Promise",
        )),
    }
}

#[cfg(target_arch = "wasm32")]
fn auth_error_from_rejection(value: JsValue) -> meerkat_core::AuthError {
    if let Some(error) = wire_auth_error_from_js_value(&value) {
        return auth_error_from_wire(error);
    }

    meerkat_core::AuthError::Other(format!(
        "{WASM_EXTERNAL_AUTH_RESOLVER_ID} resolver rejected: {}",
        js_value_display(&value),
    ))
}

#[cfg(target_arch = "wasm32")]
fn auth_envelope_from_js_value(
    value: JsValue,
) -> Result<meerkat_core::ResolvedAuthEnvelope, meerkat_core::AuthError> {
    if let Some(token) = value.as_string() {
        return bare_bearer_string_error(token);
    }

    if value.is_null() || value.is_undefined() {
        return Err(meerkat_core::AuthError::MissingSecret);
    }

    let raw = json_string_from_js_value(&value).map_err(|detail| {
        meerkat_core::AuthError::Other(format!(
            "{WASM_EXTERNAL_AUTH_RESOLVER_ID} resolver returned unsupported credential envelope: {detail}",
        ))
    })?;
    let envelope =
        serde_json::from_str::<meerkat_core::ResolvedAuthEnvelope>(&raw).map_err(|e| {
            meerkat_core::AuthError::Other(format!(
                "{WASM_EXTERNAL_AUTH_RESOLVER_ID} resolver returned unsupported credential envelope: {e}",
            ))
        })?;
    validate_auth_envelope(envelope)
}

#[cfg(target_arch = "wasm32")]
fn bare_bearer_string_error(
    token: String,
) -> Result<meerkat_core::ResolvedAuthEnvelope, meerkat_core::AuthError> {
    if token.trim().is_empty() {
        return Err(meerkat_core::AuthError::MissingSecret);
    }

    Err(meerkat_core::AuthError::Other(format!(
        "{WASM_EXTERNAL_AUTH_RESOLVER_ID} resolver returned a bare bearer string; return a structured auth envelope object instead",
    )))
}

#[cfg(target_arch = "wasm32")]
fn validate_auth_envelope(
    envelope: meerkat_core::ResolvedAuthEnvelope,
) -> Result<meerkat_core::ResolvedAuthEnvelope, meerkat_core::AuthError> {
    match &envelope {
        meerkat_core::ResolvedAuthEnvelope::InlineSecret { secret, .. } => {
            if secret.trim().is_empty() {
                return Err(meerkat_core::AuthError::MissingSecret);
            }
        }
        meerkat_core::ResolvedAuthEnvelope::StaticHeaders { headers, .. } => {
            if headers.is_empty()
                || headers
                    .iter()
                    .any(|(name, value)| name.trim().is_empty() || value.trim().is_empty())
            {
                return Err(meerkat_core::AuthError::MissingSecret);
            }
        }
        meerkat_core::ResolvedAuthEnvelope::DynamicAuthorizer { .. }
        | meerkat_core::ResolvedAuthEnvelope::None { .. } => {}
    }

    Ok(envelope)
}

#[cfg(target_arch = "wasm32")]
fn wire_auth_error_from_js_value(value: &JsValue) -> Option<meerkat_contracts::WireAuthError> {
    let raw = json_string_from_js_value(value).ok()?;
    serde_json::from_str::<meerkat_contracts::WireAuthError>(&raw).ok()
}

#[cfg(target_arch = "wasm32")]
fn json_string_from_js_value(value: &JsValue) -> Result<String, String> {
    if let Some(raw) = value.as_string() {
        return Ok(raw);
    }

    let stringified = js_sys::JSON::stringify(value)
        .map_err(|e| format!("JSON.stringify failed: {}", js_value_display(&e)))?;
    stringified
        .as_string()
        .ok_or_else(|| "JSON.stringify returned undefined".to_string())
}

#[cfg(target_arch = "wasm32")]
fn auth_error_from_wire(error: meerkat_contracts::WireAuthError) -> meerkat_core::AuthError {
    match error {
        meerkat_contracts::WireAuthError::MissingSecret => meerkat_core::AuthError::MissingSecret,
        meerkat_contracts::WireAuthError::UnsupportedCombination { backend, auth } => {
            meerkat_core::AuthError::UnsupportedCombination { backend, auth }
        }
        meerkat_contracts::WireAuthError::MissingRequiredMetadata { field } => {
            meerkat_core::AuthError::MissingRequiredMetadata(field)
        }
        meerkat_contracts::WireAuthError::WorkspaceMismatch => {
            meerkat_core::AuthError::WorkspaceMismatch
        }
        meerkat_contracts::WireAuthError::Expired => meerkat_core::AuthError::Expired,
        meerkat_contracts::WireAuthError::RefreshFailed { detail } => {
            meerkat_core::AuthError::RefreshFailed(detail)
        }
        meerkat_contracts::WireAuthError::InteractiveLoginRequired => {
            meerkat_core::AuthError::InteractiveLoginRequired
        }
        meerkat_contracts::WireAuthError::HostOwnedUnavailable => {
            meerkat_core::AuthError::HostOwnedUnavailable
        }
        meerkat_contracts::WireAuthError::Io { detail } => meerkat_core::AuthError::Io(detail),
        meerkat_contracts::WireAuthError::Other { detail } => {
            meerkat_core::AuthError::Other(detail)
        }
    }
}

// ---------------------------------------------------------------------------
// Non-wasm shim: the browser-specific types don't exist off wasm32. Expose
// no-op stubs so unit tests can link without cfg-gating every call site.
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn has_external_auth_resolver() -> bool {
    false
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;

    /// On non-wasm targets, the resolver seam reports "not registered"
    /// because the JS host doesn't exist. Unit-test coverage of the
    /// registration path lives in the wasm-bindgen integration test
    /// `tests/wasm_external_resolver.rs`.
    #[test]
    fn non_wasm_resolver_reports_not_registered() {
        assert!(
            !has_external_auth_resolver(),
            "non-wasm builds must not claim a registered resolver"
        );
    }
}
