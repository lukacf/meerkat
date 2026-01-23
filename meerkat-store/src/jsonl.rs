//! JSONL file-based session store
//!
//! Each session is stored as a single file with the session as JSON.

use crate::{SessionFilter, SessionStore, StoreError};
use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// File-based session store using JSONL format
pub struct JsonlStore {
    dir: PathBuf,
    /// Whether to use pretty-printed JSON (default: true for readability)
    pretty_print: bool,
}

/// Builder for configuring JsonlStore
pub struct JsonlStoreBuilder {
    dir: PathBuf,
    pretty_print: bool,
}

impl JsonlStoreBuilder {
    /// Create a new builder with default settings
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            pretty_print: true,
        }
    }

    /// Enable or disable pretty-printed JSON output
    ///
    /// Default: true (pretty-printed for human readability)
    /// Set to false for compact output (smaller file size)
    pub fn pretty_print(mut self, enabled: bool) -> Self {
        self.pretty_print = enabled;
        self
    }

    /// Build the JsonlStore
    pub fn build(self) -> JsonlStore {
        JsonlStore {
            dir: self.dir,
            pretty_print: self.pretty_print,
        }
    }
}

impl JsonlStore {
    /// Create a new JSONL store in the given directory with default settings
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            pretty_print: true,
        }
    }

    /// Create a builder for configuring the store
    pub fn builder(dir: PathBuf) -> JsonlStoreBuilder {
        JsonlStoreBuilder::new(dir)
    }

    /// Ensure storage directory exists
    pub async fn init(&self) -> Result<(), StoreError> {
        fs::create_dir_all(&self.dir).await?;
        Ok(())
    }

    fn session_path(&self, id: &SessionId) -> PathBuf {
        self.dir.join(format!("{}.jsonl", id.0))
    }

    fn metadata_path(&self, id: &SessionId) -> PathBuf {
        self.dir.join(format!("{}.meta", id.0))
    }
}

#[async_trait]
impl SessionStore for JsonlStore {
    async fn save(&self, session: &Session) -> Result<(), StoreError> {
        // Ensure directory exists
        self.init().await?;

        let path = self.session_path(session.id());
        let meta_path = self.metadata_path(session.id());

        // Serialize session as JSON (pretty or compact based on setting)
        let json = if self.pretty_print {
            serde_json::to_string_pretty(session)
        } else {
            serde_json::to_string(session)
        }
        .map_err(|e| StoreError::Serialization(e.to_string()))?;

        // Create metadata for the sidecar file
        let meta = SessionMeta::from(session);
        let meta_json =
            serde_json::to_string(&meta).map_err(|e| StoreError::Serialization(e.to_string()))?;

        // Write session atomically (write to temp, then rename)
        let temp_path = path.with_extension("jsonl.tmp");
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(json.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        fs::rename(&temp_path, &path).await?;

        // Write metadata sidecar atomically
        let meta_temp_path = meta_path.with_extension("meta.tmp");
        let mut meta_file = fs::File::create(&meta_temp_path).await?;
        meta_file.write_all(meta_json.as_bytes()).await?;
        meta_file.flush().await?;
        meta_file.sync_all().await?;
        drop(meta_file);

        fs::rename(&meta_temp_path, &meta_path).await?;

        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let path = self.session_path(id);

        // Use async file open and handle NotFound instead of sync path.exists()
        let mut file = match fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let mut contents = String::new();
        file.read_to_string(&mut contents).await?;

        let session: Session = serde_json::from_str(&contents)
            .map_err(|e| StoreError::Serialization(e.to_string()))?;

        Ok(Some(session))
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        self.init().await?;

        let mut entries = fs::read_dir(&self.dir).await?;
        let mut sessions = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            // Read from .meta sidecar files instead of loading full sessions
            if path.extension().and_then(|e| e.to_str()) != Some("meta") {
                continue;
            }

            // Load metadata from sidecar file
            if let Ok(mut file) = fs::File::open(&path).await {
                let mut contents = String::new();
                if file.read_to_string(&mut contents).await.is_ok() {
                    if let Ok(meta) = serde_json::from_str::<SessionMeta>(&contents) {
                        // Apply filters
                        if let Some(created_after) = filter.created_after {
                            if meta.created_at < created_after {
                                continue;
                            }
                        }
                        if let Some(updated_after) = filter.updated_after {
                            if meta.updated_at < updated_after {
                                continue;
                            }
                        }

                        sessions.push(meta);
                    }
                }
            }
        }

        // Sort by updated_at descending
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        // Apply pagination
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(usize::MAX);

        Ok(sessions.into_iter().skip(offset).take(limit).collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let path = self.session_path(id);
        let meta_path = self.metadata_path(id);

        // Use async remove_file and handle NotFound instead of sync path.exists()
        if let Err(e) = fs::remove_file(&path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        }

        if let Err(e) = fs::remove_file(&meta_path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e.into());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{Message, UserMessage};

    /// Test that load() returns None for non-existent sessions without blocking
    /// This verifies we use async file operations, not sync Path::exists()
    #[tokio::test]
    async fn test_load_nonexistent_uses_async_fs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // This should use async operations internally
        // Previously used sync path.exists() which would block the async runtime
        let id = SessionId::new();
        let result = store.load(&id).await.unwrap();
        assert!(result.is_none(), "Non-existent session should return None");
    }

    /// Test that delete() handles non-existent files gracefully with async ops
    #[tokio::test]
    async fn test_delete_nonexistent_uses_async_fs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::new(temp_dir.path().to_path_buf());
        store.init().await.unwrap();

        // Deleting a non-existent session should succeed (no-op)
        // Previously used sync path.exists() which would block
        let id = SessionId::new();
        let result = store.delete(&id).await;
        assert!(
            result.is_ok(),
            "Delete of non-existent session should succeed"
        );
    }

    #[tokio::test]
    async fn test_jsonl_store_uses_metadata_sidecar() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session with some content
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        let id = session.id().clone();
        store.save(&session).await.unwrap();

        // Verify metadata sidecar file was created
        let meta_path = temp_dir.path().join(format!("{}.meta", id.0));
        assert!(
            meta_path.exists(),
            "Metadata sidecar file should exist at {:?}",
            meta_path
        );

        // Verify the metadata file contains valid SessionMeta
        let meta_contents = fs::read_to_string(&meta_path).await.unwrap();
        let meta: SessionMeta = serde_json::from_str(&meta_contents).unwrap();
        assert_eq!(meta.id, id);
        assert_eq!(meta.message_count, 1);
    }

    #[tokio::test]
    async fn test_jsonl_store_list_reads_only_metadata() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));
        store.save(&session).await.unwrap();

        // Delete the main session file but keep the metadata
        let session_path = temp_dir.path().join(format!("{}.jsonl", session.id().0));
        fs::remove_file(&session_path).await.unwrap();

        // list() should still work because it reads from metadata sidecar
        let sessions = store.list(SessionFilter::default()).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, *session.id());
    }

    #[tokio::test]
    async fn test_jsonl_store_compact_format() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::builder(temp_dir.path().to_path_buf())
            .pretty_print(false)
            .build();

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        store.save(&session).await.unwrap();

        // Read the file and verify it's not pretty-printed (no newlines in middle)
        let session_path = temp_dir.path().join(format!("{}.jsonl", session.id().0));
        let contents = fs::read_to_string(&session_path).await.unwrap();

        // Compact JSON should be a single line
        assert_eq!(
            contents.lines().count(),
            1,
            "Compact JSON should be a single line"
        );
    }

    #[tokio::test]
    async fn test_jsonl_store_pretty_format_by_default() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        store.save(&session).await.unwrap();

        // Read the file and verify it's pretty-printed (multiple lines)
        let session_path = temp_dir.path().join(format!("{}.jsonl", session.id().0));
        let contents = fs::read_to_string(&session_path).await.unwrap();

        // Pretty JSON should have multiple lines
        assert!(
            contents.lines().count() > 1,
            "Pretty JSON should have multiple lines"
        );
    }

    #[tokio::test]
    async fn test_jsonl_store_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        let id = session.id().clone();

        // Save
        store.save(&session).await.unwrap();

        // Load
        let loaded = store.load(&id).await.unwrap().unwrap();
        assert_eq!(loaded.id(), &id);
        assert_eq!(loaded.messages().len(), 1);

        // List
        let sessions = store.list(SessionFilter::default()).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);

        // Delete
        store.delete(&id).await.unwrap();
        assert!(store.load(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_jsonl_store_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        let id = SessionId::new();
        let result = store.load(&id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_jsonl_store_filter() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create multiple sessions
        for i in 0..5 {
            let mut session = Session::new();
            session.push(Message::User(UserMessage {
                content: format!("Message {}", i),
            }));
            store.save(&session).await.unwrap();
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Test limit
        let sessions = store
            .list(SessionFilter {
                limit: Some(3),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(sessions.len(), 3);

        // Test offset
        let sessions = store
            .list(SessionFilter {
                offset: Some(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(sessions.len(), 3);
    }
}
