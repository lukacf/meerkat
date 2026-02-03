//! Utility tool set implementation

use super::datetime::DateTimeTool;
use super::wait::{WaitInterrupt, WaitTool};
use crate::builtin::BuiltinTool;
use tokio::sync::watch;

/// A set of utility tools for general-purpose operations
///
/// Utility tools are enabled by default when built-in tools are enabled.
#[derive(Debug)]
pub struct UtilityToolSet {
    /// Tool for pausing execution
    pub wait: WaitTool,
    /// Tool for getting current date/time
    pub datetime: DateTimeTool,
}

impl UtilityToolSet {
    /// Create a new UtilityToolSet without interrupt support
    pub fn new() -> Self {
        Self {
            wait: WaitTool::new(),
            datetime: DateTimeTool::new(),
        }
    }

    /// Create a UtilityToolSet with interrupt support for the wait tool
    ///
    /// The wait tool will be interrupted when a message is sent on the channel.
    pub fn with_interrupt(interrupt_rx: watch::Receiver<Option<WaitInterrupt>>) -> Self {
        Self {
            wait: WaitTool::with_interrupt(interrupt_rx),
            datetime: DateTimeTool::new(),
        }
    }

    /// Get references to all tools as a vector
    pub fn tools(&self) -> Vec<&dyn BuiltinTool> {
        vec![
            &self.wait as &dyn BuiltinTool,
            &self.datetime as &dyn BuiltinTool,
        ]
    }

    /// Get tool names in this set
    pub fn tool_names() -> Vec<&'static str> {
        vec!["wait", "datetime"]
    }

    /// Get usage instructions for the LLM on how to use utility tools
    ///
    /// These instructions should be injected into the system prompt when
    /// utility tools are enabled.
    pub fn usage_instructions() -> &'static str {
        r#"# Utility Tools

You have access to utility tools for timing and coordination.

## Available Tools
- `wait` - Pause execution for a specified number of seconds (max 300)
- `datetime` - Get the current date and time

## Using the Wait Tool

The `wait` tool is essential for async operations. Use it to:
- **Wait between status checks**: After spawning a sub-agent, call `wait(15)` before checking status
- **Rate limiting**: When making repeated API calls, wait between them
- **Coordination**: When timing matters, use wait to synchronize operations

Example pattern for sub-agents:
1. `agent_spawn` - spawn the sub-agent
2. Do other useful work (create tasks, spawn more agents, etc.)
3. `wait(20)` - wait 20 seconds
4. `agent_status` - check if done
5. If not done: `wait(15)` then check again

**DO NOT** poll status in a tight loop without waiting. Always use `wait` between status checks.

## Using the DateTime Tool

Use `datetime` when you need to:
- Know the current time for scheduling or logging
- Calculate durations or deadlines
- Include timestamps in outputs or task metadata"#
    }
}

impl Default for UtilityToolSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_utility_tool_set_creation() {
        let tool_set = UtilityToolSet::new();
        assert_eq!(tool_set.wait.name(), "wait");
        assert_eq!(tool_set.datetime.name(), "datetime");
    }

    #[test]
    fn test_utility_tool_set_tools() {
        let tool_set = UtilityToolSet::new();
        let tools = tool_set.tools();

        assert_eq!(tools.len(), 2);

        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"wait"));
        assert!(names.contains(&"datetime"));
    }

    #[test]
    fn test_utility_tool_set_tool_names() {
        let names = UtilityToolSet::tool_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"wait"));
        assert!(names.contains(&"datetime"));
    }

    #[test]
    fn test_utility_tool_set_all_enabled_by_default() {
        let tool_set = UtilityToolSet::new();
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
        assert!(instructions.contains("wait"));
        assert!(instructions.contains("datetime"));
        assert!(instructions.contains("DO NOT"));
    }

    #[test]
    fn test_utility_tool_set_default() {
        let tool_set = UtilityToolSet::default();
        assert_eq!(tool_set.tools().len(), 2);
    }
}
