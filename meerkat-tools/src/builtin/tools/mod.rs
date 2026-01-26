//! Built-in tool implementations
//!
//! This module provides concrete implementations of the [`BuiltinTool`] trait
//! for task management and other built-in functionality.

mod task_create;
mod task_get;
mod task_list;
mod task_update;

pub use task_create::TaskCreateTool;
pub use task_get::TaskGetTool;
pub use task_list::TaskListTool;
pub use task_update::TaskUpdateTool;

/// Get usage instructions for the LLM on how to properly use task tools
///
/// These instructions should be injected into the system prompt when
/// task tools are enabled.
pub fn task_tools_usage_instructions() -> &'static str {
    r#"# Task Management Tools

You have access to tools for creating and tracking tasks. Use these to organize complex, multi-step work.

## Available Tools
- `task_create` - Create a new task with subject, description, and optional metadata
- `task_list` - List all tasks and their current status
- `task_get` - Get full details of a specific task by ID
- `task_update` - Update a task's status, subject, description, or add blocking relationships

## Task Lifecycle
Tasks have three statuses: `pending` → `in_progress` → `completed`

## Best Practices

### When to Create Tasks
- Multi-step work that benefits from tracking progress
- Work that might be interrupted and resumed later
- Delegating subtasks to sub-agents
- Complex problems that need decomposition

### When NOT to Create Tasks
- Simple, single-step operations
- Tasks you'll complete immediately in the next few tool calls
- Purely informational queries

### Task Organization
- Write clear, actionable subjects (e.g., "Implement user authentication" not "Auth stuff")
- Include acceptance criteria in the description
- Use `blocked_by` to express dependencies between tasks
- Mark tasks `in_progress` when starting work, `completed` when done

### Working with Tasks
- Check `task_list` periodically to see what's pending
- Update task status as you make progress
- Use task metadata to store relevant information (e.g., file paths, test results)
- Don't create tasks for work you've already completed"#
}
