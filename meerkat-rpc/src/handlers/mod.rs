//! RPC method handlers.
//!
//! Each sub-module handles a group of JSON-RPC methods.

pub mod capabilities;
pub mod config;
pub mod initialize;
pub mod session;
pub mod turn;

use serde_json::value::RawValue;

use crate::error;
use crate::protocol::{RpcId, RpcResponse};

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
