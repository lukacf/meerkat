//! RPC method handlers.
//!
//! Each sub-module handles a group of JSON-RPC methods.

pub mod approval;
pub mod auth;
pub mod capabilities;
#[cfg(feature = "comms")]
pub mod comms;
pub mod config;
// Runtime-backed external-event convenience handler.
pub mod event;
pub mod help;
pub mod initialize;
pub mod jobs;
pub mod live;
pub mod mcp;
#[cfg(feature = "mob")]
pub mod mob;
pub mod models;
pub mod runtime;
pub mod runtime_host;
pub mod schedule;
pub mod session;
pub mod skills;
pub mod turn;
pub mod workgraph;

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
    if let Some(locator_realm) = locator.realm_id.as_ref() {
        let runtime_realm = runtime.realm_id();
        if runtime_realm.as_ref() != Some(locator_realm) {
            return Err(RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!(
                    "Session locator realm '{}' does not match active runtime realm '{}'",
                    locator_realm.as_str(),
                    runtime_realm
                        .as_ref()
                        .map(|r| r.as_str())
                        .unwrap_or("<none>")
                ),
            ));
        }
    }
    Ok(locator.session_id)
}

/// K17 ratchet: RPC handler responses are constructed ONLY from
/// `meerkat-contracts` wire types — no hand-shaped `serde_json::json!`
/// payloads in production handler code. Test modules may use `json!` to
/// build fixtures/assertions.
#[cfg(test)]
#[allow(clippy::expect_used)]
mod no_hand_shaped_handler_payloads {
    #[test]
    fn handlers_production_code_has_no_json_macro() {
        let handlers_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/handlers");
        let mut violations = Vec::new();
        for entry in std::fs::read_dir(&handlers_dir).expect("handlers dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("handler source");
            // Scan only the production region: everything before the first
            // test-module marker.
            let production_end = text
                .find("#[cfg(test)]")
                .or_else(|| text.find("mod tests"))
                .unwrap_or(text.len());
            for (idx, line) in text[..production_end].lines().enumerate() {
                if line.contains("json!(") {
                    violations.push(format!("{}:{}: {}", path.display(), idx + 1, line.trim()));
                }
            }
        }
        assert!(
            violations.is_empty(),
            "hand-shaped json! payloads in production RPC handler code (K17):\n{}",
            violations.join("\n")
        );
    }
}
