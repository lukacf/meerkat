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

use meerkat_core::skills::SkillRuntime;

use crate::builtin::BuiltinTool;

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
