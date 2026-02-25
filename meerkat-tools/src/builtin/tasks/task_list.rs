//! TaskList tool for listing tasks with optional filtering
//!
//! This module provides the [`TaskListTool`] which lists tasks from a
//! [`TaskStore`], optionally filtered by status or labels.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::Value;

use crate::builtin::store::TaskStore;
use crate::builtin::types::TaskStatus;
use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Parameters for the task_list tool
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
struct TaskListParams {
    /// Filter by status: "pending", "in_progress", "completed"
    #[serde(default)]
    status: Option<TaskStatus>,
    /// Filter by labels (any match)
    #[serde(default)]
    labels: Option<Vec<String>>,
}

/// Tool for listing tasks with optional filtering
///
/// # Example
///
/// ```text
/// use std::sync::Arc;
/// use meerkat_tools::builtin::{MemoryTaskStore, TaskStore};
/// use meerkat_tools::builtin::tasks::TaskListTool;
///
/// let store = Arc::new(MemoryTaskStore::new());
/// let tool = TaskListTool::new(store);
///
/// // List all tasks
/// let result = tool.call(serde_json::json!({})).await?;
///
/// // List only pending tasks
/// let result = tool.call(serde_json::json!({"status": "pending"})).await?;
/// ```
pub struct TaskListTool {
    store: Arc<dyn TaskStore>,
}

impl TaskListTool {
    /// Create a new TaskListTool with the given store
    pub fn new(store: Arc<dyn TaskStore>) -> Self {
        Self { store }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for TaskListTool {
    fn name(&self) -> &'static str {
        "task_list"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "task_list".into(),
            description: "List tasks in the project, optionally filtered by status or labels"
                .into(),
            input_schema: crate::schema::schema_for::<TaskListParams>(),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let params: TaskListParams = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;

        let mut tasks = self
            .store
            .list()
            .await
            .map_err(|e| BuiltinToolError::TaskError(e.to_string()))?;

        // Filter by status
        if let Some(status) = &params.status {
            tasks.retain(|t| &t.status == status);
        }

        // Filter by labels
        if let Some(labels) = &params.labels {
            tasks.retain(|t| t.labels.iter().any(|l| labels.contains(l)));
        }

        serde_json::to_value(&tasks).map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::MemoryTaskStore;
    use crate::builtin::types::{NewTask, Task, TaskPriority, TaskStatus};
    use serde_json::json;

    fn schema_resolve_ref<'a>(root: &'a Value, ref_path: &str) -> Option<&'a Value> {
        let path = ref_path.strip_prefix("#/")?;
        let mut current = root;
        for part in path.split('/') {
            current = current.get(part)?;
        }
        Some(current)
    }

    fn schema_allows_value(root: &Value, schema: &Value, expected: &Value) -> bool {
        if let Some(const_value) = schema.get("const") {
            return const_value == expected;
        }

        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            return values.contains(expected);
        }

        if let Some(ref_path) = schema.get("$ref").and_then(Value::as_str)
            && let Some(resolved) = schema_resolve_ref(root, ref_path)
        {
            return schema_allows_value(root, resolved, expected);
        }

        for key in ["anyOf", "oneOf", "allOf"] {
            let Some(options) = schema.get(key).and_then(Value::as_array) else {
                continue;
            };
            if options
                .iter()
                .any(|option| schema_allows_value(root, option, expected))
            {
                return true;
            }
        }

        false
    }

    /// Helper to create a test store with sample tasks
    async fn create_test_store() -> Arc<MemoryTaskStore> {
        let store = Arc::new(MemoryTaskStore::new());

        // Task 1: Pending, labels: ["feature", "frontend"]
        store
            .create(
                NewTask {
                    subject: "Implement login page".to_string(),
                    description: "Create the login page UI".to_string(),
                    priority: Some(TaskPriority::High),
                    labels: Some(vec!["feature".to_string(), "frontend".to_string()]),
                    blocks: None,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        // Task 2: InProgress, labels: ["bug", "backend"]
        let task2 = store
            .create(
                NewTask {
                    subject: "Fix API timeout".to_string(),
                    description: "Investigate API timeout issues".to_string(),
                    priority: Some(TaskPriority::High),
                    labels: Some(vec!["bug".to_string(), "backend".to_string()]),
                    blocks: None,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        // Update task2 to InProgress
        store
            .update(
                &task2.id,
                crate::builtin::types::TaskUpdate {
                    status: Some(TaskStatus::InProgress),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        // Task 3: Completed, labels: ["feature", "backend"]
        let task3 = store
            .create(
                NewTask {
                    subject: "Add user authentication".to_string(),
                    description: "Implement JWT authentication".to_string(),
                    priority: Some(TaskPriority::Medium),
                    labels: Some(vec!["feature".to_string(), "backend".to_string()]),
                    blocks: None,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        // Update task3 to Completed
        store
            .update(
                &task3.id,
                crate::builtin::types::TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        // Task 4: Pending, no labels
        store
            .create(
                NewTask {
                    subject: "Write documentation".to_string(),
                    description: "Document the API endpoints".to_string(),
                    priority: Some(TaskPriority::Low),
                    labels: None,
                    blocks: None,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        store
    }

    #[test]
    fn test_task_list_tool_def() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskListTool::new(store);

        assert_eq!(tool.name(), "task_list");
        assert!(tool.default_enabled());

        let def = tool.def();
        assert_eq!(def.name, "task_list");
        assert!(
            def.description
                .contains("List tasks in the project, optionally filtered")
        );

        // Verify input schema
        let schema = def.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["status"].is_object());
        assert!(schema["properties"]["labels"].is_object());

        // Verify status enum
        assert!(schema_allows_value(
            &schema,
            &schema["properties"]["status"],
            &json!("pending")
        ));
        assert!(schema_allows_value(
            &schema,
            &schema["properties"]["status"],
            &json!("in_progress")
        ));
        assert!(schema_allows_value(
            &schema,
            &schema["properties"]["status"],
            &json!("completed")
        ));
    }

    #[tokio::test]
    async fn test_task_list_empty() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskListTool::new(store);

        let result = tool.call(json!({})).await.unwrap();

        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_task_list_all() {
        let store = create_test_store().await;
        let tool = TaskListTool::new(store);

        let result = tool.call(json!({})).await.unwrap();

        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 4);

        // Verify we got all subjects
        let subjects: Vec<&str> = tasks.iter().map(|t| t.subject.as_str()).collect();
        assert!(subjects.contains(&"Implement login page"));
        assert!(subjects.contains(&"Fix API timeout"));
        assert!(subjects.contains(&"Add user authentication"));
        assert!(subjects.contains(&"Write documentation"));
    }

    #[tokio::test]
    async fn test_task_list_filter_by_status() {
        let store = create_test_store().await;
        let tool = TaskListTool::new(store);

        // Filter by pending
        let result = tool.call(json!({"status": "pending"})).await.unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.status == TaskStatus::Pending));
        let subjects: Vec<&str> = tasks.iter().map(|t| t.subject.as_str()).collect();
        assert!(subjects.contains(&"Implement login page"));
        assert!(subjects.contains(&"Write documentation"));

        // Filter by in_progress
        let result = tool.call(json!({"status": "in_progress"})).await.unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "Fix API timeout");
        assert_eq!(tasks[0].status, TaskStatus::InProgress);

        // Filter by completed
        let result = tool.call(json!({"status": "completed"})).await.unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "Add user authentication");
        assert_eq!(tasks[0].status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn test_task_list_filter_by_labels() {
        let store = create_test_store().await;
        let tool = TaskListTool::new(store);

        // Filter by "feature" label
        let result = tool.call(json!({"labels": ["feature"]})).await.unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 2);
        let subjects: Vec<&str> = tasks.iter().map(|t| t.subject.as_str()).collect();
        assert!(subjects.contains(&"Implement login page"));
        assert!(subjects.contains(&"Add user authentication"));

        // Filter by "backend" label
        let result = tool.call(json!({"labels": ["backend"]})).await.unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 2);
        let subjects: Vec<&str> = tasks.iter().map(|t| t.subject.as_str()).collect();
        assert!(subjects.contains(&"Fix API timeout"));
        assert!(subjects.contains(&"Add user authentication"));

        // Filter by "frontend" label
        let result = tool.call(json!({"labels": ["frontend"]})).await.unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "Implement login page");

        // Filter by multiple labels (any match)
        let result = tool
            .call(json!({"labels": ["bug", "frontend"]}))
            .await
            .unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 2);
        let subjects: Vec<&str> = tasks.iter().map(|t| t.subject.as_str()).collect();
        assert!(subjects.contains(&"Implement login page")); // has "frontend"
        assert!(subjects.contains(&"Fix API timeout")); // has "bug"

        // Filter by non-existent label
        let result = tool.call(json!({"labels": ["nonexistent"]})).await.unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_task_list_filter_by_status_and_labels() {
        let store = create_test_store().await;
        let tool = TaskListTool::new(store);

        // Filter by status=pending AND labels=["feature"]
        let result = tool
            .call(json!({"status": "pending", "labels": ["feature"]}))
            .await
            .unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "Implement login page");

        // Filter by status=completed AND labels=["backend"]
        let result = tool
            .call(json!({"status": "completed", "labels": ["backend"]}))
            .await
            .unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "Add user authentication");

        // Filter with no matching results
        let result = tool
            .call(json!({"status": "completed", "labels": ["frontend"]}))
            .await
            .unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_task_list_invalid_status() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskListTool::new(store);

        let result = tool.call(json!({"status": "invalid_status"})).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::InvalidArgs(msg) => {
                assert!(msg.contains("Invalid status: invalid_status"));
                assert!(msg.contains("Must be pending, in_progress, or completed"));
            }
            _ => unreachable!("Expected InvalidArgs error, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_list_empty_labels_filter() {
        let store = create_test_store().await;
        let tool = TaskListTool::new(store);

        // Empty labels array should return no tasks
        let result = tool.call(json!({"labels": []})).await.unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_task_list_null_params() {
        let store = create_test_store().await;
        let tool = TaskListTool::new(store);

        // Explicit null values should be treated as "no filter"
        let result = tool
            .call(json!({"status": null, "labels": null}))
            .await
            .unwrap();
        let tasks: Vec<Task> = serde_json::from_value(result).unwrap();
        assert_eq!(tasks.len(), 4);
    }
}
