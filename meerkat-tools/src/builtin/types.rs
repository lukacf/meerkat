//! Task types for the built-in task management tools
//!
//! This module defines the core types for task management including
//! [`Task`], [`TaskId`], [`TaskStatus`], and [`TaskPriority`].

use serde::{Deserialize, Serialize};

/// Task ID - ULID string for unique task identification
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    /// Create a new TaskId with a generated ULID
    pub fn new() -> Self {
        Self(ulid::Ulid::new().to_string())
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is waiting to be started
    #[default]
    Pending,
    /// Task is actively being worked on
    InProgress,
    /// Task has been finished
    Completed,
}

/// Priority level for a task
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    /// Low priority task
    Low,
    /// Medium priority task (default)
    #[default]
    Medium,
    /// High priority task
    High,
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
}

/// Input for creating a new task
#[derive(Clone, Debug)]
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
mod tests {
    use super::*;

    #[test]
    fn test_task_id_new_generates_ulid() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();

        // Should be 26 characters (ULID format)
        assert_eq!(id1.0.len(), 26);
        assert_eq!(id2.0.len(), 26);

        // Should be different IDs
        assert_ne!(id1, id2);

        // Should be valid ULID (uppercase alphanumeric, no I, L, O, U)
        for c in id1.0.chars() {
            assert!(c.is_ascii_alphanumeric());
        }
    }

    #[test]
    fn test_task_id_from_string() {
        let id = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(id.0, "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    }

    #[test]
    fn test_task_id_display() {
        let id = TaskId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(format!("{}", id), "01ARZ3NDEKTSV4RRFFQ69G5FAV");
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
}
