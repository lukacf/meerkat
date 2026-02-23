//! Handlers for `skills/list` and `skills/inspect`.

use std::sync::Arc;

use meerkat_core::skills::{SkillFilter, SkillId, SkillRuntime};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::protocol::{RpcId, RpcResponse};

/// Handle `skills/list` — list all skills with provenance information.
pub async fn handle_list(
    id: Option<RpcId>,
    skill_runtime: &Option<Arc<SkillRuntime>>,
) -> RpcResponse {
    let runtime = match skill_runtime {
        Some(rt) => rt,
        None => {
            return RpcResponse::error(id, -32603, "skills not enabled");
        }
    };

    match runtime
        .list_all_with_provenance(&SkillFilter::default())
        .await
    {
        Ok(entries) => {
            let wire: Vec<meerkat_contracts::SkillEntry> = entries
                .iter()
                .map(|e| meerkat_contracts::SkillEntry {
                    id: e.descriptor.id.0.clone(),
                    name: e.descriptor.name.clone(),
                    description: e.descriptor.description.clone(),
                    scope: e.descriptor.scope.to_string(),
                    source: e.descriptor.source_name.clone(),
                    is_active: e.is_active,
                    shadowed_by: e.shadowed_by.clone(),
                })
                .collect();
            let response = meerkat_contracts::SkillListResponse { skills: wire };
            RpcResponse::success(id, &response)
        }
        Err(e) => RpcResponse::error(id, -32603, format!("skill list failed: {e}")),
    }
}

#[derive(Deserialize)]
struct InspectParams {
    id: String,
    #[serde(default)]
    source: Option<String>,
}

/// Handle `skills/inspect` — inspect a skill's full content.
pub async fn handle_inspect(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    skill_runtime: &Option<Arc<SkillRuntime>>,
) -> RpcResponse {
    let runtime = match skill_runtime {
        Some(rt) => rt,
        None => {
            return RpcResponse::error(id, -32603, "skills not enabled");
        }
    };

    let input: InspectParams = match params {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(p) => p,
            Err(e) => {
                return RpcResponse::error(id, -32602, format!("invalid params: {e}"));
            }
        },
        None => {
            return RpcResponse::error(id, -32602, "missing params");
        }
    };

    match runtime
        .load_from_source(&SkillId::from(input.id.as_str()), input.source.as_deref())
        .await
    {
        Ok(doc) => {
            let response = meerkat_contracts::SkillInspectResponse {
                id: doc.descriptor.id.0.clone(),
                name: doc.descriptor.name.clone(),
                description: doc.descriptor.description.clone(),
                scope: doc.descriptor.scope.to_string(),
                source: doc.descriptor.source_name.clone(),
                body: doc.body,
            };
            RpcResponse::success(id, &response)
        }
        Err(e) => RpcResponse::error(id, -32603, format!("skill inspect failed: {e}")),
    }
}
