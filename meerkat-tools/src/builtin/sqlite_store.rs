//! SQLite-backed task store for persistent, session-scoped tasks.
//!
//! Gated behind the `sqlite` feature. The store creates a `tasks` table
//! in the provided SQLite database (typically the realm's shared db).
//!
//! Tasks are scoped by `session_id` so that resuming a session restores
//! exactly the tasks that were created during that session.

use super::store::TaskStore;
use super::types::{NewTask, Task, TaskError, TaskId, TaskStatus, TaskUpdate};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const CREATE_TASKS_TABLE_SQL: &str = r"
CREATE TABLE IF NOT EXISTS tasks (
    task_id TEXT PRIMARY KEY,
    session_id TEXT,
    task_json BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
)";

const CREATE_TASKS_SESSION_INDEX_SQL: &str = r"
CREATE INDEX IF NOT EXISTS tasks_session_idx
ON tasks(session_id)";

fn migration_0001_tasks_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(CREATE_TASKS_TABLE_SQL)?;
    tx.execute_batch(CREATE_TASKS_SESSION_INDEX_SQL)?;
    Ok(())
}

/// The task store's schema domain in the per-file migration ledger.
const TOOLS_TASKS_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "tools-tasks",
    migrations: &[meerkat_sqlite::Migration {
        version: 1,
        name: "base-schema",
        apply: migration_0001_tasks_schema,
    }],
};

/// Per-operation connection: fence guard lives exactly as long as the
/// connection it admits.
struct TaskConn {
    conn: Connection,
    _guard: meerkat_sqlite::OperationGuard,
}

impl std::ops::Deref for TaskConn {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl std::ops::DerefMut for TaskConn {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

fn open_connection(path: &Path, ensure_schema: bool) -> Result<TaskConn, TaskError> {
    let guard = meerkat_sqlite::OperationGuard::for_database(path).map_err(se)?;
    let mut conn =
        meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::PRIMARY).map_err(se)?;
    if ensure_schema {
        meerkat_sqlite::apply_domain_migrations(&mut conn, &TOOLS_TASKS_DOMAIN).map_err(se)?;
    }
    Ok(TaskConn {
        conn,
        _guard: guard,
    })
}

fn begin_immediate(conn: &mut Connection) -> Result<Transaction<'_>, TaskError> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| TaskError::StorageError(format!("Failed to begin transaction: {e}")))
}

/// Open a connection, ensuring the schema domain is current on first use
/// (tracked by the flag; the ledger fast path makes re-checks cheap anyway).
fn open_cached(path: &Path, schema_ensured: &AtomicBool) -> Result<TaskConn, TaskError> {
    let need_schema = !schema_ensured.load(Ordering::Acquire);
    let conn = open_connection(path, need_schema)?;
    if need_schema {
        schema_ensured.store(true, Ordering::Release);
    }
    Ok(conn)
}

fn se(e: impl std::fmt::Display) -> TaskError {
    TaskError::StorageError(e.to_string())
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// SQLite-backed task store with session scoping.
///
/// Tasks are stored in a `tasks` table in the provided SQLite database.
/// When constructed with a `session_id`, `list` and `get` return only
/// tasks belonging to that session. When constructed without one, all
/// tasks are visible.
pub struct SqliteTaskStore {
    path: PathBuf,
    session_id: Option<String>,
    schema_ensured: Arc<AtomicBool>,
}

impl SqliteTaskStore {
    /// Create a task store backed by the SQLite database at `path`.
    ///
    /// If `session_id` is provided, operations are scoped to that session.
    pub fn new(path: impl Into<PathBuf>, session_id: Option<String>) -> Self {
        Self {
            path: path.into(),
            session_id,
            schema_ensured: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a store scoped to a specific session.
    pub fn for_session(path: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        Self::new(path, Some(session_id.into()))
    }

    /// Create an unscoped store (sees all tasks across sessions).
    pub fn unscoped(path: impl Into<PathBuf>) -> Self {
        Self::new(path, None)
    }

    /// Return the database path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the session scope, if any.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    fn decode_task(bytes: &[u8]) -> Result<Task, TaskError> {
        serde_json::from_slice(bytes)
            .map_err(|e| TaskError::InvalidData(format!("Failed to decode task: {e}")))
    }

    fn encode_task(task: &Task) -> Result<Vec<u8>, TaskError> {
        serde_json::to_vec(task)
            .map_err(|e| TaskError::StorageError(format!("Failed to encode task: {e}")))
    }
}

#[async_trait]
impl TaskStore for SqliteTaskStore {
    async fn list(&self) -> Result<Vec<Task>, TaskError> {
        let path = self.path.clone();
        let session_id = self.session_id.clone();
        let schema = self.schema_ensured.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_cached(&path, &schema)?;
            let mut tasks = Vec::new();
            if let Some(sid) = &session_id {
                let mut stmt = conn
                    .prepare("SELECT task_json FROM tasks WHERE session_id = ?1 ORDER BY created_at_ms ASC")
                    .map_err(se)?;
                let rows = stmt
                    .query_map(params![sid], |row| row.get::<_, Vec<u8>>(0))
                    .map_err(se)?;
                for row in rows {
                    let bytes = row.map_err(se)?;
                    tasks.push(Self::decode_task(&bytes)?);
                }
            } else {
                let mut stmt = conn
                    .prepare("SELECT task_json FROM tasks ORDER BY created_at_ms ASC")
                    .map_err(se)?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, Vec<u8>>(0))
                    .map_err(se)?;
                for row in rows {
                    let bytes = row.map_err(se)?;
                    tasks.push(Self::decode_task(&bytes)?);
                }
            }
            Ok(tasks)
        })
        .await
        .map_err(|e| TaskError::StorageError(format!("Task join: {e}")))?
    }

    async fn get(&self, id: &TaskId) -> Result<Option<Task>, TaskError> {
        let path = self.path.clone();
        let task_id = id.0.clone();
        let session_id = self.session_id.clone();
        let schema = self.schema_ensured.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_cached(&path, &schema)?;
            let bytes: Option<Vec<u8>> = if let Some(sid) = &session_id {
                conn.query_row(
                    "SELECT task_json FROM tasks WHERE task_id = ?1 AND session_id = ?2",
                    params![task_id, sid],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?
            } else {
                conn.query_row(
                    "SELECT task_json FROM tasks WHERE task_id = ?1",
                    params![task_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?
            };
            match bytes {
                Some(b) => Ok(Some(Self::decode_task(&b)?)),
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| TaskError::StorageError(format!("Task join: {e}")))?
    }

    async fn create(&self, new_task: NewTask, session_id: Option<&str>) -> Result<Task, TaskError> {
        let path = self.path.clone();
        let schema = self.schema_ensured.clone();
        // Use the store's session_id for scoping; the parameter session_id
        // is for tracking who created the task (created_by_session).
        let scope_session_id = self.session_id.clone();
        let tracking_session_id = session_id.map(String::from);

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
            created_by_session: tracking_session_id.clone(),
            updated_by_session: tracking_session_id,
            owner: new_task.owner,
            metadata: new_task.metadata.unwrap_or_default(),
            blocked_by: new_task.blocked_by.unwrap_or_default(),
        };

        let task_clone = task.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_cached(&path, &schema)?;
            let tx = begin_immediate(&mut conn)?;
            let json = Self::encode_task(&task_clone)?;
            let now_ms = now_millis();
            tx.execute(
                "INSERT INTO tasks (task_id, session_id, task_json, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![task_clone.id.0, scope_session_id, json, now_ms, now_ms],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
        .map_err(|e| TaskError::StorageError(format!("Task join: {e}")))??;

        Ok(task)
    }

    async fn update(
        &self,
        id: &TaskId,
        update: TaskUpdate,
        session_id: Option<&str>,
    ) -> Result<Task, TaskError> {
        let path = self.path.clone();
        let task_id = id.0.clone();
        let scope_session_id = self.session_id.clone();
        let tracking_session_id = session_id.map(String::from);
        let schema = self.schema_ensured.clone();

        tokio::task::spawn_blocking(move || {
            let mut conn = open_cached(&path, &schema)?;
            let tx = begin_immediate(&mut conn)?;

            // Load existing task
            let bytes: Vec<u8> = if let Some(sid) = &scope_session_id {
                tx.query_row(
                    "SELECT task_json FROM tasks WHERE task_id = ?1 AND session_id = ?2",
                    params![task_id, sid],
                    |row| row.get(0),
                )
            } else {
                tx.query_row(
                    "SELECT task_json FROM tasks WHERE task_id = ?1",
                    params![task_id],
                    |row| row.get(0),
                )
            }
            .optional()
            .map_err(se)?
            .ok_or_else(|| TaskError::NotFound(task_id.clone()))?;

            let mut task: Task = Self::decode_task(&bytes)?;

            // Apply updates
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
            if let Some(owner) = update.owner {
                task.owner = Some(owner);
            }
            if let Some(metadata) = update.metadata {
                for (key, value) in metadata {
                    if value.is_null() {
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
            task.updated_by_session = tracking_session_id;

            let json = Self::encode_task(&task)?;
            let now_ms = now_millis();
            tx.execute(
                "UPDATE tasks SET task_json = ?1, updated_at_ms = ?2 WHERE task_id = ?3",
                params![json, now_ms, task_id],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(task)
        })
        .await
        .map_err(|e| TaskError::StorageError(format!("Task join: {e}")))?
    }

    async fn delete(&self, id: &TaskId) -> Result<(), TaskError> {
        let path = self.path.clone();
        let task_id = id.0.clone();
        let scope_session_id = self.session_id.clone();
        let schema = self.schema_ensured.clone();

        tokio::task::spawn_blocking(move || {
            let mut conn = open_cached(&path, &schema)?;
            let tx = begin_immediate(&mut conn)?;
            let rows = if let Some(sid) = &scope_session_id {
                tx.execute(
                    "DELETE FROM tasks WHERE task_id = ?1 AND session_id = ?2",
                    params![task_id, sid],
                )
            } else {
                tx.execute("DELETE FROM tasks WHERE task_id = ?1", params![task_id])
            }
            .map_err(se)?;
            if rows == 0 {
                return Err(TaskError::NotFound(task_id));
            }
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
        .map_err(|e| TaskError::StorageError(format!("Task join: {e}")))?
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::builtin::types::TaskPriority;
    use tempfile::TempDir;

    fn temp_store(session_id: Option<&str>) -> (TempDir, SqliteTaskStore) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("tasks.db");
        let store = SqliteTaskStore::new(db_path, session_id.map(String::from));
        (dir, store)
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let (_dir, store) = temp_store(Some("session-1"));
        let task = store
            .create(
                NewTask {
                    subject: "Test task".to_string(),
                    description: "desc".to_string(),
                    priority: Some(TaskPriority::High),
                    labels: Some(vec!["test".to_string()]),
                    ..Default::default()
                },
                Some("session-1"),
            )
            .await
            .unwrap();

        assert_eq!(task.subject, "Test task");
        assert_eq!(task.priority, TaskPriority::High);

        let fetched = store.get(&task.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, task.id);
        assert_eq!(fetched.subject, "Test task");
    }

    #[tokio::test]
    async fn test_session_scoping() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("tasks.db");

        // Create tasks in session-1
        let s1 = SqliteTaskStore::for_session(&db_path, "session-1");
        s1.create(
            NewTask {
                subject: "S1 task".to_string(),
                description: "".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

        // Create tasks in session-2
        let s2 = SqliteTaskStore::for_session(&db_path, "session-2");
        s2.create(
            NewTask {
                subject: "S2 task".to_string(),
                description: "".to_string(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();

        // Each session sees only its own tasks
        assert_eq!(s1.list().await.unwrap().len(), 1);
        assert_eq!(s1.list().await.unwrap()[0].subject, "S1 task");
        assert_eq!(s2.list().await.unwrap().len(), 1);
        assert_eq!(s2.list().await.unwrap()[0].subject, "S2 task");

        // Unscoped sees all
        let all = SqliteTaskStore::unscoped(&db_path);
        assert_eq!(all.list().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_persistence_across_reopens() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("tasks.db");

        let task_id;
        {
            let store = SqliteTaskStore::for_session(&db_path, "s1");
            let task = store
                .create(
                    NewTask {
                        subject: "Persistent".to_string(),
                        description: "survives reopen".to_string(),
                        ..Default::default()
                    },
                    None,
                )
                .await
                .unwrap();
            task_id = task.id;
        }

        // Reopen
        let store = SqliteTaskStore::for_session(&db_path, "s1");
        let task = store.get(&task_id).await.unwrap().unwrap();
        assert_eq!(task.subject, "Persistent");
    }

    #[tokio::test]
    async fn test_update_and_delete() {
        let (_dir, store) = temp_store(Some("s1"));
        let task = store
            .create(
                NewTask {
                    subject: "Original".to_string(),
                    description: "".to_string(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();

        let updated = store
            .update(
                &task.id,
                TaskUpdate {
                    subject: Some("Updated".to_string()),
                    status: Some(TaskStatus::Completed),
                    ..Default::default()
                },
                Some("s1"),
            )
            .await
            .unwrap();
        assert_eq!(updated.subject, "Updated");
        assert_eq!(updated.status, TaskStatus::Completed);

        store.delete(&task.id).await.unwrap();
        assert!(store.get(&task.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_not_found() {
        let (_dir, store) = temp_store(Some("s1"));
        let result = store.delete(&TaskId::from_string("nonexistent")).await;
        assert!(matches!(result, Err(TaskError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_update_not_found() {
        let (_dir, store) = temp_store(Some("s1"));
        let result = store
            .update(
                &TaskId::from_string("nonexistent"),
                TaskUpdate {
                    subject: Some("x".to_string()),
                    ..Default::default()
                },
                None,
            )
            .await;
        assert!(matches!(result, Err(TaskError::NotFound(_))));
    }
}
