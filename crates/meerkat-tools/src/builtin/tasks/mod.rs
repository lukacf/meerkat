//! Task management tools

pub mod task_create;
pub mod task_get;
pub mod task_list;
pub mod task_update;

pub use task_create::TaskCreateTool;
pub use task_get::TaskGetTool;
pub use task_list::TaskListTool;
pub use task_update::TaskUpdateTool;

/// The names of the task tools, in registration order.
pub fn tool_names() -> &'static [&'static str] {
    &["task_list", "task_get", "task_create", "task_update"]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::builtin::{BuiltinTool, MemoryTaskStore, TaskStore};

    #[test]
    fn tool_names_match_the_tool_definitions() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let tools: Vec<Box<dyn BuiltinTool>> = vec![
            Box::new(TaskListTool::new(Arc::clone(&store))),
            Box::new(TaskGetTool::new(Arc::clone(&store))),
            Box::new(TaskCreateTool::new(Arc::clone(&store))),
            Box::new(TaskUpdateTool::new(store)),
        ];
        let defined: Vec<String> = tools
            .iter()
            .map(|tool| tool.def().name.to_string())
            .collect();
        assert_eq!(defined, tool_names());
    }
}
