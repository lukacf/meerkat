//! Skill discovery and activation tools.
//!
//! Provides `browse_skills` and `load_skill` builtin tools for skill
//! discovery and per-turn activation.

pub mod browse;
pub mod load;

pub use browse::BrowseSkillsTool;
pub use load::LoadSkillTool;

use std::sync::Arc;

use meerkat_core::skills::SkillEngine;

use crate::builtin::BuiltinTool;

/// Bundles skill tools for registration in the dispatcher.
pub struct SkillToolSet {
    pub browse: BrowseSkillsTool,
    pub load: LoadSkillTool,
}

impl SkillToolSet {
    pub fn new(engine: Arc<dyn SkillEngine>) -> Self {
        Self {
            browse: BrowseSkillsTool::new(Arc::clone(&engine)),
            load: LoadSkillTool::new(engine),
        }
    }

    pub fn tools(&self) -> Vec<&dyn BuiltinTool> {
        vec![&self.browse as &dyn BuiltinTool, &self.load as &dyn BuiltinTool]
    }
}
