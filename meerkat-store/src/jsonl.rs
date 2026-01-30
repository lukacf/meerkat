//! JSONL file-based session store
//!
//! Each session is stored as a single file with the session as JSON.

use crate::index::RedbSessionIndex;
use crate::{SessionFilter, SessionStore, StoreError};
use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tokio::task::spawn_blocking;

/// File-based session store using JSONL format
pub struct JsonlStore {
    dir: PathBuf,
    /// Whether to use pretty-printed JSON (default: true for readability)
    pretty_print: bool,
    index: RwLock<Option<Arc<RedbSessionIndex>>>,
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
            index: RwLock::new(None),
        }
    }
}

impl JsonlStore {
    /// Create a new JSONL store in the given directory with default settings
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            pretty_print: true,
            index: RwLock::new(None),
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

    fn index_path(&self) -> PathBuf {
        self.dir.join("session_index.redb")
    }

    async fn read_all_metadata_sidecars(&self) -> Result<Vec<SessionMeta>, StoreError> {
        let mut entries = fs::read_dir(&self.dir).await?;
        let mut sessions = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("meta") {
                continue;
            }

            let contents = match fs::read_to_string(&path).await {
                Ok(contents) => contents,
                Err(err) => {
                    tracing::warn!("failed to read session metadata sidecar: {path:?}: {err}");
                    continue;
                }
            };

            match serde_json::from_str::<SessionMeta>(&contents) {
                Ok(meta) => sessions.push(meta),
                Err(err) => {
                    tracing::warn!("failed to parse session metadata sidecar: {path:?}: {err}");
                }
            }
        }

        Ok(sessions)
    }

    async fn open_index(&self) -> Result<Arc<RedbSessionIndex>, StoreError> {
        self.init().await?;

        let index_path = self.index_path();

        let open_attempt = {
            let index_path = index_path.clone();
            spawn_blocking(move || RedbSessionIndex::open(index_path)).await
        };

        let index = match open_attempt {
            Ok(Ok(index)) => Arc::new(index),
            Ok(Err(open_err)) => {
                tracing::warn!("failed to open session index, attempting rebuild: {open_err}");

                // Best-effort: quarantine the old index file if it exists, then retry.
                let quarantined = {
                    let ts =
                        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                            Ok(duration) => duration.as_secs(),
                            Err(_) => 0,
                        };
                    let filename = match index_path.file_name() {
                        Some(file) => file.to_string_lossy().to_string(),
                        None => "session_index.redb".to_string(),
                    };
                    let quarantine_path =
                        index_path.with_file_name(format!("{filename}.corrupt-{ts}"));
                    match fs::rename(&index_path, &quarantine_path).await {
                        Ok(()) => {
                            tracing::warn!(
                                "quarantined corrupt session index: {index_path:?} -> {quarantine_path:?}"
                            );
                            Ok(())
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(err) => Err(StoreError::Io(err)),
                    }
                };

                quarantined?;

                let retry = {
                    let index_path = index_path.clone();
                    spawn_blocking(move || RedbSessionIndex::open(index_path)).await
                };
                match retry {
                    Ok(Ok(index)) => Arc::new(index),
                    Ok(Err(err)) => return Err(err),
                    Err(err) => return Err(StoreError::Internal(err.to_string())),
                }
            }
            Err(err) => return Err(StoreError::Internal(err.to_string())),
        };

        let is_empty = {
            let index = Arc::clone(&index);
            spawn_blocking(move || index.is_empty()).await
        };
        let is_empty = match is_empty {
            Ok(Ok(is_empty)) => is_empty,
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(StoreError::Internal(err.to_string())),
        };

        if is_empty {
            let metas = self.read_all_metadata_sidecars().await?;
            if !metas.is_empty() {
                let index = Arc::clone(&index);
                let result = spawn_blocking(move || index.insert_many(metas)).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => return Err(err),
                    Err(err) => return Err(StoreError::Internal(err.to_string())),
                }
            }
        }

        Ok(index)
    }

    async fn index(&self) -> Result<Arc<RedbSessionIndex>, StoreError> {
        if let Some(index) = self.index.read().await.as_ref() {
            return Ok(Arc::clone(index));
        }

        let opened = self.open_index().await?;
        let mut guard = self.index.write().await;
        Ok(match guard.as_ref() {
            Some(existing) => Arc::clone(existing),
            None => {
                *guard = Some(Arc::clone(&opened));
                opened
            }
        })
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

        let index = self.index().await?;
        let result = spawn_blocking(move || index.insert_meta(meta)).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(StoreError::Internal(err.to_string())),
        }

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
        let index = self.index().await?;
        let result = spawn_blocking(move || index.list_meta(filter)).await;
        match result {
            Ok(Ok(sessions)) => Ok(sessions),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(StoreError::Internal(err.to_string())),
        }
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

        let index = self.index().await?;
        let id = id.clone();
        let result = spawn_blocking(move || index.remove(&id)).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(StoreError::Internal(err.to_string())),
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{Message, UserMessage};

    /// Test that load() returns None for non-existent sessions without blocking
    /// This verifies we use async file operations, not sync Path::exists()
    #[tokio::test]
    async fn test_load_nonexistent_uses_async_fs() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // This should use async operations internally
        // Previously used sync path.exists() which would block the async runtime
        let id = SessionId::new();
        let result = store.load(&id).await?;
        assert!(result.is_none(), "Non-existent session should return None");
        Ok(())
    }

    /// Test that delete() handles non-existent files gracefully with async ops
    #[tokio::test]
    async fn test_delete_nonexistent_uses_async_fs() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());
        store.init().await?;

        // Deleting a non-existent session should succeed (no-op)
        // Previously used sync path.exists() which would block
        let id = SessionId::new();
        let result = store.delete(&id).await;
        assert!(
            result.is_ok(),
            "Delete of non-existent session should succeed"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_uses_metadata_sidecar() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session with some content
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        let id = session.id().clone();
        store.save(&session).await?;

        // Verify metadata sidecar file was created
        let meta_path = temp_dir.path().join(format!("{}.meta", id.0));
        assert!(
            meta_path.exists(),
            "Metadata sidecar file should exist at {:?}",
            meta_path
        );

        // Verify the metadata file contains valid SessionMeta
        let meta_contents = fs::read_to_string(&meta_path).await?;
        let meta: SessionMeta = serde_json::from_str(&meta_contents)?;
        assert_eq!(meta.id, id);
        assert_eq!(meta.message_count, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_list_reads_only_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));
        store.save(&session).await?;

        // Delete the main session file but keep the metadata
        let session_path = temp_dir.path().join(format!("{}.jsonl", session.id().0));
        fs::remove_file(&session_path).await?;

        // list() should still work because it reads from metadata sidecar
        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, *session.id());
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_list_uses_index_when_metadata_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));
        let id = session.id().clone();
        store.save(&session).await?;

        // If listing relies on scanning `.meta` sidecars, this would now return 0.
        let meta_path = temp_dir.path().join(format!("{}.meta", id.0));
        fs::remove_file(&meta_path).await?;

        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_rebuilds_index_when_missing() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = tempfile::tempdir()?;
        let store_path = temp_dir.path().to_path_buf();

        let id = {
            let store = JsonlStore::new(store_path.clone());
            let mut session = Session::new();
            session.push(Message::User(UserMessage {
                content: "Hello".to_string(),
            }));
            let id = session.id().clone();
            store.save(&session).await?;
            id
        };

        // Remove the index file to force an index rebuild from `.meta` sidecars.
        let index_path = store_path.join("session_index.redb");
        fs::remove_file(&index_path).await?;

        let store = JsonlStore::new(store_path);
        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_compact_format() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::builder(temp_dir.path().to_path_buf())
            .pretty_print(false)
            .build();

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        store.save(&session).await?;

        // Read the file and verify it's not pretty-printed (no newlines in middle)
        let session_path = temp_dir.path().join(format!("{}.jsonl", session.id().0));
        let contents = fs::read_to_string(&session_path).await?;

        // Compact JSON should be a single line
        assert_eq!(
            contents.lines().count(),
            1,
            "Compact JSON should be a single line"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_pretty_format_by_default() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        store.save(&session).await?;

        // Read the file and verify it's pretty-printed (multiple lines)
        let session_path = temp_dir.path().join(format!("{}.jsonl", session.id().0));
        let contents = fs::read_to_string(&session_path).await?;

        // Pretty JSON should have multiple lines
        assert!(
            contents.lines().count() > 1,
            "Pretty JSON should have multiple lines"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        let id = session.id().clone();

        // Save
        store.save(&session).await?;

        // Load
        let loaded = store.load(&id).await?.ok_or("not found")?;
        assert_eq!(loaded.id(), &id);
        assert_eq!(loaded.messages().len(), 1);

        // List
        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);

        // Delete
        store.delete(&id).await?;
        assert!(store.load(&id).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_not_found() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        let id = SessionId::new();
        let result = store.load(&id).await?;
        assert!(result.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_filter() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create multiple sessions
        for i in 0..5 {
            let mut session = Session::new();
            session.push(Message::User(UserMessage {
                content: format!("Message {}", i),
            }));
            store.save(&session).await?;
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Test limit
        let sessions = store
            .list(SessionFilter {
                limit: Some(3),
                ..Default::default()
            })
            .await?;
        assert_eq!(sessions.len(), 3);

        // Test offset
        let sessions = store
            .list(SessionFilter {
                offset: Some(2),
                ..Default::default()
            })
            .await?;
        assert_eq!(sessions.len(), 3);
        Ok(())
    }
}
