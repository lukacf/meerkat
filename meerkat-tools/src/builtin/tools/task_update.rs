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
use crate::schema::SchemaBuilder;

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
    /// New owner/assignee (optional)
    #[serde(default)]
    owner: Option<String>,
    /// Metadata key-value pairs to merge (null value = delete key)
    #[serde(default)]
    metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// Task IDs to add to blocked_by list (optional)
    #[serde(default)]
    add_blocked_by: Option<Vec<String>>,
    /// Task IDs to remove from blocked_by list (optional)
    #[serde(default)]
    remove_blocked_by: Option<Vec<String>>,
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
            input_schema: SchemaBuilder::new()
                .property(
                    "id",
                    serde_json::json!({
                        "type": "string",
                        "description": "The task ID to update"
                    }),
                )
                .property(
                    "subject",
                    serde_json::json!({
                        "type": "string",
                        "description": "New subject/title"
                    }),
                )
                .property(
                    "description",
                    serde_json::json!({
                        "type": "string",
                        "description": "New description"
                    }),
                )
                .property(
                    "status",
                    serde_json::json!({
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed"],
                        "description": "New status"
                    }),
                )
                .property(
                    "priority",
                    serde_json::json!({
                        "type": "string",
                        "enum": ["low", "medium", "high"],
                        "description": "New priority"
                    }),
                )
                .property(
                    "labels",
                    serde_json::json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Replace all labels"
                    }),
                )
                .property(
                    "add_blocks",
                    serde_json::json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Task IDs to add to blocks list"
                    }),
                )
                .property(
                    "remove_blocks",
                    serde_json::json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Task IDs to remove from blocks list"
                    }),
                )
                .property(
                    "owner",
                    serde_json::json!({
                        "type": "string",
                        "description": "New owner/assignee"
                    }),
                )
                .property(
                    "metadata",
                    serde_json::json!({
                        "type": "object",
                        "additionalProperties": true,
                        "description": "Metadata key-value pairs to merge (null value deletes key)"
                    }),
                )
                .property(
                    "add_blocked_by",
                    serde_json::json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Task IDs to add to blocked_by list"
                    }),
                )
                .property(
                    "remove_blocked_by",
                    serde_json::json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Task IDs to remove from blocked_by list"
                    }),
                )
                .required("id")
                .build(),
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
            owner: params.owner,
            metadata: params.metadata,
            add_blocked_by: params
                .add_blocked_by
                .map(|ids| ids.into_iter().map(TaskId).collect()),
            remove_blocked_by: params
                .remove_blocked_by
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
                    ..Default::default()
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
                    ..Default::default()
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
                    ..Default::default()
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
                    ..Default::default()
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

    // ======================================================================
    // Tests for TaskUpdateTool new parameters: owner, metadata, add_blocked_by, remove_blocked_by
    // These tests verify the tool accepts and processes the new parameters
    // ======================================================================

    #[test]
    fn test_task_update_tool_def_has_owner_param() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskUpdateTool::new(store);
        let def = tool.def();

        // Schema should include owner parameter
        let props = def.input_schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("owner"),
            "Schema should have owner property"
        );

        let owner_schema = &props["owner"];
        assert_eq!(owner_schema["type"], "string");
        assert!(
            owner_schema["description"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("owner"),
            "Owner description should mention owner"
        );
    }

    #[test]
    fn test_task_update_tool_def_has_metadata_param() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskUpdateTool::new(store);
        let def = tool.def();

        // Schema should include metadata parameter
        let props = def.input_schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("metadata"),
            "Schema should have metadata property"
        );

        let metadata_schema = &props["metadata"];
        assert_eq!(metadata_schema["type"], "object");
    }

    #[test]
    fn test_task_update_tool_def_has_add_blocked_by_param() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskUpdateTool::new(store);
        let def = tool.def();

        // Schema should include add_blocked_by parameter
        let props = def.input_schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("add_blocked_by"),
            "Schema should have add_blocked_by property"
        );

        let add_blocked_by_schema = &props["add_blocked_by"];
        assert_eq!(add_blocked_by_schema["type"], "array");
        assert_eq!(add_blocked_by_schema["items"]["type"], "string");
    }

    #[test]
    fn test_task_update_tool_def_has_remove_blocked_by_param() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskUpdateTool::new(store);
        let def = tool.def();

        // Schema should include remove_blocked_by parameter
        let props = def.input_schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("remove_blocked_by"),
            "Schema should have remove_blocked_by property"
        );

        let remove_blocked_by_schema = &props["remove_blocked_by"];
        assert_eq!(remove_blocked_by_schema["type"], "array");
        assert_eq!(remove_blocked_by_schema["items"]["type"], "string");
    }

    #[tokio::test]
    async fn test_task_update_tool_set_owner() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Set owner via tool
        let result = tool
            .call(json!({
                "id": task.id.0,
                "owner": "alice"
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.owner, Some("alice".to_string()));
        assert_eq!(updated.subject, "Test task"); // unchanged

        // Verify in store
        let stored = store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(stored.owner, Some("alice".to_string()));
    }

    #[tokio::test]
    async fn test_task_update_tool_set_metadata() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Set metadata via tool
        let result = tool
            .call(json!({
                "id": task.id.0,
                "metadata": {
                    "category": "feature",
                    "estimated_hours": 8,
                    "tags": ["frontend", "urgent"]
                }
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.metadata.get("category"), Some(&json!("feature")));
        assert_eq!(updated.metadata.get("estimated_hours"), Some(&json!(8)));
        assert_eq!(
            updated.metadata.get("tags"),
            Some(&json!(["frontend", "urgent"]))
        );
    }

    #[tokio::test]
    async fn test_task_update_tool_metadata_merge() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Set initial metadata
        tool.call(json!({
            "id": task.id.0,
            "metadata": {
                "key1": "value1",
                "key2": "value2"
            }
        }))
        .await
        .unwrap();

        // Merge more metadata (update key2, add key3)
        let result = tool
            .call(json!({
                "id": task.id.0,
                "metadata": {
                    "key2": "updated",
                    "key3": "new"
                }
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.metadata.get("key1"), Some(&json!("value1"))); // unchanged
        assert_eq!(updated.metadata.get("key2"), Some(&json!("updated"))); // updated
        assert_eq!(updated.metadata.get("key3"), Some(&json!("new"))); // added
    }

    #[tokio::test]
    async fn test_task_update_tool_metadata_delete_with_null() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Set initial metadata
        tool.call(json!({
            "id": task.id.0,
            "metadata": {
                "keep": "keep me",
                "delete_me": "will be deleted"
            }
        }))
        .await
        .unwrap();

        // Delete key by setting to null
        let result = tool
            .call(json!({
                "id": task.id.0,
                "metadata": {
                    "delete_me": null
                }
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.metadata.get("keep"), Some(&json!("keep me")));
        assert!(!updated.metadata.contains_key("delete_me"));
    }

    #[tokio::test]
    async fn test_task_update_tool_add_blocked_by() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Create a blocker task
        let blocker = store
            .create(
                NewTask {
                    subject: "Blocker task".to_string(),
                    description: "".to_string(),
                    priority: None,
                    labels: None,
                    blocks: None,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        // Add blocked_by via tool
        let result = tool
            .call(json!({
                "id": task.id.0,
                "add_blocked_by": [blocker.id.0]
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.blocked_by.len(), 1);
        assert_eq!(updated.blocked_by[0], blocker.id);
    }

    #[tokio::test]
    async fn test_task_update_tool_remove_blocked_by() {
        let store = Arc::new(MemoryTaskStore::new());

        // Create task and add blocked_by
        let blocker1 = TaskId::from_string("blocker-1");
        let blocker2 = TaskId::from_string("blocker-2");

        let task = store
            .create(
                NewTask {
                    subject: "Task with blockers".to_string(),
                    description: "".to_string(),
                    priority: None,
                    labels: None,
                    blocks: None,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        let tool = TaskUpdateTool::new(store.clone());

        // First add blocked_by
        tool.call(json!({
            "id": task.id.0,
            "add_blocked_by": ["blocker-1", "blocker-2"]
        }))
        .await
        .unwrap();

        // Remove one via tool
        let result = tool
            .call(json!({
                "id": task.id.0,
                "remove_blocked_by": ["blocker-1"]
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.blocked_by.len(), 1);
        assert!(!updated.blocked_by.contains(&blocker1));
        assert!(updated.blocked_by.contains(&blocker2));
    }

    #[tokio::test]
    async fn test_task_update_tool_all_new_params_together() {
        let (store, task) = create_store_with_task().await;
        let tool = TaskUpdateTool::new(store.clone());

        // Use all new parameters in one call
        let result = tool
            .call(json!({
                "id": task.id.0,
                "owner": "team-lead",
                "metadata": {
                    "sprint": 42,
                    "points": 5
                },
                "add_blocked_by": ["prerequisite-task-1", "prerequisite-task-2"]
            }))
            .await
            .unwrap();

        let updated: Task = serde_json::from_value(result).unwrap();
        assert_eq!(updated.owner, Some("team-lead".to_string()));
        assert_eq!(updated.metadata.get("sprint"), Some(&json!(42)));
        assert_eq!(updated.metadata.get("points"), Some(&json!(5)));
        assert_eq!(updated.blocked_by.len(), 2);
    }
}
