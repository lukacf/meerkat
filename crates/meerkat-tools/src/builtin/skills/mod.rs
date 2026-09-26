//! Skill discovery and activation tools.
//!
//! Provides skill discovery/activation/resource/function builtin tools for skill
//! discovery and per-turn activation.

pub mod browse;
pub mod functions;
pub mod load;
pub mod resources;

pub use browse::BrowseSkillsTool;
pub use functions::SkillInvokeFunctionTool;
pub use load::LoadSkillTool;
pub use resources::{SkillListResourcesTool, SkillReadResourceTool};

use std::sync::Arc;

use meerkat_core::ToolDef;
use meerkat_core::skills::SkillRuntime;

use crate::builtin::BuiltinTool;

/// Canonical tool name of [`BrowseSkillsTool`].
pub const BROWSE_SKILLS_TOOL_NAME: &str = "browse_skills";

/// Canonical tool name of [`LoadSkillTool`].
pub const LOAD_SKILL_TOOL_NAME: &str = "load_skill";

/// System-prompt guidance tying the `<available_skills>` inventory to the
/// skill discovery tools.
///
/// Both tools are `default_enabled: false`, so the inventory itself never
/// names them. A host appends this guidance only when
/// [`skill_discovery_tools_composed`] reports that both tools are part of the
/// session's composed tool set.
pub const SKILL_DISCOVERY_TOOL_GUIDANCE: &str = "Use the browse_skills tool to list skills in a source or search, and the load_skill tool to activate a skill by its source_uuid and skill_name.";

/// Whether both skill discovery tools are present in a composed tool set.
pub fn skill_discovery_tools_composed(tools: &[Arc<ToolDef>]) -> bool {
    let composed = |name: &str| tools.iter().any(|tool| tool.name == name);
    composed(BROWSE_SKILLS_TOOL_NAME) && composed(LOAD_SKILL_TOOL_NAME)
}

/// Bundles skill tools for registration in the dispatcher.
pub struct SkillToolSet {
    pub browse: BrowseSkillsTool,
    pub load: LoadSkillTool,
    pub list_resources: SkillListResourcesTool,
    pub read_resource: SkillReadResourceTool,
    pub invoke_function: SkillInvokeFunctionTool,
}

impl SkillToolSet {
    pub fn new(engine: Arc<SkillRuntime>) -> Self {
        Self {
            browse: BrowseSkillsTool::new(Arc::clone(&engine)),
            load: LoadSkillTool::new(Arc::clone(&engine)),
            list_resources: SkillListResourcesTool::new(Arc::clone(&engine)),
            read_resource: SkillReadResourceTool::new(Arc::clone(&engine)),
            invoke_function: SkillInvokeFunctionTool::new(engine),
        }
    }

    pub fn tools(&self) -> Vec<&dyn BuiltinTool> {
        vec![
            &self.browse as &dyn BuiltinTool,
            &self.load as &dyn BuiltinTool,
            &self.list_resources as &dyn BuiltinTool,
            &self.read_resource as &dyn BuiltinTool,
            &self.invoke_function as &dyn BuiltinTool,
        ]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn named(names: &[&str]) -> Vec<Arc<ToolDef>> {
        names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).into(),
                    description: String::new(),
                    input_schema: serde_json::json!({"type": "object"}),
                    provenance: None,
                })
            })
            .collect()
    }

    #[test]
    fn discovery_guidance_requires_both_discovery_tools() {
        assert!(skill_discovery_tools_composed(&named(&[
            "shell",
            BROWSE_SKILLS_TOOL_NAME,
            LOAD_SKILL_TOOL_NAME,
        ])));
        assert!(!skill_discovery_tools_composed(&named(&[
            BROWSE_SKILLS_TOOL_NAME
        ])));
        assert!(!skill_discovery_tools_composed(&named(&[
            LOAD_SKILL_TOOL_NAME
        ])));
        assert!(!skill_discovery_tools_composed(&named(&["shell"])));
    }
}
