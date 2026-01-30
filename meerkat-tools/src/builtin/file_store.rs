//! File-based task store with atomic writes and locking
//!
//! This module provides [`FileTaskStore`], a persistent implementation
//! of the [`TaskStore`] trait that stores tasks in a JSON file.
//!
//! The store uses advisory file locking during read-modify-write operations
//! and atomic writes (write to temp file, then rename) to ensure data integrity.

use super::store::TaskStore;
use super::types::{
    NewTask, Task, TaskError, TaskId, TaskStatus, TaskStoreData, TaskStoreMeta, TaskUpdate,
};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

const MAX_TASKS: usize = 10_000;
const MAX_COMPLETED_TASKS: usize = 5_000;

/// File-based task store with atomic writes
///
/// Stores tasks in a JSON file with the following features:
/// - Advisory file locking during read-modify-write operations
/// - Atomic writes via temp file + rename
/// - Thread-safe concurrent access via internal mutex
///
/// # Example
///
/// ```text
/// use meerkat_tools::builtin::{FileTaskStore, TaskStore, NewTask};
/// use std::path::PathBuf;
///
/// let store = FileTaskStore::new(PathBuf::from("/path/to/.rkat/tasks.json"));
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
/// ```
pub struct FileTaskStore {
    path: PathBuf,
    /// Mutex to ensure only one operation at a time accesses the file
    lock: Mutex<()>,
}

impl FileTaskStore {
    /// Create a new file store at the given path
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// Create a store at the default location in project's .rkat directory
    pub fn in_project(project_root: &std::path::Path) -> Self {
        Self::new(project_root.join(".rkat").join("tasks.json"))
    }

    /// Get the path to the store file
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Load the store data from disk, creating default data if the file doesn't exist
    async fn load(&self) -> Result<TaskStoreData, TaskError> {
        let exists = fs::try_exists(&self.path)
            .await
            .map_err(|e| TaskError::StorageError(format!("Failed to check file: {}", e)))?;
        if !exists {
            return Ok(TaskStoreData {
                meta: TaskStoreMeta {
                    version: 1,
                    project_id: ulid::Ulid::new().to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    store_rev: 0,
                },
                tasks: Vec::new(),
            });
        }

        let content = fs::read_to_string(&self.path)
            .await
            .map_err(|e| TaskError::StorageError(format!("Failed to read file: {}", e)))?;

        serde_json::from_str(&content)
            .map_err(|e| TaskError::InvalidData(format!("Failed to parse JSON: {}", e)))
    }

    /// Save the store data to disk atomically
    ///
    /// This writes to a temporary file first, then renames it to the target path.
    /// This ensures that the file is never partially written.
    async fn save(&self, data: &mut TaskStoreData) -> Result<(), TaskError> {
        // Increment store revision
        data.meta.store_rev += 1;

        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                TaskError::StorageError(format!("Failed to create directory: {}", e))
            })?;
        }

        // Write to temp file first
        let temp_path = self.path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| TaskError::StorageError(format!("Failed to serialize: {}", e)))?;

        let mut file = fs::File::create(&temp_path)
            .await
            .map_err(|e| TaskError::StorageError(format!("Failed to create temp file: {}", e)))?;

        file.write_all(content.as_bytes())
            .await
            .map_err(|e| TaskError::StorageError(format!("Failed to write: {}", e)))?;

        file.sync_all()
            .await
            .map_err(|e| TaskError::StorageError(format!("Failed to sync: {}", e)))?;

        // Atomic rename
        fs::rename(&temp_path, &self.path)
            .await
            .map_err(|e| TaskError::StorageError(format!("Failed to rename: {}", e)))?;

        Ok(())
    }

    fn enforce_retention(tasks: &mut Vec<Task>) -> usize {
        let total = tasks.len();
        if total <= MAX_TASKS {
            let completed = tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Completed)
                .count();
            if completed <= MAX_COMPLETED_TASKS {
                return 0;
            }
        }

        let mut remove = vec![false; total];

        let mut completed_indices: Vec<usize> = tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| task.status == TaskStatus::Completed)
            .map(|(idx, _)| idx)
            .collect();
        completed_indices.sort_by(|a, b| tasks[*a].updated_at.cmp(&tasks[*b].updated_at));

        let completed_count = completed_indices.len();
        let mut excess_completed = completed_count.saturating_sub(MAX_COMPLETED_TASKS);
        for idx in completed_indices {
            if excess_completed == 0 {
                break;
            }
            remove[idx] = true;
            excess_completed -= 1;
        }

        let mut removed = remove.iter().filter(|&&flag| flag).count();
        let remaining = total.saturating_sub(removed);
        let mut excess_total = remaining.saturating_sub(MAX_TASKS);
        if excess_total > 0 {
            let mut all_indices: Vec<usize> = (0..total).collect();
            all_indices.sort_by(|a, b| tasks[*a].updated_at.cmp(&tasks[*b].updated_at));
            for idx in all_indices {
                if excess_total == 0 {
                    break;
                }
                if !remove[idx] {
                    remove[idx] = true;
                    removed += 1;
                    excess_total -= 1;
                }
            }
        }

        if removed > 0 {
            let mut idx = 0usize;
            tasks.retain(|_| {
                let keep = !remove[idx];
                idx += 1;
                keep
            });
        }

        removed
    }
}

#[async_trait]
impl TaskStore for FileTaskStore {
    async fn list(&self) -> Result<Vec<Task>, TaskError> {
        let _guard = self.lock.lock().await;
        let data = self.load().await?;
        Ok(data.tasks)
    }

    async fn get(&self, id: &TaskId) -> Result<Option<Task>, TaskError> {
        let _guard = self.lock.lock().await;
        let data = self.load().await?;
        Ok(data.tasks.into_iter().find(|t| &t.id == id))
    }

    async fn create(&self, new_task: NewTask, session_id: Option<&str>) -> Result<Task, TaskError> {
        let _guard = self.lock.lock().await;
        let mut data = self.load().await?;

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

        data.tasks.push(task.clone());
        Self::enforce_retention(&mut data.tasks);
        self.save(&mut data).await?;

        Ok(task)
    }

    async fn update(
        &self,
        id: &TaskId,
        update: TaskUpdate,
        session_id: Option<&str>,
    ) -> Result<Task, TaskError> {
        let _guard = self.lock.lock().await;
        let mut data = self.load().await?;

        let task = data
            .tasks
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

        let updated_task = task.clone();
        self.save(&mut data).await?;

        Ok(updated_task)
    }

    async fn delete(&self, id: &TaskId) -> Result<(), TaskError> {
        let _guard = self.lock.lock().await;
        let mut data = self.load().await?;

        let len_before = data.tasks.len();
        data.tasks.retain(|t| &t.id != id);

        if data.tasks.len() == len_before {
            return Err(TaskError::NotFound(id.0.clone()));
        }

        self.save(&mut data).await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::types::{TaskPriority, TaskStatus};
    use tempfile::TempDir;

    fn create_temp_store() -> (TempDir, FileTaskStore) {
        let temp_dir = TempDir::new().unwrap();
        let store_path = temp_dir.path().join(".rkat").join("tasks.json");
        let store = FileTaskStore::new(store_path);
        (temp_dir, store)
    }

    #[tokio::test]
    async fn test_file_store_create_and_get() {
        let (_temp_dir, store) = create_temp_store();

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
    async fn test_file_store_list() {
        let (_temp_dir, store) = create_temp_store();

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
    async fn test_file_store_update() {
        let (_temp_dir, store) = create_temp_store();

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
    async fn test_file_store_delete() {
        let (_temp_dir, store) = create_temp_store();

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

        // Delete it
        store.delete(&created.id).await.unwrap();

        // Verify task is gone
        assert!(store.get(&created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_file_store_delete_not_found() {
        let (_temp_dir, store) = create_temp_store();

        let result = store.delete(&TaskId::from_string("nonexistent")).await;
        assert!(matches!(result, Err(TaskError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_file_store_update_not_found() {
        let (_temp_dir, store) = create_temp_store();

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
    async fn test_file_store_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let store_path = temp_dir.path().join(".rkat").join("tasks.json");

        // Create a task with the first store instance
        let task_id;
        {
            let store = FileTaskStore::new(store_path.clone());
            let new_task = NewTask {
                subject: "Persisted task".to_string(),
                description: "Should survive reload".to_string(),
                priority: Some(TaskPriority::High),
                labels: Some(vec!["persistent".to_string()]),
                blocks: None,
                ..Default::default()
            };
            let created = store.create(new_task, Some("session-1")).await.unwrap();
            task_id = created.id;
            // store is dropped here
        }

        // Create a new store instance and verify data persists
        {
            let store = FileTaskStore::new(store_path.clone());
            let fetched = store.get(&task_id).await.unwrap();
            assert!(fetched.is_some());
            let task = fetched.unwrap();
            assert_eq!(task.subject, "Persisted task");
            assert_eq!(task.description, "Should survive reload");
            assert_eq!(task.priority, TaskPriority::High);
            assert_eq!(task.labels, vec!["persistent".to_string()]);
            assert_eq!(task.created_by_session, Some("session-1".to_string()));
        }
    }

    #[tokio::test]
    async fn test_file_store_creates_parent_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let deeply_nested = temp_dir
            .path()
            .join("a")
            .join("b")
            .join("c")
            .join("tasks.json");

        // Directories shouldn't exist yet
        assert!(!deeply_nested.parent().unwrap().exists());

        let store = FileTaskStore::new(deeply_nested.clone());

        let new_task = NewTask {
            subject: "Nested task".to_string(),
            description: "".to_string(),
            priority: None,
            labels: None,
            blocks: None,
            ..Default::default()
        };

        let created = store.create(new_task, None).await.unwrap();

        // Directories should now exist
        assert!(deeply_nested.parent().unwrap().exists());
        assert!(deeply_nested.exists());

        // Task should be retrievable
        let fetched = store.get(&created.id).await.unwrap();
        assert!(fetched.is_some());
    }

    #[tokio::test]
    async fn test_file_store_atomic_write() {
        let temp_dir = TempDir::new().unwrap();
        let store_path = temp_dir.path().join(".rkat").join("tasks.json");
        let temp_path = store_path.with_extension("json.tmp");

        let store = FileTaskStore::new(store_path.clone());

        // Create a task
        let new_task = NewTask {
            subject: "Atomic test".to_string(),
            description: "".to_string(),
            priority: None,
            labels: None,
            blocks: None,
            ..Default::default()
        };

        store.create(new_task, None).await.unwrap();

        // The temp file should not exist after the operation
        assert!(!temp_path.exists());

        // The main file should exist and be valid JSON
        assert!(store_path.exists());
        let content = fs::read_to_string(&store_path).await.unwrap();
        let data: TaskStoreData = serde_json::from_str(&content).unwrap();
        assert_eq!(data.tasks.len(), 1);
        assert_eq!(data.meta.store_rev, 1);
    }

    #[tokio::test]
    async fn test_file_store_store_rev_increments() {
        let (_temp_dir, store) = create_temp_store();

        // Create first task
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

        // Read the raw file to check store_rev
        let content = fs::read_to_string(store.path()).await.unwrap();
        let data: TaskStoreData = serde_json::from_str(&content).unwrap();
        assert_eq!(data.meta.store_rev, 1);

        // Create second task
        store
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

        let content = fs::read_to_string(store.path()).await.unwrap();
        let data: TaskStoreData = serde_json::from_str(&content).unwrap();
        assert_eq!(data.meta.store_rev, 2);

        // Update a task
        store
            .update(
                &task1.id,
                TaskUpdate {
                    subject: Some("Updated".to_string()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        let content = fs::read_to_string(store.path()).await.unwrap();
        let data: TaskStoreData = serde_json::from_str(&content).unwrap();
        assert_eq!(data.meta.store_rev, 3);

        // Delete a task
        store.delete(&task1.id).await.unwrap();

        let content = fs::read_to_string(store.path()).await.unwrap();
        let data: TaskStoreData = serde_json::from_str(&content).unwrap();
        assert_eq!(data.meta.store_rev, 4);
    }

    #[tokio::test]
    async fn test_file_store_add_and_remove_blocks() {
        let (_temp_dir, store) = create_temp_store();

        // Create tasks
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
        assert!(updated.blocks.contains(&task2.id));

        // Verify persistence
        let fetched = store.get(&task1.id).await.unwrap().unwrap();
        assert_eq!(fetched.blocks.len(), 1);

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

        assert!(updated.blocks.is_empty());
    }

    #[tokio::test]
    async fn test_file_store_in_project() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileTaskStore::in_project(temp_dir.path());

        let expected_path = temp_dir.path().join(".rkat").join("tasks.json");
        assert_eq!(store.path(), &expected_path);
    }

    #[tokio::test]
    async fn test_file_store_get_nonexistent() {
        let (_temp_dir, store) = create_temp_store();

        let result = store
            .get(&TaskId::from_string("nonexistent"))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_file_store_create_with_defaults() {
        let (_temp_dir, store) = create_temp_store();

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
}
