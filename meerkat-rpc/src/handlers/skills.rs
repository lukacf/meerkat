//! Handlers for `skills/list` and `skills/inspect`.
//!
//! Post-V4 the runtime keys skills by `SkillKey` (source_uuid + skill_name).
//! The wire response carries that key directly; source names are display
//! metadata and never the provenance identity.

use std::sync::Arc;

use meerkat_core::skills::{SkillFilter, SkillIntrospectionEntry, SkillRuntime};
use serde::{Deserialize, Deserializer};

use crate::protocol::{RpcId, RpcResponse};

fn skill_source_provenance(
    identity: meerkat_core::skills::SourceIdentityRecord,
) -> meerkat_contracts::SkillSourceProvenance {
    meerkat_contracts::SkillSourceProvenance { identity }
}

fn skill_entry(e: &SkillIntrospectionEntry) -> Result<meerkat_contracts::SkillEntry, String> {
    let source_identity = e
        .source_identity
        .clone()
        .ok_or_else(|| format!("skill {} missing typed source identity", e.descriptor.key))?;
    Ok(meerkat_contracts::SkillEntry {
        key: e.descriptor.key.clone(),
        name: e.descriptor.name.clone(),
        description: e.descriptor.description.clone(),
        scope: e.descriptor.scope.to_string(),
        source: skill_source_provenance(source_identity),
        is_active: e.is_active,
        shadowed_by: e.shadowed_by_identity.clone().map(skill_source_provenance),
    })
}

pub(crate) fn reject_retired_skill_references<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom(
        "skill_references is retired; use structured skill_refs",
    ))
}

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
            let wire: Vec<meerkat_contracts::SkillEntry> =
                match entries.iter().map(skill_entry).collect::<Result<_, _>>() {
                    Ok(wire) => wire,
                    Err(e) => return RpcResponse::error(id, -32603, e),
                };
            let response = meerkat_contracts::SkillListResponse { skills: wire };
            RpcResponse::success(id, &response)
        }
        Err(e) => RpcResponse::error(id, -32603, format!("skill list failed: {e}")),
    }
}
