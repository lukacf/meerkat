//! Task store trait for persisting tasks
//!
//! This module defines the [`TaskStore`] trait that abstracts over
//! different storage backends for tasks.

use super::types::{NewTask, Task, TaskError, TaskId, TaskUpdate};
use async_trait::async_trait;

/// Trait for task storage backends
///
/// Implementations of this trait provide persistence for tasks.
/// The trait is async to support both in-memory and file-based storage.
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// List all tasks in the store
    async fn list(&self) -> Result<Vec<Task>, TaskError>;

    /// Get a single task by ID
    ///
    /// Returns `Ok(None)` if the task does not exist.
    async fn get(&self, id: &TaskId) -> Result<Option<Task>, TaskError>;

    /// Create a new task
    ///
    /// # Arguments
    /// * `task` - The new task data
    /// * `session_id` - Optional session ID for tracking who created the task
    ///
    /// # Returns
    /// The created task with generated ID and timestamps
    async fn create(&self, task: NewTask, session_id: Option<&str>) -> Result<Task, TaskError>;

    /// Update an existing task
    ///
    /// # Arguments
    /// * `id` - The ID of the task to update
    /// * `update` - The fields to update
    /// * `session_id` - Optional session ID for tracking who updated the task
    ///
    /// # Returns
    /// The updated task
    ///
    /// # Errors
    /// Returns `TaskError::NotFound` if the task does not exist
    async fn update(
        &self,
        id: &TaskId,
        update: TaskUpdate,
        session_id: Option<&str>,
    ) -> Result<Task, TaskError>;

    /// Delete a task by ID
    ///
    /// # Errors
    /// Returns `TaskError::NotFound` if the task does not exist
    async fn delete(&self, id: &TaskId) -> Result<(), TaskError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::types::{TaskPriority, TaskStatus};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A simple in-memory task store for testing
    struct MemoryTaskStore {
        tasks: Mutex<HashMap<String, Task>>,
    }

    impl MemoryTaskStore {
        fn new() -> Self {
            Self {
                tasks: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl TaskStore for MemoryTaskStore {
        async fn list(&self) -> Result<Vec<Task>, TaskError> {
            let tasks = self.tasks.lock().unwrap();
            Ok(tasks.values().cloned().collect())
        }

        async fn get(&self, id: &TaskId) -> Result<Option<Task>, TaskError> {
            let tasks = self.tasks.lock().unwrap();
            Ok(tasks.get(&id.0).cloned())
        }

        async fn create(&self, task: NewTask, session_id: Option<&str>) -> Result<Task, TaskError> {
            let now = chrono::Utc::now().to_rfc3339();
            let new_task = Task {
                id: TaskId::new(),
                subject: task.subject,
                description: task.description,
                status: TaskStatus::Pending,
                priority: task.priority.unwrap_or_default(),
                labels: task.labels.unwrap_or_default(),
                blocks: task.blocks.unwrap_or_default(),
                owner: task.owner,
                metadata: task.metadata.unwrap_or_default(),
                blocked_by: task.blocked_by.unwrap_or_default(),
                created_at: now.clone(),
                updated_at: now,
                created_by_session: session_id.map(String::from),
                updated_by_session: session_id.map(String::from),
            };

            let mut tasks = self.tasks.lock().unwrap();
            tasks.insert(new_task.id.0.clone(), new_task.clone());
            Ok(new_task)
        }

        async fn update(
            &self,
            id: &TaskId,
            update: TaskUpdate,
            session_id: Option<&str>,
        ) -> Result<Task, TaskError> {
            let mut tasks = self.tasks.lock().unwrap();
            let task = tasks
                .get_mut(&id.0)
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

            task.updated_at = chrono::Utc::now().to_rfc3339();
            task.updated_by_session = session_id.map(String::from);

            Ok(task.clone())
        }

        async fn delete(&self, id: &TaskId) -> Result<(), TaskError> {
            let mut tasks = self.tasks.lock().unwrap();
            tasks
                .remove(&id.0)
                .ok_or_else(|| TaskError::NotFound(id.0.clone()))?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_memory_store_create_and_get() {
        let store = MemoryTaskStore::new();

        let new_task = NewTask {
            subject: "Test task".to_string(),
            description: "Test description".to_string(),
            priority: Some(TaskPriority::High),
            labels: Some(vec!["test".to_string()]),
            blocks: None,
            ..Default::default()
        };

        let created = store.create(new_task, Some("session-1")).await.unwrap();
        assert_eq!(created.subject, "Test task");
        assert_eq!(created.description, "Test description");
        assert_eq!(created.priority, TaskPriority::High);
        assert_eq!(created.labels, vec!["test".to_string()]);
        assert_eq!(created.status, TaskStatus::Pending);
        assert_eq!(created.created_by_session, Some("session-1".to_string()));

        let fetched = store.get(&created.id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, created.id);
    }

    #[tokio::test]
    async fn test_memory_store_list() {
        let store = MemoryTaskStore::new();

        let task1 = NewTask {
            subject: "Task 1".to_string(),
            description: "Desc 1".to_string(),
            priority: None,
            labels: None,
            blocks: None,
            ..Default::default()
        };
        let task2 = NewTask {
            subject: "Task 2".to_string(),
            description: "Desc 2".to_string(),
            priority: None,
            labels: None,
            blocks: None,
            ..Default::default()
        };

        store.create(task1, None).await.unwrap();
        store.create(task2, None).await.unwrap();

        let tasks = store.list().await.unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[tokio::test]
    async fn test_memory_store_update() {
        let store = MemoryTaskStore::new();

        let new_task = NewTask {
            subject: "Original".to_string(),
            description: "Original desc".to_string(),
            priority: None,
            labels: None,
            blocks: None,
            ..Default::default()
        };

        let created = store.create(new_task, None).await.unwrap();

        let update = TaskUpdate {
            subject: Some("Updated".to_string()),
            status: Some(TaskStatus::Completed),
            ..Default::default()
        };

        let updated = store
            .update(&created.id, update, Some("session-2"))
            .await
            .unwrap();
        assert_eq!(updated.subject, "Updated");
        assert_eq!(updated.status, TaskStatus::Completed);
        assert_eq!(updated.description, "Original desc"); // unchanged
        assert_eq!(updated.updated_by_session, Some("session-2".to_string()));
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
        assert!(matches!(result, Err(TaskError::NotFound(_))));
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
        assert!(store.get(&created.id).await.unwrap().is_some());

        store.delete(&created.id).await.unwrap();
        assert!(store.get(&created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_store_delete_not_found() {
        let store = MemoryTaskStore::new();

        let result = store.delete(&TaskId::from_string("nonexistent")).await;
        assert!(matches!(result, Err(TaskError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_memory_store_add_remove_blocks() {
        let store = MemoryTaskStore::new();

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

        // Add block
        let updated = store
            .update(
                &task1.id,
                TaskUpdate {
                    add_blocks: Some(vec![task2.id.clone()]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(updated.blocks.len(), 1);
        assert_eq!(updated.blocks[0], task2.id);

        // Remove block
        let updated = store
            .update(
                &task1.id,
                TaskUpdate {
                    remove_blocks: Some(vec![task2.id.clone()]),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(updated.blocks.len(), 0);
    }
}
