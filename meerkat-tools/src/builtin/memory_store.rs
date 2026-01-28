//! In-memory task store for testing
//!
//! This module provides [`MemoryTaskStore`], a simple in-memory implementation
//! of the [`TaskStore`] trait for use in tests.

use super::store::TaskStore;
use super::types::{NewTask, Task, TaskError, TaskId, TaskStatus, TaskUpdate};
use async_trait::async_trait;
use std::sync::RwLock;

/// In-memory task store for testing
///
/// This store keeps all tasks in memory using a `RwLock<Vec<Task>>`.
/// It is thread-safe and can be used in async contexts.
///
/// # Example
///
/// ```ignore
/// use meerkat_tools::builtin::{MemoryTaskStore, TaskStore, NewTask};
///
/// let store = MemoryTaskStore::new();
///
/// let task = store.create(
///     NewTask {
///         subject: "Test task".to_string(),
///         description: "A test task".to_string(),
///         priority: None,
///         labels: None,
///         blocks: None,
///     },
///     None
/// ).await.unwrap();
///
/// assert_eq!(store.list().await.unwrap().len(), 1);
/// ```
pub struct MemoryTaskStore {
    tasks: RwLock<Vec<Task>>,
}

impl MemoryTaskStore {
    /// Create a new empty memory store
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(Vec::new()),
        }
    }

    /// Create a memory store pre-populated with tasks
    ///
    /// This is useful for setting up test fixtures.
    pub fn with_tasks(tasks: Vec<Task>) -> Self {
        Self {
            tasks: RwLock::new(tasks),
        }
    }

    /// Get the current number of tasks in the store
    ///
    /// This is a convenience method for testing.
    pub fn len(&self) -> usize {
        self.tasks.read().map(|t| t.len()).unwrap_or(0)
    }

    /// Check if the store is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemoryTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskStore for MemoryTaskStore {
    async fn list(&self) -> Result<Vec<Task>, TaskError> {
        let tasks = self
            .tasks
            .read()
            .map_err(|e| TaskError::StorageError(e.to_string()))?;
        Ok(tasks.clone())
    }

    async fn get(&self, id: &TaskId) -> Result<Option<Task>, TaskError> {
        let tasks = self
            .tasks
            .read()
            .map_err(|e| TaskError::StorageError(e.to_string()))?;
        Ok(tasks.iter().find(|t| &t.id == id).cloned())
    }

    async fn create(&self, new_task: NewTask, session_id: Option<&str>) -> Result<Task, TaskError> {
        let now = chrono::Utc::now().to_rfc3339();
        let task = Task {
            id: TaskId::new(),
            subject: new_task.subject,
            description: new_task.description,
            status: TaskStatus::default(),
            priority: new_task.priority.unwrap_or_default(),
            labels: new_task.labels.unwrap_or_default(),
            blocks: new_task.blocks.unwrap_or_default(),
            created_at: now.clone(),
            updated_at: now,
            created_by_session: session_id.map(String::from),
            updated_by_session: session_id.map(String::from),
            owner: new_task.owner,
            metadata: new_task.metadata.unwrap_or_default(),
            blocked_by: new_task.blocked_by.unwrap_or_default(),
        };

        let mut tasks = self
            .tasks
            .write()
            .map_err(|e| TaskError::StorageError(e.to_string()))?;
        tasks.push(task.clone());
        Ok(task)
    }

    async fn update(
        &self,
        id: &TaskId,
        update: TaskUpdate,
        session_id: Option<&str>,
    ) -> Result<Task, TaskError> {
        let mut tasks = self
            .tasks
            .write()
            .map_err(|e| TaskError::StorageError(e.to_string()))?;
        let task = tasks
            .iter_mut()
            .find(|t| &t.id == id)
            .ok_or_else(|| TaskError::NotFound(id.0.clone()))?;

        if let Some(subject) = update.subject {
            task.subject = subject;
        }
        if let Some(description) = update.description {
            task.description = description;
        }
        if let Some(status) = update.status {
            task.status = status;
        }
        if let Some(priority) = update.priority {
            task.priority = priority;
        }
        if let Some(labels) = update.labels {
            task.labels = labels;
        }
        if let Some(add_blocks) = update.add_blocks {
            for block_id in add_blocks {
                if !task.blocks.contains(&block_id) {
                    task.blocks.push(block_id);
                }
            }
        }
        if let Some(remove_blocks) = update.remove_blocks {
            task.blocks.retain(|b| !remove_blocks.contains(b));
        }

        // Handle new fields: owner, metadata, add_blocked_by, remove_blocked_by
        if let Some(owner) = update.owner {
            task.owner = Some(owner);
        }
        if let Some(metadata) = update.metadata {
            for (key, value) in metadata {
                if value.is_null() {
                    // Null value means delete the key
                    task.metadata.remove(&key);
                } else {
                    task.metadata.insert(key, value);
                }
            }
        }
        if let Some(add_blocked_by) = update.add_blocked_by {
            for block_id in add_blocked_by {
                if !task.blocked_by.contains(&block_id) {
                    task.blocked_by.push(block_id);
                }
            }
        }
        if let Some(remove_blocked_by) = update.remove_blocked_by {
            task.blocked_by.retain(|b| !remove_blocked_by.contains(b));
        }

        task.updated_at = chrono::Utc::now().to_rfc3339();
        task.updated_by_session = session_id.map(String::from);

        Ok(task.clone())
    }

    async fn delete(&self, id: &TaskId) -> Result<(), TaskError> {
        let mut tasks = self
            .tasks
            .write()
            .map_err(|e| TaskError::StorageError(e.to_string()))?;
        let len_before = tasks.len();
        tasks.retain(|t| &t.id != id);
        if tasks.len() == len_before {
            return Err(TaskError::NotFound(id.0.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::types::{TaskPriority, TaskStatus};

    #[tokio::test]
    async fn test_memory_store_create_and_get() {
        let store = MemoryTaskStore::new();

        let new_task = NewTask {
            subject: "Test task".to_string(),
            description: "Test description".to_string(),
            priority: Some(TaskPriority::High),
            labels: Some(vec!["test".to_string(), "important".to_string()]),
            blocks: None,
            ..Default::default()
        };

        let created = store.create(new_task, Some("session-1")).await.unwrap();

        // Verify created task fields
        assert_eq!(created.subject, "Test task");
        assert_eq!(created.description, "Test description");
        assert_eq!(created.priority, TaskPriority::High);
        assert_eq!(
            created.labels,
            vec!["test".to_string(), "important".to_string()]
        );
        assert_eq!(created.status, TaskStatus::Pending);
        assert_eq!(created.created_by_session, Some("session-1".to_string()));
        assert_eq!(created.updated_by_session, Some("session-1".to_string()));
        assert!(!created.created_at.is_empty());
        assert!(!created.updated_at.is_empty());
        assert_eq!(created.id.0.len(), 26); // ULID format

        // Verify we can retrieve it
        let fetched = store.get(&created.id).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.subject, created.subject);
        assert_eq!(fetched.description, created.description);
    }

    #[tokio::test]
    async fn test_memory_store_create_with_defaults() {
        let store = MemoryTaskStore::new();

        let new_task = NewTask {
            subject: "Simple task".to_string(),
            description: "No optional fields".to_string(),
            priority: None,
            labels: None,
            blocks: None,
            ..Default::default()
        };

        let created = store.create(new_task, None).await.unwrap();

        // Verify defaults are applied
        assert_eq!(created.priority, TaskPriority::Medium);
        assert!(created.labels.is_empty());
        assert!(created.blocks.is_empty());
        assert!(created.created_by_session.is_none());
        assert!(created.updated_by_session.is_none());
    }

    #[tokio::test]
    async fn test_memory_store_get_nonexistent() {
        let store = MemoryTaskStore::new();

        let result = store
            .get(&TaskId::from_string("nonexistent"))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_memory_store_list() {
        let store = MemoryTaskStore::new();

        // Initially empty
        let tasks = store.list().await.unwrap();
        assert!(tasks.is_empty());

        // Add some tasks
        let task1 = NewTask {
            subject: "Task 1".to_string(),
            description: "First task".to_string(),
            priority: Some(TaskPriority::Low),
            labels: None,
            blocks: None,
            ..Default::default()
        };
        let task2 = NewTask {
            subject: "Task 2".to_string(),
            description: "Second task".to_string(),
            priority: Some(TaskPriority::High),
            labels: None,
            blocks: None,
            ..Default::default()
        };
        let task3 = NewTask {
            subject: "Task 3".to_string(),
            description: "Third task".to_string(),
            priority: None,
            labels: Some(vec!["urgent".to_string()]),
            blocks: None,
            ..Default::default()
        };

        let created1 = store.create(task1, None).await.unwrap();
        let created2 = store.create(task2, None).await.unwrap();
        let created3 = store.create(task3, None).await.unwrap();

        // List should return all tasks
        let tasks = store.list().await.unwrap();
        assert_eq!(tasks.len(), 3);

        // Verify all tasks are present
        let ids: Vec<_> = tasks.iter().map(|t| &t.id).collect();
        assert!(ids.contains(&&created1.id));
        assert!(ids.contains(&&created2.id));
        assert!(ids.contains(&&created3.id));
    }

    #[tokio::test]
    async fn test_memory_store_update() {
        let store = MemoryTaskStore::new();

        let new_task = NewTask {
            subject: "Original subject".to_string(),
            description: "Original description".to_string(),
            priority: Some(TaskPriority::Low),
            labels: Some(vec!["initial".to_string()]),
            blocks: None,
            ..Default::default()
        };

        let created = store.create(new_task, Some("session-1")).await.unwrap();
        let original_created_at = created.created_at.clone();

        // Small delay to ensure updated_at is different
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Update the task
        let update = TaskUpdate {
            subject: Some("Updated subject".to_string()),
            description: Some("Updated description".to_string()),
            status: Some(TaskStatus::InProgress),
            priority: Some(TaskPriority::High),
            labels: Some(vec!["updated".to_string(), "reviewed".to_string()]),
            add_blocks: None,
            remove_blocks: None,
            owner: None,
            metadata: None,
            add_blocked_by: None,
            remove_blocked_by: None,
        };

        let updated = store
            .update(&created.id, update, Some("session-2"))
            .await
            .unwrap();

        // Verify updates
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.subject, "Updated subject");
        assert_eq!(updated.description, "Updated description");
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.priority, TaskPriority::High);
        assert_eq!(
            updated.labels,
            vec!["updated".to_string(), "reviewed".to_string()]
        );
        assert_eq!(updated.created_at, original_created_at); // unchanged
        assert_eq!(updated.created_by_session, Some("session-1".to_string())); // unchanged
        assert_eq!(updated.updated_by_session, Some("session-2".to_string()));
        assert_ne!(updated.updated_at, original_created_at); // changed
    }

    #[tokio::test]
    async fn test_memory_store_partial_update() {
        let store = MemoryTaskStore::new();

        let new_task = NewTask {
            subject: "Original".to_string(),
            description: "Original desc".to_string(),
            priority: Some(TaskPriority::High),
            labels: Some(vec!["keep".to_string()]),
            blocks: None,
            ..Default::default()
        };

        let created = store.create(new_task, None).await.unwrap();

        // Only update subject
        let update = TaskUpdate {
            subject: Some("New subject".to_string()),
            ..Default::default()
        };

        let updated = store.update(&created.id, update, None).await.unwrap();

        // Subject changed, everything else unchanged
        assert_eq!(updated.subject, "New subject");
        assert_eq!(updated.description, "Original desc");
        assert_eq!(updated.priority, TaskPriority::High);
        assert_eq!(updated.labels, vec!["keep".to_string()]);
        assert_eq!(updated.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_memory_store_update_not_found() {
        let store = MemoryTaskStore::new();

        let update = TaskUpdate {
            subject: Some("Updated".to_string()),
            ..Default::default()
        };

        let result = store
            .update(&TaskId::from_string("nonexistent"), update, None)
            .await;

        assert!(matches!(result, Err(TaskError::NotFound(id)) if id == "nonexistent"));
    }

    #[tokio::test]
    async fn test_memory_store_delete() {
        let store = MemoryTaskStore::new();

        let new_task = NewTask {
            subject: "To delete".to_string(),
            description: "Will be deleted".to_string(),
            priority: None,
            labels: None,
            blocks: None,
            ..Default::default()
        };

        let created = store.create(new_task, None).await.unwrap();

        // Verify task exists
        assert!(store.get(&created.id).await.unwrap().is_some());
        assert_eq!(store.len(), 1);

        // Delete it
        store.delete(&created.id).await.unwrap();

        // Verify task is gone
        assert!(store.get(&created.id).await.unwrap().is_none());
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn test_memory_store_delete_not_found() {
        let store = MemoryTaskStore::new();

        let result = store.delete(&TaskId::from_string("nonexistent")).await;

        assert!(matches!(result, Err(TaskError::NotFound(id)) if id == "nonexistent"));
    }

    #[tokio::test]
    async fn test_memory_store_delete_multiple() {
        let store = MemoryTaskStore::new();

        // Create multiple tasks
        for i in 1..=5 {
            store
                .create(
                    NewTask {
                        subject: format!("Task {}", i),
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
        }

        assert_eq!(store.len(), 5);

        let tasks = store.list().await.unwrap();

        // Delete tasks 2 and 4
        store.delete(&tasks[1].id).await.unwrap();
        store.delete(&tasks[3].id).await.unwrap();

        assert_eq!(store.len(), 3);

        // Verify correct tasks remain
        assert!(store.get(&tasks[0].id).await.unwrap().is_some());
        assert!(store.get(&tasks[1].id).await.unwrap().is_none());
        assert!(store.get(&tasks[2].id).await.unwrap().is_some());
        assert!(store.get(&tasks[3].id).await.unwrap().is_none());
        assert!(store.get(&tasks[4].id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_memory_store_add_blocks() {
        let store = MemoryTaskStore::new();

        // Create two tasks
        let task1 = store
            .create(
                NewTask {
                    subject: "Task 1".to_string(),
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

        let task2 = store
            .create(
                NewTask {
                    subject: "Task 2".to_string(),
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

        let task3 = store
            .create(
                NewTask {
                    subject: "Task 3".to_string(),
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

        // Add blocks to task1
        let update = TaskUpdate {
            add_blocks: Some(vec![task2.id.clone(), task3.id.clone()]),
            ..Default::default()
        };

        let updated = store.update(&task1.id, update, None).await.unwrap();

        assert_eq!(updated.blocks.len(), 2);
        assert!(updated.blocks.contains(&task2.id));
        assert!(updated.blocks.contains(&task3.id));

        // Adding the same blocks again should not duplicate
        let update = TaskUpdate {
            add_blocks: Some(vec![task2.id.clone()]),
            ..Default::default()
        };

        let updated = store.update(&task1.id, update, None).await.unwrap();
        assert_eq!(updated.blocks.len(), 2); // Still 2, not 3
    }

    #[tokio::test]
    async fn test_memory_store_remove_blocks() {
        let store = MemoryTaskStore::new();

        // Create tasks with initial blocks
        let blocker1 = TaskId::from_string("blocker-1");
        let blocker2 = TaskId::from_string("blocker-2");
        let blocker3 = TaskId::from_string("blocker-3");

        let task = store
            .create(
                NewTask {
                    subject: "Main task".to_string(),
                    description: "".to_string(),
                    priority: None,
                    labels: None,
                    blocks: Some(vec![blocker1.clone(), blocker2.clone(), blocker3.clone()]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        assert_eq!(task.blocks.len(), 3);

        // Remove one block
        let update = TaskUpdate {
            remove_blocks: Some(vec![blocker2.clone()]),
            ..Default::default()
        };

        let updated = store.update(&task.id, update, None).await.unwrap();
        assert_eq!(updated.blocks.len(), 2);
        assert!(updated.blocks.contains(&blocker1));
        assert!(!updated.blocks.contains(&blocker2));
        assert!(updated.blocks.contains(&blocker3));

        // Remove multiple blocks
        let update = TaskUpdate {
            remove_blocks: Some(vec![blocker1.clone(), blocker3.clone()]),
            ..Default::default()
        };

        let updated = store.update(&task.id, update, None).await.unwrap();
        assert!(updated.blocks.is_empty());
    }

    #[tokio::test]
    async fn test_memory_store_add_and_remove_blocks_same_update() {
        let store = MemoryTaskStore::new();

        let blocker1 = TaskId::from_string("blocker-1");
        let blocker2 = TaskId::from_string("blocker-2");

        let task = store
            .create(
                NewTask {
                    subject: "Task".to_string(),
                    description: "".to_string(),
                    priority: None,
                    labels: None,
                    blocks: Some(vec![blocker1.clone()]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        // Add blocker2 and remove blocker1 in the same update
        let update = TaskUpdate {
            add_blocks: Some(vec![blocker2.clone()]),
            remove_blocks: Some(vec![blocker1.clone()]),
            ..Default::default()
        };

        let updated = store.update(&task.id, update, None).await.unwrap();

        // add_blocks is processed first, then remove_blocks
        assert_eq!(updated.blocks.len(), 1);
        assert!(!updated.blocks.contains(&blocker1));
        assert!(updated.blocks.contains(&blocker2));
    }

    #[tokio::test]
    async fn test_memory_store_with_tasks() {
        let existing_tasks = vec![
            Task {
                id: TaskId::from_string("task-1"),
                subject: "Existing task 1".to_string(),
                description: "".to_string(),
                status: TaskStatus::Completed,
                priority: TaskPriority::High,
                labels: vec![],
                blocks: vec![],
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![],
                created_at: "2025-01-01T00:00:00Z".to_string(),
                updated_at: "2025-01-01T00:00:00Z".to_string(),
                created_by_session: None,
                updated_by_session: None,
            },
            Task {
                id: TaskId::from_string("task-2"),
                subject: "Existing task 2".to_string(),
                description: "".to_string(),
                status: TaskStatus::InProgress,
                priority: TaskPriority::Medium,
                labels: vec!["work".to_string()],
                blocks: vec![],
                owner: None,
                metadata: std::collections::HashMap::new(),
                blocked_by: vec![],
                created_at: "2025-01-02T00:00:00Z".to_string(),
                updated_at: "2025-01-02T00:00:00Z".to_string(),
                created_by_session: Some("old-session".to_string()),
                updated_by_session: Some("old-session".to_string()),
            },
        ];

        let store = MemoryTaskStore::with_tasks(existing_tasks);

        assert_eq!(store.len(), 2);
        assert!(!store.is_empty());

        // Verify we can retrieve pre-populated tasks
        let task1 = store
            .get(&TaskId::from_string("task-1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(task1.subject, "Existing task 1");
        assert_eq!(task1.status, TaskStatus::Completed);

        let task2 = store
            .get(&TaskId::from_string("task-2"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(task2.subject, "Existing task 2");
        assert_eq!(task2.labels, vec!["work".to_string()]);
    }

    #[tokio::test]
    async fn test_memory_store_default() {
        let store = MemoryTaskStore::default();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    // ======================================================================
    // Tests for TaskUpdate new fields: owner, metadata, add_blocked_by, remove_blocked_by
    // These tests verify the store correctly handles the update semantics
    // ======================================================================

    #[tokio::test]
    async fn test_memory_store_update_owner() {
        let store = MemoryTaskStore::new();

        let task = store
            .create(
                NewTask {
                    subject: "Task with no owner".to_string(),
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

        // Initially no owner
        assert!(task.owner.is_none());

        // Set owner
        let update = TaskUpdate {
            owner: Some("alice".to_string()),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();
        assert_eq!(updated.owner, Some("alice".to_string()));

        // Change owner
        let update = TaskUpdate {
            owner: Some("bob".to_string()),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();
        assert_eq!(updated.owner, Some("bob".to_string()));

        // Note: To clear owner, we'd need to support explicit None vs absent
        // For now, absent owner in update means "don't change"
    }

    #[tokio::test]
    async fn test_memory_store_update_metadata_merge() {
        let store = MemoryTaskStore::new();

        let task = store
            .create(
                NewTask {
                    subject: "Task with metadata".to_string(),
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

        // Initially empty metadata
        assert!(task.metadata.is_empty());

        // Add some metadata
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("key1".to_string(), serde_json::json!("value1"));
        metadata.insert("key2".to_string(), serde_json::json!(100));

        let update = TaskUpdate {
            metadata: Some(metadata),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        assert_eq!(updated.metadata.len(), 2);
        assert_eq!(
            updated.metadata.get("key1"),
            Some(&serde_json::json!("value1"))
        );
        assert_eq!(updated.metadata.get("key2"), Some(&serde_json::json!(100)));

        // Merge more metadata - existing keys unchanged, new keys added
        let mut metadata2 = std::collections::HashMap::new();
        metadata2.insert("key2".to_string(), serde_json::json!(200)); // update existing
        metadata2.insert("key3".to_string(), serde_json::json!("new")); // add new

        let update = TaskUpdate {
            metadata: Some(metadata2),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        assert_eq!(updated.metadata.len(), 3);
        assert_eq!(
            updated.metadata.get("key1"),
            Some(&serde_json::json!("value1"))
        ); // unchanged
        assert_eq!(updated.metadata.get("key2"), Some(&serde_json::json!(200))); // updated
        assert_eq!(
            updated.metadata.get("key3"),
            Some(&serde_json::json!("new"))
        ); // added
    }

    #[tokio::test]
    async fn test_memory_store_update_metadata_delete_with_null() {
        let store = MemoryTaskStore::new();

        let task = store
            .create(
                NewTask {
                    subject: "Task with metadata to delete".to_string(),
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

        // Add initial metadata
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("keep".to_string(), serde_json::json!("keep me"));
        metadata.insert(
            "delete_me".to_string(),
            serde_json::json!("will be deleted"),
        );

        let update = TaskUpdate {
            metadata: Some(metadata),
            ..Default::default()
        };
        store.update(&task.id, update, None).await.unwrap();

        // Delete a key by setting it to null
        let mut metadata_delete = std::collections::HashMap::new();
        metadata_delete.insert("delete_me".to_string(), serde_json::Value::Null);

        let update = TaskUpdate {
            metadata: Some(metadata_delete),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        // "delete_me" should be removed, "keep" should remain
        assert_eq!(updated.metadata.len(), 1);
        assert_eq!(
            updated.metadata.get("keep"),
            Some(&serde_json::json!("keep me"))
        );
        assert!(!updated.metadata.contains_key("delete_me"));
    }

    #[tokio::test]
    async fn test_memory_store_update_add_blocked_by() {
        let store = MemoryTaskStore::new();

        // Create blocking tasks
        let blocker1 = store
            .create(
                NewTask {
                    subject: "Blocker 1".to_string(),
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

        let blocker2 = store
            .create(
                NewTask {
                    subject: "Blocker 2".to_string(),
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

        // Create the blocked task
        let task = store
            .create(
                NewTask {
                    subject: "Blocked task".to_string(),
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

        // Initially no blocked_by
        assert!(task.blocked_by.is_empty());

        // Add blocker1 to blocked_by
        let update = TaskUpdate {
            add_blocked_by: Some(vec![blocker1.id.clone()]),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        assert_eq!(updated.blocked_by.len(), 1);
        assert!(updated.blocked_by.contains(&blocker1.id));

        // Add blocker2 (blocker1 should still be there)
        let update = TaskUpdate {
            add_blocked_by: Some(vec![blocker2.id.clone()]),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        assert_eq!(updated.blocked_by.len(), 2);
        assert!(updated.blocked_by.contains(&blocker1.id));
        assert!(updated.blocked_by.contains(&blocker2.id));

        // Adding same blocker again should not duplicate
        let update = TaskUpdate {
            add_blocked_by: Some(vec![blocker1.id.clone()]),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        assert_eq!(updated.blocked_by.len(), 2); // Still 2, no duplicate
    }

    #[tokio::test]
    async fn test_memory_store_update_remove_blocked_by() {
        let store = MemoryTaskStore::new();

        let blocker1 = TaskId::from_string("blocker-1");
        let blocker2 = TaskId::from_string("blocker-2");
        let blocker3 = TaskId::from_string("blocker-3");

        // Create task with initial blocked_by
        // Note: This requires Task to have blocked_by field - will fail until implemented
        let task = store
            .create(
                NewTask {
                    subject: "Task blocked by multiple".to_string(),
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

        // First add some blocked_by entries
        let update = TaskUpdate {
            add_blocked_by: Some(vec![blocker1.clone(), blocker2.clone(), blocker3.clone()]),
            ..Default::default()
        };
        let task = store.update(&task.id, update, None).await.unwrap();
        assert_eq!(task.blocked_by.len(), 3);

        // Remove one
        let update = TaskUpdate {
            remove_blocked_by: Some(vec![blocker2.clone()]),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        assert_eq!(updated.blocked_by.len(), 2);
        assert!(updated.blocked_by.contains(&blocker1));
        assert!(!updated.blocked_by.contains(&blocker2));
        assert!(updated.blocked_by.contains(&blocker3));

        // Remove multiple
        let update = TaskUpdate {
            remove_blocked_by: Some(vec![blocker1.clone(), blocker3.clone()]),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        assert!(updated.blocked_by.is_empty());
    }

    #[tokio::test]
    async fn test_memory_store_update_add_and_remove_blocked_by_same_update() {
        let store = MemoryTaskStore::new();

        let blocker1 = TaskId::from_string("blocker-1");
        let blocker2 = TaskId::from_string("blocker-2");

        let task = store
            .create(
                NewTask {
                    subject: "Task".to_string(),
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

        // Add blocker1 first
        let update = TaskUpdate {
            add_blocked_by: Some(vec![blocker1.clone()]),
            ..Default::default()
        };
        store.update(&task.id, update, None).await.unwrap();

        // Add blocker2 and remove blocker1 in the same update
        let update = TaskUpdate {
            add_blocked_by: Some(vec![blocker2.clone()]),
            remove_blocked_by: Some(vec![blocker1.clone()]),
            ..Default::default()
        };
        let updated = store.update(&task.id, update, None).await.unwrap();

        // add_blocked_by is processed first, then remove_blocked_by
        assert_eq!(updated.blocked_by.len(), 1);
        assert!(!updated.blocked_by.contains(&blocker1));
        assert!(updated.blocked_by.contains(&blocker2));
    }
}
