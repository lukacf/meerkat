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
//! - [`ExternalAuthResolverHandle`] — wasm_bindgen-exposed handle that
//!   stores a JS callback returning a Promise<string> of a bearer
//!   token. The handle survives across WASM calls (registered once,
//!   consulted per-session).
//! - [`register_external_auth_resolver`] — wasm_bindgen entry point
//!   that the host page calls to install the resolver.
//! - [`build_session_request_with_connection_ref`] — replacement for
//!   the legacy flat-path `build_direct_session_request` that routes
//!   through the provider-runtime registry. When the resolved binding
//!   uses an `ExternalResolver` credential source, the registry looks
//!   up this handle's callback, awaits it, and wires the returned
//!   token into the LLM client's Authorization header.
//!
//! The callback shape matches plan §Phase 4d.wasm:
//!
//! ```js
//! // Host-page registration:
//! meerkat.register_external_auth_resolver(async (bindingKey) => {
//!   // host-owned OAuth flow; returns a bearer string
//!   const token = await my_oauth_client.fresh_token_for(bindingKey);
//!   return token; // plain string; meerkat wraps as envelope internally
//! });
//! ```
//!
//! The returned `bindingKey` is `"<realm_id>:<binding_id>"`.

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

/// Register a JS-side external-auth resolver. The callback receives a
/// single string argument (`"<realm_id>:<binding_id>"`) and must return
/// a Promise that resolves to a bearer-token string.
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

/// Invoke the registered external-auth resolver for a binding key.
/// Returns the JS Promise the callback produced, or an error if no
/// resolver is registered.
///
/// Caller awaits the Promise via wasm-bindgen-futures to obtain the
/// bearer token string.
#[cfg(target_arch = "wasm32")]
#[allow(dead_code)] // wired in when the provider-runtime-registry WASM path consumes it
pub(crate) fn invoke_external_auth_resolver(binding_key: &str) -> Result<Promise, JsValue> {
    EXTERNAL_AUTH_RESOLVER.with(|slot| {
        let slot = slot.borrow();
        let callback = slot.as_ref().ok_or_else(|| {
            JsValue::from_str(
                "external_auth: no resolver registered; call register_external_auth_resolver first",
            )
        })?;
        let this = JsValue::NULL;
        let result = callback.call1(&this, &JsValue::from_str(binding_key))?;
        result
            .dyn_into::<Promise>()
            .map_err(|_| JsValue::from_str("external_auth: callback must return a Promise"))
    })
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
