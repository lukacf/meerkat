//! Skill resource tools.
//!
//! Keyed by typed `SkillKey` (source_uuid + skill_name) — no slash-string
//! parsing anywhere on the ingress path.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::skills::{
    SkillError, SkillFilter, SkillKey, SkillName, SkillRuntime, SourceUuid,
};
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SkillListResourcesArgs {
    source_uuid: String,
    skill_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SkillReadResourceArgs {
    source_uuid: String,
    skill_name: String,
    path: String,
}

fn parse_key(source_raw: &str, skill_raw: &str) -> Result<SkillKey, BuiltinToolError> {
    let source_uuid =
        SourceUuid::parse(source_raw).map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;
    let skill_name =
        SkillName::parse(skill_raw).map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;
    Ok(SkillKey {
        source_uuid,
        skill_name,
    })
}

/// Whether the skill listing shows `key` to this caller.
///
/// A resource tool makes this typed read after a source answered
/// `SkillError::NotFound`: the `SkillSource` defaults answer that for every
/// key, so the variant alone does not say that the skill is missing.
///
/// The read is the one `browse_skills` renders, with the filter that tool
/// builds for the key's source: the engine's listing with provenance, of
/// which only the active entries count. It therefore sees what the listing
/// shows this caller and no more. A skill the listing hides (one whose
/// required capability is unavailable, or a copy that is not the active one)
/// is not listed here either, and a listing that fails counts as not listed:
/// a fact that could not be read stays unclassified, and the caller keeps the
/// answer it had.
async fn skill_is_listed(engine: &SkillRuntime, key: &SkillKey) -> bool {
    let filter = SkillFilter {
        source_uuid: Some(key.source_uuid.clone()),
        ..SkillFilter::default()
    };
    engine
        .list_all_with_provenance(&filter)
        .await
        .is_ok_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.is_active && &entry.descriptor.key == key)
        })
}

/// What `skill_list_resources` answers for a listed skill when no source
/// returned a resource listing for it. It is an error, so it cannot be read
/// as an empty listing. It states the two things that were read and one
/// conditional next step. It says nothing about sources or files, about what
/// the next step returns, or about whether that tool is composed for the
/// caller: this tool holds no such fact.
fn no_resource_listing_returned(key: &SkillKey) -> String {
    format!(
        "The skill {key} is listed, but no source returned a resource listing for it. \
         This is not an empty listing. If {} is available, call it with this key to use \
         the skill.",
        super::LOAD_SKILL_TOOL_NAME
    )
}

pub struct SkillListResourcesTool {
    engine: Arc<SkillRuntime>,
}

impl SkillListResourcesTool {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
        Self { engine }
    }
}

pub struct SkillReadResourceTool {
    engine: Arc<SkillRuntime>,
}

impl SkillReadResourceTool {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
        Self { engine }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for SkillListResourcesTool {
    fn name(&self) -> &'static str {
        super::SKILL_LIST_RESOURCES_TOOL_NAME
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            audience: Default::default(),
            name: super::SKILL_LIST_RESOURCES_TOOL_NAME.into(),
            description:
                "List resources exposed by a skill identified by (source_uuid, skill_name).".into(),
            input_schema: crate::schema::schema_for::<SkillListResourcesArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "skills".into(),
                artifact: None,
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    /// Skill-resource listing read.
    fn mutation_class(&self) -> meerkat_core::ToolMutationClass {
        meerkat_core::ToolMutationClass::ReadOnly
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: SkillListResourcesArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::InvalidArgs(err.to_string()))?;
        let raw_key = parse_key(&args.source_uuid, &args.skill_name)?;
        // Apply source-identity lineage remaps before dispatch.
        let key = self
            .engine
            .canonical_skill_key(&raw_key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let artifacts = match self.engine.list_artifacts(&key).await {
            // A successful listing, empty or not, is the source's answer.
            Ok(artifacts) => artifacts,
            // "Not found" is also the `SkillSource` default's answer for
            // every key. Read whether the skill listing shows this key
            // before saying the skill is not found; a key it does not show
            // keeps the answer as it is.
            Err(error @ SkillError::NotFound { .. }) => {
                let message = if skill_is_listed(&self.engine, &key).await {
                    no_resource_listing_returned(&key)
                } else {
                    error.to_string()
                };
                return Err(BuiltinToolError::ExecutionFailed(message));
            }
            Err(error) => return Err(BuiltinToolError::ExecutionFailed(error.to_string())),
        };
        Ok(ToolOutput::Json(json!({
            "source_uuid": key.source_uuid.to_string(),
            "skill_name": key.skill_name.as_str(),
            "artifacts": artifacts,
        })))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for SkillReadResourceTool {
    fn name(&self) -> &'static str {
        super::SKILL_READ_RESOURCE_TOOL_NAME
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            audience: Default::default(),
            name: super::SKILL_READ_RESOURCE_TOOL_NAME.into(),
            description:
                "Read a resource at `path` from a skill identified by (source_uuid, skill_name)."
                    .into(),
            input_schema: crate::schema::schema_for::<SkillReadResourceArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "skills".into(),
                artifact: None,
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    /// Skill-resource read.
    fn mutation_class(&self) -> meerkat_core::ToolMutationClass {
        meerkat_core::ToolMutationClass::ReadOnly
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: SkillReadResourceArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::InvalidArgs(err.to_string()))?;
        let raw_key = parse_key(&args.source_uuid, &args.skill_name)?;
        // Apply source-identity lineage remaps before dispatch.
        let key = self
            .engine
            .canonical_skill_key(&raw_key)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        let artifact = self
            .engine
            .read_artifact(&key, &args.path)
            .await
            .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))?;
        Ok(ToolOutput::Json(json!({
            "source_uuid": key.source_uuid.to_string(),
            "skill_name": key.skill_name.as_str(),
            "artifact": artifact,
        })))
    }
}
