//! Utility tool set implementation

#[cfg(not(target_arch = "wasm32"))]
use super::ApplyPatchTool;
use super::datetime::DateTimeTool;
use crate::builtin::BuiltinTool;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

/// A set of utility tools for general-purpose operations
///
/// Utility tools are enabled by default when built-in tools are enabled.
#[derive(Debug)]
pub struct UtilityToolSet {
    /// Tool for getting current date/time
    pub datetime: DateTimeTool,
    /// Tool for applying structured file edits
    #[cfg(not(target_arch = "wasm32"))]
    pub apply_patch: ApplyPatchTool,
}

impl UtilityToolSet {
    /// Create a new UtilityToolSet without interrupt support
    #[cfg(target_arch = "wasm32")]
    pub fn new() -> Self {
        Self {
            datetime: DateTimeTool::new(),
        }
    }

    /// Create a new UtilityToolSet without interrupt support.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            datetime: DateTimeTool::new(),
            apply_patch: ApplyPatchTool::new(project_root),
        }
    }

    /// Get references to all tools as a vector
    #[cfg(target_arch = "wasm32")]
    pub fn tools(&self) -> Vec<&dyn BuiltinTool> {
        vec![&self.datetime as &dyn BuiltinTool]
    }

    /// Get references to all tools as a vector.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tools(&self) -> Vec<&dyn BuiltinTool> {
        vec![
            &self.datetime as &dyn BuiltinTool,
            &self.apply_patch as &dyn BuiltinTool,
        ]
    }

    /// Get tool names in this set
    #[cfg(target_arch = "wasm32")]
    pub fn tool_names() -> Vec<&'static str> {
        vec!["datetime"]
    }

    /// Get tool names in this set.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tool_names() -> Vec<&'static str> {
        vec!["datetime", "apply_patch"]
    }

    /// Get usage instructions for the LLM on how to use utility tools
    ///
    /// These instructions should be injected into the system prompt when
    /// utility tools are enabled.
    pub fn usage_instructions() -> &'static str {
        r"# Utility Tools

You have access to utility tools for time lookup and structured file edits.

## Available Tools
- `datetime` - Get the current date and time
- `apply_patch` - Apply structured file edits inside the project root

## Using the DateTime Tool

Use `datetime` when you need to:
- Know the current time for scheduling or logging
- Calculate durations or deadlines
- Include timestamps in outputs or task metadata"
    }
}

impl Default for UtilityToolSet {
    #[cfg(target_arch = "wasm32")]
    fn default() -> Self {
        Self::new()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_utility_tool_set_creation() {
        #[cfg(target_arch = "wasm32")]
        let tool_set = UtilityToolSet::new();
        #[cfg(not(target_arch = "wasm32"))]
        let tool_set = UtilityToolSet::new(PathBuf::from("/tmp/test"));
        assert_eq!(tool_set.datetime.name(), "datetime");
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(tool_set.apply_patch.name(), "apply_patch");
    }

    #[test]
    fn test_utility_tool_set_tools() {
        #[cfg(target_arch = "wasm32")]
        let tool_set = UtilityToolSet::new();
        #[cfg(not(target_arch = "wasm32"))]
        let tool_set = UtilityToolSet::new(PathBuf::from("/tmp/test"));
        let tools = tool_set.tools();

        #[cfg(target_arch = "wasm32")]
        assert_eq!(tools.len(), 1);
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(tools.len(), 2);

        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"datetime"));
        #[cfg(not(target_arch = "wasm32"))]
        assert!(names.contains(&"apply_patch"));
    }

    #[test]
    fn test_utility_tool_set_tool_names() {
        let names = UtilityToolSet::tool_names();
        #[cfg(target_arch = "wasm32")]
        assert_eq!(names.len(), 1);
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"datetime"));
        #[cfg(not(target_arch = "wasm32"))]
        assert!(names.contains(&"apply_patch"));
    }

    #[test]
    fn test_utility_tool_set_all_enabled_by_default() {
        #[cfg(target_arch = "wasm32")]
        let tool_set = UtilityToolSet::new();
        #[cfg(not(target_arch = "wasm32"))]
        let tool_set = UtilityToolSet::new(PathBuf::from("/tmp/test"));
        let tools = tool_set.tools();

        for tool in tools {
            assert!(
                tool.default_enabled(),
                "Tool {} should be enabled by default",
                tool.name()
            );
        }
    }

    #[test]
    fn test_utility_tool_set_usage_instructions() {
        let instructions = UtilityToolSet::usage_instructions();
        assert!(instructions.contains("datetime"));
        assert!(!instructions.contains("wait"));
    }

    #[test]
    fn test_utility_tool_set_default() {
        let tool_set = UtilityToolSet::default();
        #[cfg(target_arch = "wasm32")]
        assert_eq!(tool_set.tools().len(), 1);
        #[cfg(not(target_arch = "wasm32"))]
        assert_eq!(tool_set.tools().len(), 2);
    }
}
