//! TaskGet tool implementation
//!
//! Retrieves a task by its unique identifier.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::Value;

use crate::builtin::store::TaskStore;
use crate::builtin::types::TaskId;
use crate::builtin::{BuiltinTool, BuiltinToolError};
use crate::schema::SchemaBuilder;

/// Parameters for the task_get tool
#[derive(Debug, Deserialize)]
struct TaskGetParams {
    /// The task ID to retrieve
    id: String,
}

/// Tool for retrieving a task by ID
///
/// This tool looks up a task in the store by its unique identifier
/// and returns the full task details if found.
pub struct TaskGetTool {
    store: Arc<dyn TaskStore>,
}

impl TaskGetTool {
    /// Create a new TaskGetTool with the given store
    pub fn new(store: Arc<dyn TaskStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl BuiltinTool for TaskGetTool {
    fn name(&self) -> &'static str {
        "task_get"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "task_get".to_string(),
            description: "Get a task by its ID".to_string(),
            input_schema: SchemaBuilder::new()
                .property(
                    "id",
                    serde_json::json!({
                        "type": "string",
                        "description": "The task ID"
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
        let params: TaskGetParams = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;

        let task = self
            .store
            .get(&TaskId(params.id.clone()))
            .await
            .map_err(|e| BuiltinToolError::TaskError(e.to_string()))?;

        match task {
            Some(t) => serde_json::to_value(&t)
                .map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string())),
            None => Err(BuiltinToolError::ExecutionFailed(format!(
                "Task not found: {}",
                params.id
            ))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::memory_store::MemoryTaskStore;
    use crate::builtin::types::{Task, TaskPriority, TaskStatus};
    use serde_json::json;

    fn create_test_store() -> Arc<MemoryTaskStore> {
        Arc::new(MemoryTaskStore::new())
    }

    fn create_test_store_with_task() -> (Arc<MemoryTaskStore>, Task) {
        let task = Task {
            id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            subject: "Test task".to_string(),
            description: "A test task description".to_string(),
            status: TaskStatus::InProgress,
            priority: TaskPriority::High,
            labels: vec!["test".to_string(), "important".to_string()],
            blocks: vec![TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAW")],
            owner: Some("alice".to_string()),
            metadata: std::collections::HashMap::new(),
            blocked_by: vec![],
            created_at: "2025-01-23T10:00:00Z".to_string(),
            updated_at: "2025-01-23T11:00:00Z".to_string(),
            created_by_session: Some("session-123".to_string()),
            updated_by_session: Some("session-456".to_string()),
        };

        let store = Arc::new(MemoryTaskStore::with_tasks(vec![task.clone()]));
        (store, task)
    }

    #[test]
    fn test_task_get_tool_def() {
        let store = create_test_store();
        let tool = TaskGetTool::new(store);

        // Check name
        assert_eq!(tool.name(), "task_get");

        // Check definition
        let def = tool.def();
        assert_eq!(def.name, "task_get");
        assert_eq!(def.description, "Get a task by its ID");

        // Check input schema
        let schema = def.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["id"].is_object());
        assert_eq!(schema["properties"]["id"]["type"], "string");
        assert_eq!(schema["properties"]["id"]["description"], "The task ID");
        assert_eq!(schema["required"], json!(["id"]));

        // Check default enabled
        assert!(tool.default_enabled());
    }

    #[tokio::test]
    async fn test_task_get_existing() {
        let (store, expected_task) = create_test_store_with_task();
        let tool = TaskGetTool::new(store);

        let args = json!({
            "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV"
        });

        let result = tool.call(args).await.unwrap();

        // Verify the returned task matches the expected task
        assert_eq!(result["id"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(result["subject"], expected_task.subject);
        assert_eq!(result["description"], expected_task.description);
        assert_eq!(result["status"], "in_progress");
        assert_eq!(result["priority"], "high");
        assert_eq!(result["labels"], json!(["test", "important"]));
        assert_eq!(result["blocks"], json!(["01ARZ3NDEKTSV4RRFFQ69G5FAW"]));
        assert_eq!(result["created_at"], expected_task.created_at);
        assert_eq!(result["updated_at"], expected_task.updated_at);
        assert_eq!(result["created_by_session"], "session-123");
        assert_eq!(result["updated_by_session"], "session-456");
    }

    #[tokio::test]
    async fn test_task_get_not_found() {
        let store = create_test_store();
        let tool = TaskGetTool::new(store);

        let args = json!({
            "id": "nonexistent-task-id"
        });

        let result = tool.call(args).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::ExecutionFailed(msg) => {
                assert!(msg.contains("Task not found"));
                assert!(msg.contains("nonexistent-task-id"));
            }
            _ => unreachable!("Expected ExecutionFailed error, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_get_missing_id() {
        let store = create_test_store();
        let tool = TaskGetTool::new(store);

        // Empty object - missing required 'id' field
        let args = json!({});

        let result = tool.call(args).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::InvalidArgs(msg) => {
                assert!(msg.contains("id") || msg.contains("missing"));
            }
            _ => unreachable!("Expected InvalidArgs error, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_get_wrong_type_id() {
        let store = create_test_store();
        let tool = TaskGetTool::new(store);

        // Wrong type for 'id' - number instead of string
        let args = json!({
            "id": 12345
        });

        let result = tool.call(args).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::InvalidArgs(msg) => {
                assert!(msg.contains("string") || msg.contains("invalid type"));
            }
            _ => unreachable!("Expected InvalidArgs error, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_get_null_id() {
        let store = create_test_store();
        let tool = TaskGetTool::new(store);

        // Null value for 'id'
        let args = json!({
            "id": null
        });

        let result = tool.call(args).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::InvalidArgs(_) => {}
            _ => unreachable!("Expected InvalidArgs error, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_get_extra_fields_ignored() {
        let (store, expected_task) = create_test_store_with_task();
        let tool = TaskGetTool::new(store);

        // Extra fields should be ignored
        let args = json!({
            "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "extra_field": "should be ignored",
            "another_extra": 123
        });

        let result = tool.call(args).await.unwrap();

        // Should still work and return the task
        assert_eq!(result["id"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(result["subject"], expected_task.subject);
    }

    #[tokio::test]
    async fn test_task_get_empty_string_id() {
        let store = create_test_store();
        let tool = TaskGetTool::new(store);

        // Empty string ID
        let args = json!({
            "id": ""
        });

        let result = tool.call(args).await;

        // Empty ID is technically valid but won't find a task
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            BuiltinToolError::ExecutionFailed(msg) => {
                assert!(msg.contains("Task not found"));
            }
            _ => unreachable!("Expected ExecutionFailed error, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_get_returns_complete_task_structure() {
        let (store, _) = create_test_store_with_task();
        let tool = TaskGetTool::new(store);

        let args = json!({
            "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV"
        });

        let result = tool.call(args).await.unwrap();

        // Verify all expected fields are present in the result
        assert!(result.get("id").is_some());
        assert!(result.get("subject").is_some());
        assert!(result.get("description").is_some());
        assert!(result.get("status").is_some());
        assert!(result.get("priority").is_some());
        assert!(result.get("labels").is_some());
        assert!(result.get("blocks").is_some());
        assert!(result.get("created_at").is_some());
        assert!(result.get("updated_at").is_some());
        assert!(result.get("created_by_session").is_some());
        assert!(result.get("updated_by_session").is_some());
    }
}
