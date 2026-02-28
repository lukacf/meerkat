//! `mob/*` method handlers.

use serde::Deserialize;
use serde::Serialize;
use serde_json::value::RawValue;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use meerkat_mob_mcp::MobMcpState;
use std::sync::Arc;

#[derive(Debug, Serialize)]
struct MobPrefabEntry {
    key: String,
    toml_template: String,
}

#[derive(Debug, Serialize)]
struct MobPrefabsResult {
    prefabs: Vec<MobPrefabEntry>,
}

/// Handle `mob/prefabs` — list built-in mob prefab templates.
pub async fn handle_prefabs(id: Option<RpcId>) -> RpcResponse {
    let prefabs = meerkat_mob::Prefab::all()
        .into_iter()
        .map(|prefab| MobPrefabEntry {
            key: prefab.key().to_string(),
            toml_template: prefab.toml_template().to_string(),
        })
        .collect();
    RpcResponse::success(id, MobPrefabsResult { prefabs })
}

#[derive(Debug, Serialize)]
struct MobToolsResult {
    tools: Vec<serde_json::Value>,
}

/// Handle `mob/tools` — list callable mob lifecycle tools.
pub async fn handle_tools(id: Option<RpcId>) -> RpcResponse {
    RpcResponse::success(
        id,
        MobToolsResult {
            tools: meerkat_mob_mcp::tools_list(),
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct MobCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// Handle `mob/call` — call a mob lifecycle tool by name with JSON args.
pub async fn handle_call(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobCallParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match meerkat_mob_mcp::handle_tools_call(state, &params.name, &params.arguments).await {
        Ok(value) => RpcResponse::success(id, value),
        Err(err) => RpcResponse::error(id, error::INVALID_PARAMS, err.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handle_prefabs_returns_expected_shape() {
        let resp = handle_prefabs(Some(RpcId::Num(7))).await;
        assert!(resp.error.is_none());
        assert_eq!(resp.id, Some(RpcId::Num(7)));
        let raw = resp.result.expect("result payload");
        let value: serde_json::Value =
            serde_json::from_str(raw.get()).expect("valid response JSON");
        let prefabs = value["prefabs"]
            .as_array()
            .expect("prefabs should be an array");
        assert!(prefabs.iter().all(|entry| entry["key"].is_string()));
        assert!(
            prefabs
                .iter()
                .all(|entry| entry["toml_template"].is_string())
        );
    }
}
