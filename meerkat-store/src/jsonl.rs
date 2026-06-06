//! JSONL file-based session store
//!
//! Each session is stored as a single file with the session as JSON.

use crate::error::into_session_store_error;
use crate::index::SqliteSessionIndex;
use crate::{SessionFilter, SessionStore, SessionStoreError, StoreError};
use async_trait::async_trait;
use fs4::fs_std::FileExt;
use meerkat_core::{Session, SessionId, SessionMeta};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tokio::task::spawn_blocking;

const SESSION_WRITE_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION_WRITE_LOCK_POLL: Duration = Duration::from_millis(10);

struct SessionWriteLock {
    file: std::fs::File,
}

impl Drop for SessionWriteLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// File-based session store using JSONL format
pub struct JsonlStore {
    dir: PathBuf,
    /// Whether to use pretty-printed JSON (default: true for readability)
    pretty_print: bool,
    index: RwLock<Option<Arc<SqliteSessionIndex>>>,
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
        self.dir.join("session_index.sqlite3")
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
                Err(err) => return Err(StoreError::Io(err)),
            };

            match serde_json::from_str::<SessionMeta>(&contents) {
                Ok(meta) => sessions.push(meta),
                Err(err) => return Err(StoreError::Serialization(err)),
            }
        }

        Ok(sessions)
    }

    async fn open_index(&self) -> Result<Arc<SqliteSessionIndex>, StoreError> {
        self.init().await?;

        let index_path = self.index_path();

        let open_attempt = {
            let index_path = index_path.clone();
            spawn_blocking(move || SqliteSessionIndex::open(index_path)).await
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
                        None => "session_index.sqlite3".to_string(),
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
                    spawn_blocking(move || SqliteSessionIndex::open(index_path)).await
                };
                match retry {
                    Ok(Ok(index)) => Arc::new(index),
                    Ok(Err(err)) => return Err(err),
                    Err(err) => return Err(StoreError::Join(err)),
                }
            }
            Err(err) => return Err(StoreError::Join(err)),
        };

        // Always reconcile: rebuild from sidecars if index is missing entries
        // This handles the case where a crash happens after writing session/meta but before index insert
        let metas = self.read_all_metadata_sidecars().await?;
        let sidecar_count = metas.len();

        let index_count = {
            let index = Arc::clone(&index);
            spawn_blocking(move || index.entry_count()).await
        };
        let index_count = match index_count {
            Ok(Ok(count)) => count,
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(StoreError::Join(err)),
        };

        // Always reconcile on startup when sidecars exist.
        // This handles:
        // 1. Missing index entries (sidecar_count > index_count)
        // 2. Stale updated_at entries when session was updated but index write failed
        // 3. Any other partial-write scenarios where counts match but metadata differs
        if sidecar_count > 0 {
            if sidecar_count != index_count {
                tracing::info!(
                    "Reconciling session index: {} sidecars vs {} indexed entries",
                    sidecar_count,
                    index_count
                );
            }
            let index = Arc::clone(&index);
            let result = spawn_blocking(move || index.insert_many(metas)).await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Err(err),
                Err(err) => return Err(StoreError::Join(err)),
            }
        }

        Ok(index)
    }

    async fn index(&self) -> Result<Arc<SqliteSessionIndex>, StoreError> {
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

    fn session_lock_path(&self, id: &SessionId) -> PathBuf {
        self.dir.join(format!("{}.lock", id.0))
    }

    async fn acquire_session_write_lock(
        &self,
        id: &SessionId,
    ) -> Result<SessionWriteLock, StoreError> {
        self.init().await?;
        let path = self.session_lock_path(id);
        let id = id.clone();
        spawn_blocking(move || -> Result<SessionWriteLock, StoreError> {
            // The file path is only a rendezvous point; the kernel lock is the
            // authority, so a stale file left by a crashed process is harmless.
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            let start = Instant::now();

            loop {
                match file.try_lock_exclusive() {
                    Ok(()) => {
                        file.seek(SeekFrom::Start(0))?;
                        file.set_len(0)?;
                        writeln!(file, "pid={}", std::process::id())?;
                        file.sync_all()?;
                        return Ok(SessionWriteLock { file });
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if start.elapsed() >= SESSION_WRITE_LOCK_TIMEOUT {
                            return Err(StoreError::Internal(format!(
                                "timed out acquiring JSONL session write lock for {id}"
                            )));
                        }
                        std::thread::sleep(SESSION_WRITE_LOCK_POLL);
                    }
                    Err(err) => {
                        return Err(StoreError::Internal(format!(
                            "failed to acquire JSONL session write lock for {id}: {err}"
                        )));
                    }
                }
            }
        })
        .await
        .map_err(StoreError::Join)?
    }
}

// Private methods return StoreError (preserves internal ? chains).
// Trait methods convert at the boundary via into_session_store_error().
impl JsonlStore {
    async fn save_impl(&self, session: &Session) -> Result<(), StoreError> {
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
        .map_err(StoreError::Serialization)?;

        // Create metadata for the sidecar file
        let meta = SessionMeta::from(session);
        let meta_json = serde_json::to_string(&meta).map_err(StoreError::Serialization)?;

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
            Err(err) => return Err(StoreError::Join(err)),
        }

        Ok(())
    }

    async fn load_impl(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let path = self.session_path(id);

        // Use async file open and handle NotFound instead of sync path.exists()
        let mut file = match fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let mut contents = String::new();
        file.read_to_string(&mut contents).await?;

        let session =
            meerkat_core::session_migrations::deserialize_session_migrating(contents.as_bytes())
                .map_err(|err| StoreError::Internal(err.to_string()))?;

        Ok(Some(session))
    }

    async fn list_impl(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let index = self.index().await?;
        let result = spawn_blocking(move || index.list_meta(filter)).await;
        match result {
            Ok(Ok(sessions)) => Ok(sessions),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(StoreError::Join(err)),
        }
    }

    async fn delete_impl(&self, id: &SessionId) -> Result<(), StoreError> {
        let path = self.session_path(id);
        let meta_path = self.metadata_path(id);

        // Use async remove_file and handle NotFound instead of sync path.exists()
        if let Err(e) = fs::remove_file(&path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e.into());
        }

        if let Err(e) = fs::remove_file(&meta_path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e.into());
        }

        let index = self.index().await?;
        let id = id.clone();
        let result = spawn_blocking(move || index.remove(&id)).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(StoreError::Join(err)),
        }

        Ok(())
    }
}

#[async_trait]
impl SessionStore for JsonlStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
        // F1 closure (wave-c C-H1): reject shrink-attempts at the trait
        // boundary before the JSONL row is rewritten on disk.
        let _write_lock = self
            .acquire_session_write_lock(session.id())
            .await
            .map_err(into_session_store_error)?;
        let previous = self
            .load_impl(session.id())
            .await
            .map_err(into_session_store_error)?;
        meerkat_core::session_store::append_only_save_guard(session, previous.as_ref())?;
        self.save_impl(session)
            .await
            .map_err(into_session_store_error)
    }

    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        let _write_lock = self
            .acquire_session_write_lock(session.id())
            .await
            .map_err(into_session_store_error)?;
        let previous = self
            .load_impl(session.id())
            .await
            .map_err(into_session_store_error)?;
        meerkat_core::session_store::transcript_rewrite_save_guard(
            session,
            previous.as_ref(),
            commit,
        )?;
        self.save_impl(session)
            .await
            .map_err(into_session_store_error)
    }

    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        let _write_lock = self
            .acquire_session_write_lock(session.id())
            .await
            .map_err(into_session_store_error)?;
        self.save_impl(session)
            .await
            .map_err(into_session_store_error)
    }

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        let _write_lock = self
            .acquire_session_write_lock(session.id())
            .await
            .map_err(into_session_store_error)?;
        let previous = self
            .load_impl(session.id())
            .await
            .map_err(into_session_store_error)?;
        meerkat_core::session_store::authoritative_projection_current_revision_guard(
            session,
            previous.as_ref(),
            expected_current_revision.as_deref(),
        )?;
        self.save_impl(session)
            .await
            .map_err(into_session_store_error)
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        self.load_impl(id).await.map_err(into_session_store_error)
    }

    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError> {
        self.list_impl(filter)
            .await
            .map_err(into_session_store_error)
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError> {
        self.delete_impl(id).await.map_err(into_session_store_error)
    }

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        let _write_lock = self
            .acquire_session_write_lock(id)
            .await
            .map_err(into_session_store_error)?;
        let Some(previous) = self.load_impl(id).await.map_err(into_session_store_error)? else {
            return Ok(false);
        };
        let previous_token = meerkat_core::session_store::session_projection_cas_token(&previous)?;
        if previous_token != expected_current_revision {
            return Ok(false);
        }
        self.delete_impl(id)
            .await
            .map_err(into_session_store_error)?;
        Ok(true)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, TranscriptRewriteReason,
        TranscriptRewriteSelection, UserMessage,
    };

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
        session.push(Message::User(UserMessage::text("Hello".to_string())));

        let id = session.id().clone();
        store.save(&session).await?;

        // Verify metadata sidecar file was created
        let meta_path = temp_dir.path().join(format!("{}.meta", id.0));
        assert!(
            meta_path.exists(),
            "Metadata sidecar file should exist at {meta_path:?}"
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
        session.push(Message::User(UserMessage::text("Hello".to_string())));
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
    async fn test_jsonl_write_lock_allows_stale_lock_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());
        store.init().await?;

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Hello".to_string())));
        let id = session.id().clone();

        fs::write(store.session_lock_path(&id), "pid=stale\n").await?;

        store.save(&session).await?;
        let loaded = store.load(&id).await?.expect("session should be saved");
        assert_eq!(loaded.messages().len(), 1);

        session.push(Message::User(UserMessage::text("again".to_string())));
        store.save(&session).await?;
        let loaded = store.load(&id).await?.expect("session should be updated");
        assert_eq!(loaded.messages().len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_list_uses_index_when_metadata_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Hello".to_string())));
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
            session.push(Message::User(UserMessage::text("Hello".to_string())));
            let id = session.id().clone();
            store.save(&session).await?;
            id
        };

        // Remove the index file to force an index rebuild from `.meta` sidecars.
        let index_path = store_path.join("session_index.sqlite3");
        fs::remove_file(&index_path).await?;

        let store = JsonlStore::new(store_path);
        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        Ok(())
    }

    /// A corrupt `.meta` sidecar must surface as a typed `SessionStoreError`
    /// rather than being silently dropped (laundered) from the reconciliation
    /// set, which would otherwise yield an incomplete `list()` with no fault.
    #[tokio::test]
    async fn test_jsonl_store_list_surfaces_corrupt_metadata_sidecar()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store_path = temp_dir.path().to_path_buf();

        let id = {
            let store = JsonlStore::new(store_path.clone());
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("Hello".to_string())));
            let id = session.id().clone();
            store.save(&session).await?;
            id
        };

        // Corrupt the metadata sidecar so reconciliation can no longer parse it.
        let meta_path = store_path.join(format!("{}.meta", id.0));
        fs::write(&meta_path, b"{ this is not valid SessionMeta json").await?;

        // A fresh store forces index reconciliation, which scans sidecars.
        // The corrupt sidecar must propagate as a typed serialization error
        // instead of an incomplete `Ok(short Vec)`.
        let store = JsonlStore::new(store_path);
        let result = store.list(SessionFilter::default()).await;
        assert!(
            matches!(result, Err(SessionStoreError::Serialization(_))),
            "corrupt .meta must surface as SessionStoreError::Serialization, got {result:?}"
        );
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
        session.push(Message::User(UserMessage::text("Hello".to_string())));

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
        session.push(Message::User(UserMessage::text("Hello".to_string())));

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
        session.push(Message::User(UserMessage::text("Hello".to_string())));

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
            session.push(Message::User(UserMessage::text(format!("Message {i}"))));
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

    #[tokio::test]
    async fn test_jsonl_save_transcript_rewrite_rejects_stale_parent_after_intervening_save()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let first = JsonlStore::new(temp_dir.path().to_path_buf());
        let second = JsonlStore::new(temp_dir.path().to_path_buf());

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "original".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
        first.save(&session).await?;

        let mut stale = first
            .load(session.id())
            .await?
            .expect("session should exist");
        let mut newer = second
            .load(session.id())
            .await?
            .expect("session should exist");
        newer.push(Message::User(UserMessage::text("intervening".to_string())));
        second.save(&newer).await?;

        let commit = stale.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "replacement".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            ))],
            TranscriptRewriteReason::new("compaction"),
            Some("test".to_string()),
            None,
        )?;

        let err = first
            .save_transcript_rewrite(&stale, &commit)
            .await
            .expect_err("stale rewrite must not overwrite newer session state");
        assert!(
            matches!(err, SessionStoreError::TranscriptRevisionConflict { .. }),
            "unexpected error: {err}"
        );

        let saved = first
            .load(session.id())
            .await?
            .expect("session should remain saved");
        assert_eq!(saved.messages().len(), newer.messages().len());
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_authoritative_projection_expected_revision_rejects_stale_writer()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let first = JsonlStore::new(temp_dir.path().to_path_buf());
        let second = JsonlStore::new(temp_dir.path().to_path_buf());

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        first.save(&session).await?;
        let expected_revision = session.transcript_revision()?;

        let mut newer = second.load(session.id()).await?.expect("session exists");
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        second.save(&newer).await?;

        let mut stale_projection = session.clone();
        stale_projection.push(Message::User(UserMessage::text("stale".to_string())));
        let err = first
            .save_authoritative_projection_if_current_revision(
                &stale_projection,
                Some(expected_revision),
            )
            .await
            .expect_err("stale authoritative projection should be rejected");
        assert!(
            matches!(err, SessionStoreError::TranscriptContinuityViolation { .. }),
            "unexpected error: {err}"
        );

        let saved = first
            .load(session.id())
            .await?
            .expect("session should remain saved");
        assert_eq!(saved.messages().len(), newer.messages().len());
        assert_eq!(saved.transcript_revision()?, newer.transcript_revision()?);
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_delete_if_current_revision_only_deletes_matching_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let first = JsonlStore::new(temp_dir.path().to_path_buf());
        let second = JsonlStore::new(temp_dir.path().to_path_buf());

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("base".to_string())));
        first.save(&session).await?;
        let stale_token = meerkat_core::session_store::session_projection_cas_token(&session)?;

        let mut newer = second.load(session.id()).await?.expect("session exists");
        newer.push(Message::User(UserMessage::text("newer".to_string())));
        second.save(&newer).await?;

        assert!(
            !first
                .delete_if_current_revision(session.id(), &stale_token)
                .await?
        );
        assert!(first.load(session.id()).await?.is_some());

        let current_token = meerkat_core::session_store::session_projection_cas_token(&newer)?;
        assert!(
            first
                .delete_if_current_revision(session.id(), &current_token)
                .await?
        );
        assert!(first.load(session.id()).await?.is_none());
        Ok(())
    }
}
