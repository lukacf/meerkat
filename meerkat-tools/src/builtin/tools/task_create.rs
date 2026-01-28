//! TaskCreate built-in tool
//!
//! This module provides the [`TaskCreateTool`] which allows agents to create
//! new tasks in the task management system.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::Value;

use crate::builtin::store::TaskStore;
use crate::builtin::types::{NewTask, TaskId, TaskPriority};
use crate::builtin::{BuiltinTool, BuiltinToolError};
use crate::schema::SchemaBuilder;

/// Parameters for the task_create tool
#[derive(Debug, Deserialize)]
struct TaskCreateParams {
    /// Brief title for the task
    subject: String,
    /// Detailed description of what needs to be done
    description: String,
    /// Priority level: "low", "medium", or "high"
    #[serde(default)]
    priority: Option<String>,
    /// Labels/tags for categorization
    #[serde(default)]
    labels: Option<Vec<String>>,
    /// IDs of tasks that this task blocks
    #[serde(default)]
    blocks: Option<Vec<String>>,
    /// Owner/assignee of the task
    #[serde(default)]
    owner: Option<String>,
    /// Arbitrary key-value metadata
    #[serde(default)]
    metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// IDs of tasks that block THIS task
    #[serde(default)]
    blocked_by: Option<Vec<String>>,
}

/// Built-in tool for creating tasks
///
/// This tool allows agents to create new tasks in the task management system.
/// It validates input parameters and delegates to the configured [`TaskStore`].
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use meerkat_tools::builtin::{MemoryTaskStore, TaskCreateTool, BuiltinTool};
/// use serde_json::json;
///
/// let store = Arc::new(MemoryTaskStore::new());
/// let tool = TaskCreateTool::new(store);
///
/// let result = tool.call(json!({
///     "subject": "Implement feature",
///     "description": "Add the new authentication feature"
/// })).await.unwrap();
/// ```
pub struct TaskCreateTool {
    store: Arc<dyn TaskStore>,
    session_id: Option<String>,
}

impl TaskCreateTool {
    /// Create a new TaskCreateTool with the given store
    pub fn new(store: Arc<dyn TaskStore>) -> Self {
        Self {
            store,
            session_id: None,
        }
    }

    /// Create a new TaskCreateTool with the given store and session ID
    ///
    /// The session ID is used to track which session created tasks.
    pub fn with_session(store: Arc<dyn TaskStore>, session_id: String) -> Self {
        Self {
            store,
            session_id: Some(session_id),
        }
    }

    /// Create a new TaskCreateTool with the given store and optional session ID
    ///
    /// The session ID is used to track which session created tasks.
    /// This is a convenience method that accepts `Option<String>`.
    pub fn with_session_opt(store: Arc<dyn TaskStore>, session_id: Option<String>) -> Self {
        Self { store, session_id }
    }
}

#[async_trait]
impl BuiltinTool for TaskCreateTool {
    fn name(&self) -> &'static str {
        "task_create"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "task_create".to_string(),
            description: "Create a new task in the project task list".to_string(),
            input_schema: SchemaBuilder::new()
                .property(
                    "subject",
                    serde_json::json!({
                        "type": "string",
                        "description": "Brief title for the task"
                    }),
                )
                .property(
                    "description",
                    serde_json::json!({
                        "type": "string",
                        "description": "Detailed description of what needs to be done"
                    }),
                )
                .property(
                    "priority",
                    serde_json::json!({
                        "type": "string",
                        "enum": ["low", "medium", "high"],
                        "description": "Task priority level"
                    }),
                )
                .property(
                    "labels",
                    serde_json::json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Labels/tags for categorization"
                    }),
                )
                .property(
                    "blocks",
                    serde_json::json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "IDs of tasks that this task blocks"
                    }),
                )
                .property(
                    "owner",
                    serde_json::json!({
                        "type": "string",
                        "description": "Owner/assignee of the task"
                    }),
                )
                .property(
                    "metadata",
                    serde_json::json!({
                        "type": "object",
                        "additionalProperties": true,
                        "description": "Arbitrary key-value metadata"
                    }),
                )
                .property(
                    "blocked_by",
                    serde_json::json!({
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "IDs of tasks that block THIS task"
                    }),
                )
                .required("subject")
                .required("description")
                .build(),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let params: TaskCreateParams = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::InvalidArgs(e.to_string()))?;

        let priority = match params.priority.as_deref() {
            Some("low") => Some(TaskPriority::Low),
            Some("medium") => Some(TaskPriority::Medium),
            Some("high") => Some(TaskPriority::High),
            Some(other) => {
                return Err(BuiltinToolError::InvalidArgs(format!(
                    "Invalid priority: {}. Must be low, medium, or high",
                    other
                )));
            }
            None => None,
        };

        let new_task = NewTask {
            subject: params.subject,
            description: params.description,
            priority,
            labels: params.labels,
            blocks: params
                .blocks
                .map(|ids| ids.into_iter().map(TaskId).collect()),
            owner: params.owner,
            metadata: params.metadata,
            blocked_by: params
                .blocked_by
                .map(|ids| ids.into_iter().map(TaskId).collect()),
        };

        let task = self
            .store
            .create(new_task, self.session_id.as_deref())
            .await
            .map_err(|e| BuiltinToolError::TaskError(e.to_string()))?;

        serde_json::to_value(&task).map_err(|e| BuiltinToolError::ExecutionFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::MemoryTaskStore;
    use crate::builtin::types::{Task, TaskPriority, TaskStatus};

    #[test]
    fn test_task_create_tool_def() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store);

        // Verify tool name
        assert_eq!(tool.name(), "task_create");

        // Verify tool definition
        let def = tool.def();
        assert_eq!(def.name, "task_create");
        assert_eq!(
            def.description,
            "Create a new task in the project task list"
        );

        // Verify schema has required fields
        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");

        let properties = &schema["properties"];
        assert!(properties["subject"].is_object());
        assert!(properties["description"].is_object());
        assert!(properties["priority"].is_object());
        assert!(properties["labels"].is_object());
        assert!(properties["blocks"].is_object());

        // Verify required fields
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("subject")));
        assert!(required.contains(&serde_json::json!("description")));

        // Verify priority enum values
        let priority_enum = properties["priority"]["enum"].as_array().unwrap();
        assert!(priority_enum.contains(&serde_json::json!("low")));
        assert!(priority_enum.contains(&serde_json::json!("medium")));
        assert!(priority_enum.contains(&serde_json::json!("high")));
    }

    #[test]
    fn test_task_create_default_enabled() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store);

        assert!(tool.default_enabled());
    }

    #[tokio::test]
    async fn test_task_create_basic() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store.clone());

        let args = serde_json::json!({
            "subject": "Implement feature X",
            "description": "Add the new feature X to the system"
        });

        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();

        assert_eq!(task.subject, "Implement feature X");
        assert_eq!(task.description, "Add the new feature X to the system");
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.priority, TaskPriority::Medium); // default
        assert!(task.labels.is_empty());
        assert!(task.blocks.is_empty());
        assert!(!task.id.0.is_empty());
        assert!(!task.created_at.is_empty());
        assert!(!task.updated_at.is_empty());
        assert!(task.created_by_session.is_none());

        // Verify task is stored
        assert_eq!(store.len(), 1);
    }

    #[tokio::test]
    async fn test_task_create_with_session() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::with_session(store.clone(), "test-session-123".to_string());

        let args = serde_json::json!({
            "subject": "Task with session",
            "description": "This task was created by a session"
        });

        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();

        assert_eq!(
            task.created_by_session,
            Some("test-session-123".to_string())
        );
        assert_eq!(
            task.updated_by_session,
            Some("test-session-123".to_string())
        );
    }

    #[tokio::test]
    async fn test_task_create_with_priority() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store.clone());

        // Test low priority
        let args = serde_json::json!({
            "subject": "Low priority task",
            "description": "Not urgent",
            "priority": "low"
        });
        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();
        assert_eq!(task.priority, TaskPriority::Low);

        // Test medium priority
        let args = serde_json::json!({
            "subject": "Medium priority task",
            "description": "Normal urgency",
            "priority": "medium"
        });
        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();
        assert_eq!(task.priority, TaskPriority::Medium);

        // Test high priority
        let args = serde_json::json!({
            "subject": "High priority task",
            "description": "Very urgent",
            "priority": "high"
        });
        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();
        assert_eq!(task.priority, TaskPriority::High);
    }

    #[tokio::test]
    async fn test_task_create_with_labels() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store.clone());

        let args = serde_json::json!({
            "subject": "Labeled task",
            "description": "This task has labels",
            "labels": ["bug", "urgent", "backend"]
        });

        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();

        assert_eq!(task.labels.len(), 3);
        assert!(task.labels.contains(&"bug".to_string()));
        assert!(task.labels.contains(&"urgent".to_string()));
        assert!(task.labels.contains(&"backend".to_string()));
    }

    #[tokio::test]
    async fn test_task_create_with_blocks() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store.clone());

        let args = serde_json::json!({
            "subject": "Blocking task",
            "description": "This task blocks other tasks",
            "blocks": ["01ARZ3NDEKTSV4RRFFQ69G5FAV", "01ARZ3NDEKTSV4RRFFQ69G5FAW"]
        });

        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();

        assert_eq!(task.blocks.len(), 2);
        assert_eq!(task.blocks[0].0, "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(task.blocks[1].0, "01ARZ3NDEKTSV4RRFFQ69G5FAW");
    }

    #[tokio::test]
    async fn test_task_create_with_all_fields() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::with_session(store.clone(), "session-all".to_string());

        let args = serde_json::json!({
            "subject": "Complete task",
            "description": "This task has all fields populated",
            "priority": "high",
            "labels": ["feature", "v2"],
            "blocks": ["01ARZ3NDEKTSV4RRFFQ69G5FAV"]
        });

        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();

        assert_eq!(task.subject, "Complete task");
        assert_eq!(task.description, "This task has all fields populated");
        assert_eq!(task.priority, TaskPriority::High);
        assert_eq!(task.labels, vec!["feature", "v2"]);
        assert_eq!(task.blocks.len(), 1);
        assert_eq!(task.blocks[0].0, "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(task.created_by_session, Some("session-all".to_string()));
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_task_create_invalid_priority() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store);

        let args = serde_json::json!({
            "subject": "Task with invalid priority",
            "description": "Should fail",
            "priority": "critical"  // Invalid priority
        });

        let result = tool.call(args).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        match err {
            BuiltinToolError::InvalidArgs(msg) => {
                assert!(msg.contains("Invalid priority"));
                assert!(msg.contains("critical"));
                assert!(msg.contains("low, medium, or high"));
            }
            _ => panic!("Expected InvalidArgs error, got {:?}", err),
        }
    }

    #[tokio::test]
    async fn test_task_create_missing_required_field() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store);

        // Missing subject
        let args = serde_json::json!({
            "description": "Task without subject"
        });

        let result = tool.call(args).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, BuiltinToolError::InvalidArgs(_)));

        // Missing description
        let args = serde_json::json!({
            "subject": "Task without description"
        });

        let result = tool.call(args).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, BuiltinToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn test_task_create_empty_args() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store);

        let args = serde_json::json!({});

        let result = tool.call(args).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, BuiltinToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn test_task_create_empty_labels() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store.clone());

        let args = serde_json::json!({
            "subject": "Task with empty labels",
            "description": "Labels array is empty",
            "labels": []
        });

        let result = tool.call(args).await.unwrap();
        let task: Task = serde_json::from_value(result).unwrap();

        assert!(task.labels.is_empty());
    }

    #[tokio::test]
    async fn test_task_create_multiple_tasks() {
        let store = Arc::new(MemoryTaskStore::new());
        let tool = TaskCreateTool::new(store.clone());

        // Create multiple tasks
        for i in 1..=5 {
            let args = serde_json::json!({
                "subject": format!("Task {}", i),
                "description": format!("Description for task {}", i)
            });
            tool.call(args).await.unwrap();
        }

        // Verify all tasks were created
        assert_eq!(store.len(), 5);

        let tasks = store.list().await.unwrap();
        let subjects: Vec<_> = tasks.iter().map(|t| t.subject.as_str()).collect();
        assert!(subjects.contains(&"Task 1"));
        assert!(subjects.contains(&"Task 5"));
    }
}
