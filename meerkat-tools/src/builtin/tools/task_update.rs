//! TaskUpdate tool for updating existing tasks
//!
//! This tool allows agents to update tasks in the task store.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::Value;

use crate::builtin::store::TaskStore;
use crate::builtin::types::{TaskId, TaskPriority, TaskStatus, TaskUpdate};
use crate::builtin::{BuiltinTool, BuiltinToolError};

/// Parameters for the task_update tool
#[derive(Debug, Deserialize)]
struct TaskUpdateParams {
    /// The ID of the task to update
    id: String,
    /// New subject/title (optional)
    #[serde(default)]
    subject: Option<String>,
    /// New description (optional)
    #[serde(default)]
    description: Option<String>,
    /// New status: "pending", "in_progress", "completed" (optional)
    #[serde(default)]
    status: Option<String>,
    /// New priority: "low", "medium", "high" (optional)
    #[serde(default)]
    priority: Option<String>,
    /// Replace all labels (optional)
    #[serde(default)]
    labels: Option<Vec<String>>,
    /// Task IDs to add to blocks list (optional)
    #[serde(default)]
    add_blocks: Option<Vec<String>>,
    /// Task IDs to remove from blocks list (optional)
    #[serde(default)]
    remove_blocks: Option<Vec<String>>,
}

/// Tool for updating existing tasks
pub struct TaskUpdateTool {
    store: Arc<dyn TaskStore>,
    session_id: Option<String>,
}

impl TaskUpdateTool {
    /// Create a new TaskUpdateTool with the given store
    pub fn new(store: Arc<dyn TaskStore>) -> Self {
        Self {
            store,
            session_id: None,
        }
    }

    /// Create a new TaskUpdateTool with a session ID for tracking updates
    pub fn with_session(store: Arc<dyn TaskStore>, session_id: String) -> Self {
        Self {
            store,
            session_id: Some(session_id),
        }
    }

    /// Create a new TaskUpdateTool with an optional session ID for tracking updates
    ///
    /// This is a convenience method that accepts `Option<String>`.
    pub fn with_session_opt(store: Arc<dyn TaskStore>, session_id: Option<String>) -> Self {
        Self { store, session_id }
    }
}

#[async_trait]
impl BuiltinTool for TaskUpdateTool {
    fn name(&self) -> &'static str {
        "task_update"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "task_update".to_string(),
            description: "Update an existing task".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The task ID to update"
                    },
                    "subject": {
                        "type": "string",
                        "description": "New subject/title"
                    },
                    "description": {
                        "type": "string",
                        "description": "New description"
                    },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed"],
                        "description": "New status"
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["low", "medium", "high"],
                        "description": "New priority"
                    },
                    "labels": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Replace all labels"
                    },
                    "add_blocks": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Task IDs to add to blocks list"
                    },
                    "remove_blocks": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Task IDs to remove from blocks list"
                    }
                },
                "required": ["id"]
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let params: TaskUpdateParams = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;

        let status = match params.status.as_deref() {
            Some("pending") => Some(TaskStatus::Pending),
            Some("in_progress") => Some(TaskStatus::InProgress),
            Some("completed") => Some(TaskStatus::Completed),
            Some(other) => {
                return Err(BuiltinToolError::InvalidArgs(format!(
                    "Invalid status: {}",
                    other
                )));
            }
            None => None,
        };

        let priority = match params.priority.as_deref() {
            Some("low") => Some(TaskPriority::Low),
            Some("medium") => Some(TaskPriority::Medium),
            Some("high") => Some(TaskPriority::High),
            Some(other) => {
                return Err(BuiltinToolError::InvalidArgs(format!(
                    "Invalid priority: {}",
                    other
                )));
            }
            None => None,
        };

        let update = TaskUpdate {
            subject: params.subject,
            description: params.description,
            status,
            priority,
            labels: params.labels,
            add_blocks: params
                .add_blocks
                .map(|ids| ids.into_iter().map(TaskId).collect()),
            remove_blocks: params
                .remove_blocks
                .map(|ids| ids.into_iter().map(TaskId).collect()),
        };

        let task = self
            .store
            .update(&TaskId(params.id), update, self.session_id.as_deref())
            .await
            .map_err(|e| BuiltinToolError::TaskError(e.to_string()))?;

        serde_json::to_value(&task).map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::memory_store::MemoryTaskStore;
    use crate::builtin::types::{NewTask, Task, TaskPriority, TaskStatus};
    use serde_json::json;

    /// Helper to create a store with a test task
    async fn create_store_with_task() -> (Arc<MemoryTaskStore>, Task) {
        let store = Arc::new(MemoryTaskStore::new());
        let task = store
            .create(
                NewTask {
                    subject: "Test task".to_string(),
                    description: "Test description".to_string(),
                    priority: Some(TaskPriority::Medium),
                    labels: Some(vec!["initial".to_string()]),
                    blocks: None,
                },
                None,
            )
            .await
            .unwrap();
        (store, task)
    }

    #[test]
    fn test_task_update_tool_def() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskUpdateTool::new(store);

        assert_eq!(tool.name(), "task_update");

        let def = tool.def();
        assert_eq!(def.name, "task_update");
        assert_eq!(def.description, "Update an existing task");

        // Check schema has required "id" field
        let schema = def.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["id"].is_object());
        assert!(schema["properties"]["subject"].is_object());
        assert!(schema["properties"]["status"].is_object());
        assert!(schema["properties"]["priority"].is_object());
        assert!(schema["properties"]["labels"].is_object());
        assert!(schema["properties"]["add_blocks"].is_object());
        assert!(schema["properties"]["remove_blocks"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "id");

        // Check status enum
        let status_enum = schema["properties"]["status"]["enum"].as_array().unwrap();
        assert!(status_enum.contains(&json!("pending")));
        assert!(status_enum.contains(&json!("in_progress")));
        assert!(status_enum.contains(&json!("completed")));

        // Check priority enum
        let priority_enum = schema["properties"]["priority"]["enum"].as_array().unwrap();
        assert!(priority_enum.contains(&json!("low")));
        assert!(priority_enum.contains(&json!("medium")));
        assert!(priority_enum.contains(&json!("high")));
    }

    #[test]
    fn test_task_update_tool_default_enabled() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskUpdateTool::new(store);
        assert!(tool.default_enabled());
    }

    #[tokio::test]
    async fn test_task_update_status() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Update status to in_progress
        let result = tool
            .call(json!({
                "id": task.id.0,
                "status": "in_progress"
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.id, task.id);
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.subject, "Test task"); // unchanged

        // Verify in store
        let stored = store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(stored.status, TaskStatus::InProgress);

        // Update to completed
        let result = tool
            .call(json!({
                "id": task.id.0,
                "status": "completed"
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn test_task_update_priority() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Update priority to high
        let result = tool
            .call(json!({
                "id": task.id.0,
                "priority": "high"
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.priority, TaskPriority::High);

        // Update to low
        let result = tool
            .call(json!({
                "id": task.id.0,
                "priority": "low"
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.priority, TaskPriority::Low);
    }

    #[tokio::test]
    async fn test_task_update_subject_and_description() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        let result = tool
            .call(json!({
                "id": task.id.0,
                "subject": "Updated subject",
                "description": "Updated description"
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.subject, "Updated subject");
        assert_eq!(updated.description, "Updated description");
        assert_eq!(updated.status, TaskStatus::Pending); // unchanged
        assert_eq!(updated.priority, TaskPriority::Medium); // unchanged
    }

    #[tokio::test]
    async fn test_task_update_labels() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        let result = tool
            .call(json!({
                "id": task.id.0,
                "labels": ["new-label", "another-label"]
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(
            updated.labels,
            vec!["new-label".to_string(), "another-label".to_string()]
        );
    }

    #[tokio::test]
    async fn test_task_update_add_blocks() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Create another task to use as a blocker
        let blocker = store
            .create(
                NewTask {
                    subject: "Blocker task".to_string(),
                    description: "".to_string(),
                    priority: None,
                    labels: None,
                    blocks: None,
                },
                None,
            )
            .await
            .unwrap();

        let result = tool
            .call(json!({
                "id": task.id.0,
                "add_blocks": [blocker.id.0]
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.blocks.len(), 1);
        assert_eq!(updated.blocks[0], blocker.id);

        // Add another block
        let blocker2 = store
            .create(
                NewTask {
                    subject: "Another blocker".to_string(),
                    description: "".to_string(),
                    priority: None,
                    labels: None,
                    blocks: None,
                },
                None,
            )
            .await
            .unwrap();

        let result = tool
            .call(json!({
                "id": task.id.0,
                "add_blocks": [blocker2.id.0]
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.blocks.len(), 2);
        assert!(updated.blocks.contains(&blocker.id));
        assert!(updated.blocks.contains(&blocker2.id));
    }

    #[tokio::test]
    async fn test_task_update_remove_blocks() {
        let store = Arc::new(MemoryTaskStore::new());
        let blocker1 = TaskId::from_string("blocker-1");
        let blocker2 = TaskId::from_string("blocker-2");

        let task = store
            .create(
                NewTask {
                    subject: "Task with blocks".to_string(),
                    description: "".to_string(),
                    priority: None,
                    labels: None,
                    blocks: Some(vec![blocker1.clone(), blocker2.clone()]),
                },
                None,
            )
            .await
            .unwrap();

        let tool = TaskUpdateTool::new(store.clone());

        // Remove one block
        let result = tool
            .call(json!({
                "id": task.id.0,
                "remove_blocks": ["blocker-1"]
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.blocks.len(), 1);
        assert!(!updated.blocks.contains(&blocker1));
        assert!(updated.blocks.contains(&blocker2));
    }

    #[tokio::test]
    async fn test_task_update_not_found() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskUpdateTool::new(store);

        let result = tool
            .call(json!({
                "id": "nonexistent-task-id",
                "status": "completed"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::TaskError(msg) => {
                assert!(msg.contains("not found") || msg.contains("NotFound"));
            }
            _ => panic!("Expected TaskError, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_update_invalid_status() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store);

        let result = tool
            .call(json!({
                "id": task.id.0,
                "status": "invalid_status"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::InvalidArgs(msg) => {
                assert!(msg.contains("Invalid status"));
                assert!(msg.contains("invalid_status"));
            }
            _ => panic!("Expected InvalidArgs, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_update_invalid_priority() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store);

        let result = tool
            .call(json!({
                "id": task.id.0,
                "priority": "urgent"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::InvalidArgs(msg) => {
                assert!(msg.contains("Invalid priority"));
                assert!(msg.contains("urgent"));
            }
            _ => panic!("Expected InvalidArgs, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_update_with_session() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::with_session(store.clone(), "test-session".to_string());

        let result = tool
            .call(json!({
                "id": task.id.0,
                "subject": "Updated with session"
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.updated_by_session, Some("test-session".to_string()));
    }

    #[tokio::test]
    async fn test_task_update_missing_id() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskUpdateTool::new(store);

        let result = tool
            .call(json!({
                "subject": "No ID provided"
            }))
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            BuiltinToolError::InvalidArgs(_) => {}
            other => panic!("Expected InvalidArgs, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_task_update_multiple_fields() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        let result = tool
            .call(json!({
                "id": task.id.0,
                "subject": "Completely updated",
                "description": "New description",
                "status": "completed",
                "priority": "high",
                "labels": ["done", "reviewed"]
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.subject, "Completely updated");
        assert_eq!(updated.description, "New description");
        assert_eq!(updated.status, TaskStatus::Completed);
        assert_eq!(updated.priority, TaskPriority::High);
        assert_eq!(
            updated.labels,
            vec!["done".to_string(), "reviewed".to_string()]
        );
    }
}
