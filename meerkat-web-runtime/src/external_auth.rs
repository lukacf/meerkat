//! External-auth resolver seam for the browser runtime (Phase 4d.wasm.1).
//!
//! The browser cannot persist OAuth refresh tokens safely — Service
//! Worker / IndexedDB storage is exposed to any extension or
//! XSS-hijacked script. Instead, the host page owns the OAuth flow
//! (browser redirect, PKCE, token endpoint exchange) and hands the
//! meerkat WASM runtime a *resolved bearer token* via a JS callback.
//!
//! This module defines:
//!
//! - JS callback slot — wasm_bindgen-exposed registration that stores a
//!   host callback returning a Promise<string> of a bearer token. The
//!   callback survives across WASM calls (registered once, consulted
//!   per-session).
//! - [`register_external_auth_resolver`] — wasm_bindgen entry point
//!   that the host page calls to install the resolver.
//! - [`build_session_request_with_connection_ref`] — the post-§6.14
//!   session-request builder that routes through the provider-runtime
//!   registry. When the resolved binding uses an `ExternalResolver`
//!   credential source, the registry looks up this resolver id's callback,
//!   awaits it, and wires the returned token into the LLM client's
//!   Authorization header.
//!
//! The callback shape matches plan §Phase 4d.wasm:
//!
//! ```js
//! // Host-page registration:
//! meerkat.register_external_auth_resolver(async (connectionRef) => {
//!   // host-owned OAuth flow; returns a bearer string
//!   const token = await my_oauth_client.fresh_token_for(connectionRef);
//!   return token; // plain string; meerkat wraps as envelope internally
//! });
//! ```

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
/// and must return a Promise that resolves to a bearer-token string.
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
/// bearer token string.
#[cfg(target_arch = "wasm32")]
#[allow(dead_code)] // wired in when the provider-runtime-registry WASM path consumes it
pub(crate) fn invoke_external_auth_resolver(
    connection_ref: &meerkat_core::ConnectionRef,
) -> Result<Promise, JsValue> {
    EXTERNAL_AUTH_RESOLVER.with(|slot| {
        let slot = slot.borrow();
        let callback = slot.as_ref().ok_or_else(|| {
            JsValue::from_str(
                "external_auth: no resolver registered; call register_external_auth_resolver first",
            )
        })?;
        let this = JsValue::NULL;
        let result = callback.call1(&this, &connection_ref_js_value(connection_ref))?;
        result
            .dyn_into::<Promise>()
            .map_err(|_| JsValue::from_str("external_auth: callback must return a Promise"))
    })
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
        let promise = invoke_external_auth_resolver(&binding.connection_ref).map_err(|e| {
            meerkat_core::AuthError::Other(format!(
                "{} resolver: {}",
                WASM_EXTERNAL_AUTH_RESOLVER_ID,
                js_value_display(&e),
            ))
        })?;
        let js_value = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(|e| {
                meerkat_core::AuthError::Other(format!(
                    "{WASM_EXTERNAL_AUTH_RESOLVER_ID} resolver rejected: {}",
                    js_value_display(&e)
                ))
            })?;
        let token = js_value.as_string().ok_or_else(|| {
            meerkat_core::AuthError::Other(format!(
                "{WASM_EXTERNAL_AUTH_RESOLVER_ID} resolver must resolve its Promise to a string bearer token",
            ))
        })?;
        if token.trim().is_empty() {
            return Err(meerkat_core::AuthError::MissingSecret);
        }
        Ok(meerkat_core::ResolvedAuthEnvelope::InlineSecret {
            secret: token,
            metadata: meerkat_core::AuthMetadata::default(),
            expires_at: None,
        })
    }
}

#[cfg(target_arch = "wasm32")]
fn js_value_display(v: &JsValue) -> String {
    v.as_string()
        .or_else(|| v.dyn_ref::<js_sys::Error>().map(|e| e.message().into()))
        .unwrap_or_else(|| format!("{v:?}"))
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
