//! Handlers for `skills/list` and `skills/inspect`.
//!
//! Post-V4 the runtime keys skills by `SkillKey` (source_uuid + skill_name).
//! This handler projects the typed key onto the existing wire `SkillEntry`
//! shape by rendering `"{source_uuid}/{skill_name}"` — the wire contract
//! itself is owned by `meerkat-contracts` and a separate task retypes it
//! to carry `source_uuid` and `skill_name` as distinct fields.

use std::sync::Arc;

use meerkat_core::skills::{SkillFilter, SkillRuntime};

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
                    id: format!(
                        "{}/{}",
                        e.descriptor.key.source_uuid, e.descriptor.key.skill_name
                    ),
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
