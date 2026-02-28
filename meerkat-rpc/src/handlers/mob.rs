//! `mob/*` method handlers.

use serde::Serialize;

use crate::protocol::{RpcId, RpcResponse};

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
