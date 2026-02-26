//! RPC method handlers.
//!
//! Each sub-module handles a group of JSON-RPC methods.

pub mod capabilities;
#[cfg(feature = "comms")]
pub mod comms;
pub mod config;
// BRIDGE(M7→M12): Legacy event/push handler, kept for internal reference.
// Remove when M12 eradicates all legacy surfaces.
#[cfg(feature = "comms")]
pub mod event;
pub mod initialize;
pub mod session;
pub mod skills;
pub mod turn;

use serde_json::value::RawValue;

use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;
use meerkat_core::SessionId;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Parse typed params from a `RawValue`, returning an `RpcResponse` error on failure.
#[allow(clippy::result_large_err)]
pub(crate) fn parse_params<T: serde::de::DeserializeOwned>(
    params: Option<&RawValue>,
) -> Result<T, RpcResponse> {
    let raw =
        params.ok_or_else(|| RpcResponse::error(None, error::INVALID_PARAMS, "Missing params"))?;
    serde_json::from_str(raw.get()).map_err(|e| {
        RpcResponse::error(None, error::INVALID_PARAMS, format!("Invalid params: {e}"))
    })
}

/// Extension trait to set the id on an RpcResponse (used when `parse_params`
/// returns an error before we have the id available in the response).
pub(crate) trait RpcResponseExt {
    fn with_id(self, id: Option<RpcId>) -> Self;
}

impl RpcResponseExt for RpcResponse {
    fn with_id(mut self, id: Option<RpcId>) -> Self {
        self.id = id;
        self
    }
}

/// Re-export `WireUsage` from contracts as the canonical usage type.
pub use meerkat_contracts::WireUsage as UsageResult;

/// Parse `<session_id>` or `<realm_id>:<session_id>` and enforce realm match.
#[allow(clippy::result_large_err)]
pub(crate) fn parse_session_id_for_runtime(
    id: Option<RpcId>,
    raw: &str,
    runtime: &SessionRuntime,
) -> Result<SessionId, RpcResponse> {
    let locator = match meerkat_contracts::SessionLocator::parse(raw) {
        Ok(locator) => locator,
        Err(err) => {
            return Err(RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Invalid session_id '{raw}': {err}"),
            ));
        }
    };
    if let Some(realm) = locator.realm_id.as_deref()
        && runtime.realm_id() != Some(realm)
    {
        return Err(RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "Session locator realm '{}' does not match active runtime realm '{}'",
                realm,
                runtime.realm_id().unwrap_or("<none>")
            ),
        ));
    }
    Ok(locator.session_id)
}
