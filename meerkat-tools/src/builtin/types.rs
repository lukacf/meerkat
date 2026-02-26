//! Task types for the built-in task management tools
//!
//! This module defines the core types for task management including
//! [`Task`], [`TaskId`], [`TaskStatus`], and [`TaskPriority`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Task ID - UUID v7 string for unique task identification
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    /// Create a new TaskId with a generated UUID v7
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }

    /// Create a TaskId from an existing string
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Status of a task in its lifecycle
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")] // for Serialize
pub enum TaskStatus {
    /// Task is waiting to be started
    #[default]
    Pending,
    /// Task is actively being worked on
    InProgress,
    /// Task has been finished
    Completed,
}

impl<'de> Deserialize<'de> for TaskStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            other => Err(serde::de::Error::custom(format!(
                "Invalid status: {other}. Must be pending, in_progress, or completed"
            ))),
        }
    }
}

/// Priority level for a task
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")] // for Serialize
pub enum TaskPriority {
    /// Low priority task
    Low,
    /// Medium priority task (default)
    #[default]
    Medium,
    /// High priority task
    High,
}

impl<'de> Deserialize<'de> for TaskPriority {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            other => Err(serde::de::Error::custom(format!(
                "Invalid priority: {other}. Must be low, medium, or high"
            ))),
        }
    }
}

/// A task in the task management system
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    /// Unique identifier for this task
    pub id: TaskId,
    /// Short subject/title of the task
    pub subject: String,
    /// Detailed description of the task
    pub description: String,
    /// Current status of the task
    pub status: TaskStatus,
    /// Priority level of the task
    pub priority: TaskPriority,
    /// Labels/tags for categorization
    pub labels: Vec<String>,
    /// IDs of tasks that this task blocks
    pub blocks: Vec<TaskId>,
    /// ISO 8601 timestamp when the task was created
    pub created_at: String,
    /// ISO 8601 timestamp when the task was last updated
    pub updated_at: String,
    /// Session ID that created this task
    pub created_by_session: Option<String>,
    /// Session ID that last updated this task
    pub updated_by_session: Option<String>,
    /// Owner/assignee of this task (agent name or user identifier)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Arbitrary key-value metadata for extensibility
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
    /// IDs of tasks that block THIS task (inverse of `blocks`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<TaskId>,
}

/// Input for creating a new task
#[derive(Clone, Debug, Default)]
pub struct NewTask {
    /// Short subject/title of the task
    pub subject: String,
    /// Detailed description of the task
    pub description: String,
    /// Priority level (defaults to Medium if None)
    pub priority: Option<TaskPriority>,
    /// Labels/tags for categorization
    pub labels: Option<Vec<String>>,
    /// IDs of tasks that this task blocks
    pub blocks: Option<Vec<TaskId>>,
    /// Owner/assignee of the task
    pub owner: Option<String>,
    /// Arbitrary key-value metadata
    pub metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// IDs of tasks that block THIS task
    pub blocked_by: Option<Vec<TaskId>>,
}

/// Input for updating an existing task
#[derive(Clone, Debug, Default)]
pub struct TaskUpdate {
    /// New subject/title (if Some)
    pub subject: Option<String>,
    /// New description (if Some)
    pub description: Option<String>,
    /// New status (if Some)
    pub status: Option<TaskStatus>,
    /// New priority (if Some)
    pub priority: Option<TaskPriority>,
    /// Replace all labels (if Some)
    pub labels: Option<Vec<String>>,
    /// Task IDs to add to blocks
    pub add_blocks: Option<Vec<TaskId>>,
    /// Task IDs to remove from blocks
    pub remove_blocks: Option<Vec<TaskId>>,
    /// New owner/assignee (if Some)
    pub owner: Option<String>,
    /// Metadata key-value pairs to merge (null value = delete key)
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Task IDs to add to blocked_by
    pub add_blocked_by: Option<Vec<TaskId>>,
    /// Task IDs to remove from blocked_by
    pub remove_blocked_by: Option<Vec<TaskId>>,
}

/// Errors that can occur during task operations
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    /// Task with the given ID was not found
    #[error("Task not found: {0}")]
    NotFound(String),
    /// Error occurred during storage operations
    #[error("Storage error: {0}")]
    StorageError(String),
    /// Invalid task data was provided
    #[error("Invalid task data: {0}")]
    InvalidData(String),
}

/// Metadata about the task store
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskStoreMeta {
    /// Version of the store format
    pub version: u32,
    /// Unique identifier for the project
    pub project_id: String,
    /// ISO 8601 timestamp when the store was created
    pub created_at: String,
    /// Revision counter for the store (incremented on each write)
    pub store_rev: u64,
}

/// Complete serializable state of the task store
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskStoreData {
    /// Store metadata
    pub meta: TaskStoreMeta,
    /// All tasks in the store
    pub tasks: Vec<Task>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_task_id_new_generates_uuid() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();

        // Should be 36 characters (UUID format)
        assert_eq!(id1.0.len(), 36);
        assert_eq!(id2.0.len(), 36);

        // Should be different IDs
        assert_ne!(id1, id2);

        // Should be valid UUID
        assert!(uuid::Uuid::parse_str(&id1.0).is_ok());
    }

    #[test]
    fn test_task_id_from_string() {
        let id = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(id.0, "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    }

    #[test]
    fn test_task_id_display() {
        let id = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(format!("{id}"), "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    }

    #[test]
    fn test_task_id_serde() {
        let id = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"01ARZ3NDEKTSV4RRFFQ69G5FAV\"");

        let parsed: TaskId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_task_status_serde() {
        // Test serialization
        assert_eq!(
            serde_json::to_string(&TaskStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Completed).unwrap(),
            "\"completed\""
        );

        // Test deserialization
        assert_eq!(
            serde_json::from_str::<TaskStatus>("\"pending\"").unwrap(),
            TaskStatus::Pending
        );
        assert_eq!(
            serde_json::from_str::<TaskStatus>("\"in_progress\"").unwrap(),
            TaskStatus::InProgress
        );
        assert_eq!(
            serde_json::from_str::<TaskStatus>("\"completed\"").unwrap(),
            TaskStatus::Completed
        );
    }

    #[test]
    fn test_task_priority_serde() {
        // Test serialization
        assert_eq!(
            serde_json::to_string(&TaskPriority::Low).unwrap(),
            "\"low\""
        );
        assert_eq!(
            serde_json::to_string(&TaskPriority::Medium).unwrap(),
            "\"medium\""
        );
        assert_eq!(
            serde_json::to_string(&TaskPriority::High).unwrap(),
            "\"high\""
        );

        // Test deserialization
        assert_eq!(
            serde_json::from_str::<TaskPriority>("\"low\"").unwrap(),
            TaskPriority::Low
        );
        assert_eq!(
            serde_json::from_str::<TaskPriority>("\"medium\"").unwrap(),
            TaskPriority::Medium
        );
        assert_eq!(
            serde_json::from_str::<TaskPriority>("\"high\"").unwrap(),
            TaskPriority::High
        );
    }

    #[test]
    fn test_task_status_default() {
        assert_eq!(TaskStatus::default(), TaskStatus::Pending);
    }

    #[test]
    fn test_task_priority_default() {
        assert_eq!(TaskPriority::default(), TaskPriority::Medium);
    }

    #[test]
    fn test_task_serialization_roundtrip() {
        let task = Task {
            id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            subject: "Implement feature X".to_string(),
            description: "Add the new feature X to the system".to_string(),
            status: TaskStatus::InProgress,
            priority: TaskPriority::High,
            labels: vec!["feature".to_string(), "urgent".to_string()],
            blocks: vec![TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAW")],
            created_at: "2025-01-23T10:00:00Z".to_string(),
            updated_at: "2025-01-23T11:00:00Z".to_string(),
            created_by_session: Some("session-123".to_string()),
            updated_by_session: Some("session-456".to_string()),
            owner: None,
            metadata: std::collections::HashMap::new(),
            blocked_by: vec![],
        };

        let json = serde_json::to_string_pretty(&task).unwrap();
        let parsed: Task = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, task.id);
        assert_eq!(parsed.subject, task.subject);
        assert_eq!(parsed.description, task.description);
        assert_eq!(parsed.status, task.status);
        assert_eq!(parsed.priority, task.priority);
        assert_eq!(parsed.labels, task.labels);
        assert_eq!(parsed.blocks, task.blocks);
        assert_eq!(parsed.created_at, task.created_at);
        assert_eq!(parsed.updated_at, task.updated_at);
        assert_eq!(parsed.created_by_session, task.created_by_session);
        assert_eq!(parsed.updated_by_session, task.updated_by_session);
    }

    #[test]
    fn test_task_store_data_serde() {
        let data = TaskStoreData {
            meta: TaskStoreMeta {
                version: 1,
                project_id: "my-project".to_string(),
                created_at: "2025-01-23T10:00:00Z".to_string(),
                store_rev: 42,
            },
            tasks: vec![
                Task {
                    id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                    subject: "Task 1".to_string(),
                    description: "Description 1".to_string(),
                    status: TaskStatus::Pending,
                    priority: TaskPriority::Medium,
                    labels: vec![],
                    blocks: vec![],
                    created_at: "2025-01-23T10:00:00Z".to_string(),
                    updated_at: "2025-01-23T10:00:00Z".to_string(),
                    created_by_session: None,
                    updated_by_session: None,
                    owner: None,
                    metadata: std::collections::HashMap::new(),
                    blocked_by: vec![],
                },
                Task {
                    id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAW"),
                    subject: "Task 2".to_string(),
                    description: "Description 2".to_string(),
                    status: TaskStatus::Completed,
                    priority: TaskPriority::Low,
                    labels: vec!["done".to_string()],
                    blocks: vec![],
                    created_at: "2025-01-23T11:00:00Z".to_string(),
                    updated_at: "2025-01-23T12:00:00Z".to_string(),
                    created_by_session: Some("session-1".to_string()),
                    updated_by_session: Some("session-2".to_string()),
                    owner: None,
                    metadata: std::collections::HashMap::new(),
                    blocked_by: vec![],
                },
            ],
        };

        let json = serde_json::to_string_pretty(&data).unwrap();
        let parsed: TaskStoreData = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.meta.version, data.meta.version);
        assert_eq!(parsed.meta.project_id, data.meta.project_id);
        assert_eq!(parsed.meta.created_at, data.meta.created_at);
        assert_eq!(parsed.meta.store_rev, data.meta.store_rev);
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(parsed.tasks[0].id, data.tasks[0].id);
        assert_eq!(parsed.tasks[1].id, data.tasks[1].id);
    }

    #[test]
    fn test_task_error_display() {
        let err = TaskError::NotFound("123".to_string());
        assert_eq!(err.to_string(), "Task not found: 123");

        let err = TaskError::StorageError("disk full".to_string());
        assert_eq!(err.to_string(), "Storage error: disk full");

        let err = TaskError::InvalidData("missing subject".to_string());
        assert_eq!(err.to_string(), "Invalid task data: missing subject");
    }

    #[test]
    fn test_task_update_default() {
        let update = TaskUpdate::default();
        assert!(update.subject.is_none());
        assert!(update.description.is_none());
        assert!(update.status.is_none());
        assert!(update.priority.is_none());
        assert!(update.labels.is_none());
        assert!(update.add_blocks.is_none());
        assert!(update.remove_blocks.is_none());
    }

    // ======================================================================
    // Tests for new TaskUpdate fields: owner, metadata, add_blocked_by, remove_blocked_by
    // These tests are written TDD-style - they will fail until implementation
    // ======================================================================

    #[test]
    fn test_task_update_has_owner_field() {
        // TaskUpdate should have an optional owner field to set task ownership
        let update = TaskUpdate {
            owner: Some("alice".to_string()),
            ..Default::default()
        };
        assert_eq!(update.owner, Some("alice".to_string()));

        // Default should be None
        let default_update = TaskUpdate::default();
        assert!(default_update.owner.is_none());
    }

    #[test]
    fn test_task_update_has_metadata_field() {
        // TaskUpdate should have a metadata field for merging key-value pairs
        // Setting a key to a value adds/updates it
        // Setting a key to null (Value::Null) removes it
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("key1".to_string(), serde_json::json!("value1"));
        metadata.insert("key2".to_string(), serde_json::json!(42));
        metadata.insert("key3".to_string(), serde_json::Value::Null); // delete key3

        let update = TaskUpdate {
            metadata: Some(metadata.clone()),
            ..Default::default()
        };
        assert!(update.metadata.is_some());
        let meta = update.metadata.unwrap();
        assert_eq!(meta.get("key1"), Some(&serde_json::json!("value1")));
        assert_eq!(meta.get("key2"), Some(&serde_json::json!(42)));
        assert_eq!(meta.get("key3"), Some(&serde_json::Value::Null));

        // Default should be None
        let default_update = TaskUpdate::default();
        assert!(default_update.metadata.is_none());
    }

    #[test]
    fn test_task_update_has_add_blocked_by_field() {
        // TaskUpdate should have add_blocked_by to add blocking relationships
        // blocked_by tracks which tasks block THIS task (inverse of blocks)
        let update = TaskUpdate {
            add_blocked_by: Some(vec![
                TaskId::from_string("blocker-1"),
                TaskId::from_string("blocker-2"),
            ]),
            ..Default::default()
        };
        assert!(update.add_blocked_by.is_some());
        let blocked_by = update.add_blocked_by.unwrap();
        assert_eq!(blocked_by.len(), 2);
        assert_eq!(blocked_by[0], TaskId::from_string("blocker-1"));
        assert_eq!(blocked_by[1], TaskId::from_string("blocker-2"));

        // Default should be None
        let default_update = TaskUpdate::default();
        assert!(default_update.add_blocked_by.is_none());
    }

    #[test]
    fn test_task_update_has_remove_blocked_by_field() {
        // TaskUpdate should have remove_blocked_by to remove blocking relationships
        let update = TaskUpdate {
            remove_blocked_by: Some(vec![TaskId::from_string("blocker-1")]),
            ..Default::default()
        };
        assert!(update.remove_blocked_by.is_some());
        let remove_blocked_by = update.remove_blocked_by.unwrap();
        assert_eq!(remove_blocked_by.len(), 1);
        assert_eq!(remove_blocked_by[0], TaskId::from_string("blocker-1"));

        // Default should be None
        let default_update = TaskUpdate::default();
        assert!(default_update.remove_blocked_by.is_none());
    }

    #[test]
    fn test_task_update_all_new_fields_together() {
        // Test setting all new fields at once
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "priority_reason".to_string(),
            serde_json::json!("urgent customer request"),
        );

        let update = TaskUpdate {
            owner: Some("bob".to_string()),
            metadata: Some(metadata),
            add_blocked_by: Some(vec![TaskId::from_string("prerequisite-task")]),
            remove_blocked_by: Some(vec![TaskId::from_string("old-blocker")]),
            ..Default::default()
        };

        assert_eq!(update.owner, Some("bob".to_string()));
        assert!(update.metadata.is_some());
        assert!(update.add_blocked_by.is_some());
        assert!(update.remove_blocked_by.is_some());
    }

    // ==========================================================================
    // Tests for new Task struct fields: owner, metadata, blocked_by
    // Written TDD-style - these will fail until implementation in Task #4
    // ==========================================================================

    // Task #1: Tests for Task struct with owner field
    mod task_owner_tests {
        use super::*;

        #[test]
        fn test_task_owner_field_exists() {
            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test description".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None, // New field
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![],
            };
            assert!(task.owner.is_none());
        }

        #[test]
        fn test_task_owner_with_value() {
            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test description".to_string(),
                status: TaskStatus::InProgress,
                priority: TaskPriority::High,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: Some("alice".to_string()),
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![],
            };
            assert_eq!(task.owner, Some("alice".to_string()));
        }

        #[test]
        fn test_task_owner_serialization() {
            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: Some("bob".to_string()),
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![],
            };

            let json = serde_json::to_string(&task).unwrap();
            assert!(json.contains("\"owner\":\"bob\""));

            let parsed: Task = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.owner, Some("bob".to_string()));
        }

        #[test]
        fn test_task_owner_none_serialization() {
            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![],
            };

            let json = serde_json::to_string(&task).unwrap();
            let parsed: Task = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.owner, None);
        }
    }

    // Task #2: Tests for Task struct with metadata field
    mod task_metadata_tests {
        use super::*;
        use std::collections::HashMap;

        #[test]
        fn test_task_metadata_field_exists() {
            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test description".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata: HashMap::new(),
                blocked_by: vec![],
            };
            assert!(task.metadata.is_empty());
        }

        #[test]
        fn test_task_metadata_with_values() {
            let mut metadata = HashMap::new();
            metadata.insert("priority_score".to_string(), serde_json::json!(42));
            metadata.insert("estimate_hours".to_string(), serde_json::json!(8.5));
            metadata.insert(
                "assignee_email".to_string(),
                serde_json::json!("alice@example.com"),
            );

            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test description".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata,
                blocked_by: vec![],
            };

            assert_eq!(task.metadata.len(), 3);
            assert_eq!(
                task.metadata.get("priority_score"),
                Some(&serde_json::json!(42))
            );
            assert_eq!(
                task.metadata.get("estimate_hours"),
                Some(&serde_json::json!(8.5))
            );
        }

        #[test]
        fn test_task_metadata_with_complex_values() {
            let mut metadata = HashMap::new();
            metadata.insert(
                "tags".to_string(),
                serde_json::json!(["frontend", "urgent"]),
            );
            metadata.insert(
                "config".to_string(),
                serde_json::json!({"retries": 3, "timeout": 30}),
            );

            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata,
                blocked_by: vec![],
            };

            let tags = task.metadata.get("tags").unwrap();
            assert!(tags.is_array());
            assert_eq!(tags.as_array().unwrap().len(), 2);

            let config = task.metadata.get("config").unwrap();
            assert!(config.is_object());
            assert_eq!(config.get("retries"), Some(&serde_json::json!(3)));
        }

        #[test]
        fn test_task_metadata_serialization() {
            let mut metadata = HashMap::new();
            metadata.insert("key1".to_string(), serde_json::json!("value1"));
            metadata.insert("key2".to_string(), serde_json::json!(123));

            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata,
                blocked_by: vec![],
            };

            let json = serde_json::to_string(&task).unwrap();
            assert!(json.contains("\"metadata\""));

            let parsed: Task = serde_json::from_str(&json).unwrap();
            assert_eq!(
                parsed.metadata.get("key1"),
                Some(&serde_json::json!("value1"))
            );
            assert_eq!(parsed.metadata.get("key2"), Some(&serde_json::json!(123)));
        }
    }

    // Task #3: Tests for Task struct with blocked_by field
    mod task_blocked_by_tests {
        use super::*;

        #[test]
        fn test_task_blocked_by_field_exists() {
            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Test task".to_string(),
                description: "Test description".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![],
            };
            assert!(task.blocked_by.is_empty());
        }

        #[test]
        fn test_task_blocked_by_single_blocker() {
            let blocker_id = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAW");

            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Blocked task".to_string(),
                description: "This task is blocked by another".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![blocker_id.clone()],
            };

            assert_eq!(task.blocked_by.len(), 1);
            assert_eq!(task.blocked_by[0], blocker_id);
        }

        #[test]
        fn test_task_blocked_by_multiple_blockers() {
            let blocker1 = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FA1");
            let blocker2 = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FA2");
            let blocker3 = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FA3");

            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Multi-blocked task".to_string(),
                description: "This task is blocked by three others".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::High,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![blocker1.clone(), blocker2.clone(), blocker3.clone()],
            };

            assert_eq!(task.blocked_by.len(), 3);
            assert!(task.blocked_by.contains(&blocker1));
            assert!(task.blocked_by.contains(&blocker2));
            assert!(task.blocked_by.contains(&blocker3));
        }

        #[test]
        fn test_task_blocked_by_serialization() {
            let blocker_id = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAW");

            let task = Task {
                id: TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                subject: "Blocked task".to_string(),
                description: "Test".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![],
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![blocker_id.clone()],
            };

            let json = serde_json::to_string(&task).unwrap();
            assert!(json.contains("\"blocked_by\""));
            assert!(json.contains("01ARZ3NDEKTSV4RRFFQ69G5FAW"));

            let parsed: Task = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.blocked_by.len(), 1);
            assert_eq!(parsed.blocked_by[0], blocker_id);
        }

        #[test]
        fn test_task_blocks_vs_blocked_by_distinction() {
            // Task A blocks Task B
            // From A's perspective: blocks = [B]
            // From B's perspective: blocked_by = [A]

            let task_a_id = TaskId::from_string("TASK_A_ID_00000000000000");
            let task_b_id = TaskId::from_string("TASK_B_ID_00000000000000");

            // Task A - the blocker
            let task_a = Task {
                id: task_a_id.clone(),
                subject: "Task A - the blocker".to_string(),
                description: "This task blocks Task B".to_string(),
                status: TaskStatus::InProgress,
                priority: TaskPriority::High,
                labels: vec![],
                blocks: vec![task_b_id.clone()], // A blocks B
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![], // A is not blocked by anything
            };

            // Task B - the blocked task
            let task_b = Task {
                id: task_b_id.clone(),
                subject: "Task B - blocked".to_string(),
                description: "This task is blocked by Task A".to_string(),
                status: TaskStatus::Pending,
                priority: TaskPriority::Medium,
                labels: vec![],
                blocks: vec![], // B doesn't block anything
                created_at: "2025-01-24T10:00:00Z".to_string(),
                updated_at: "2025-01-24T10:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![task_a_id.clone()], // B is blocked by A
            };

            // Verify the relationship
            assert!(task_a.blocks.contains(&task_b_id));
            assert!(task_a.blocked_by.is_empty());

            assert!(task_b.blocks.is_empty());
            assert!(task_b.blocked_by.contains(&task_a_id));
        }
    }
}
