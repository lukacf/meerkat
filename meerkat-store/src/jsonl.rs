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

    /// Derive `SessionMeta` for every durable session file.
    ///
    /// The per-session `.jsonl` file is the single durable truth; the metadata
    /// used to rebuild the index projection is derived from it directly, so a
    /// reconcile can never resurrect state that diverges from what `load`
    /// returns. A corrupt session file surfaces as a typed serialization
    /// fault rather than being silently dropped from the listing.
    async fn read_all_session_metas(&self) -> Result<Vec<SessionMeta>, StoreError> {
        let mut entries = fs::read_dir(&self.dir).await?;
        let mut sessions = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let contents = match fs::read_to_string(&path).await {
                Ok(contents) => contents,
                Err(err) => return Err(StoreError::Io(err)),
            };

            match serde_json::from_str::<Session>(&contents) {
                Ok(session) => sessions.push(SessionMeta::from(&session)),
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

        // Always reconcile: rebuild the index projection from the canonical
        // per-session `.jsonl` files. This handles a crash after the session
        // rename but before the index insert — the durable truth is intact and
        // the projection is rematerialized from it.
        let metas = self.read_all_session_metas().await?;
        let session_count = metas.len();

        let index_count = {
            let index = Arc::clone(&index);
            spawn_blocking(move || index.entry_count()).await
        };
        let index_count = match index_count {
            Ok(Ok(count)) => count,
            Ok(Err(err)) => return Err(err),
            Err(err) => return Err(StoreError::Join(err)),
        };

        // Always reconcile on startup when session files exist.
        // This handles:
        // 1. Missing index entries (session_count > index_count)
        // 2. Stale updated_at entries when a session was updated but the index write failed
        // 3. Any other partial-write scenarios where counts match but metadata differs
        if session_count > 0 {
            if session_count != index_count {
                tracing::info!(
                    "Reconciling session index: {} session files vs {} indexed entries",
                    session_count,
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

        // Serialize session as JSON (pretty or compact based on setting)
        let json = if self.pretty_print {
            serde_json::to_string_pretty(session)
        } else {
            serde_json::to_string(session)
        }
        .map_err(StoreError::Serialization)?;

        // Write the session file atomically (write to temp, then rename). This
        // single rename is the durable commit point: the `.jsonl` file is the
        // only durable truth, and both `load` and the index projection derive
        // from it — there is no second durable artifact that could lag it.
        let temp_path = path.with_extension("jsonl.tmp");
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(json.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        fs::rename(&temp_path, &path).await?;

        // Update the index projection. A crash between the rename above and
        // this insert self-heals: `open_index` reconciles the projection from
        // the session files.
        let meta = SessionMeta::from(session);
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
            serde_json::from_str::<Session>(&contents).map_err(StoreError::Serialization)?;

        Ok(Some(session))
    }

    /// List sessions via the SQLite session index.
    ///
    /// The index is a **derived projection**, not canonical state. The single
    /// durable truth is the per-session `.jsonl` file written atomically
    /// (temp + rename + `sync_all`) in [`save_impl`](Self::save_impl).
    /// `open_index` (and therefore the first `index()` call backing this
    /// method) always reconciles the index by re-deriving `SessionMeta` from
    /// the session files, so a crash that lands the session rename but not the
    /// index insert self-heals on next open: the canonical truth is intact and
    /// the projection is rematerialized from it. There is no second durable
    /// metadata artifact that this listing could diverge toward.
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

        // Use async remove_file and handle NotFound instead of sync path.exists()
        if let Err(e) = fs::remove_file(&path).await
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

    #[tokio::test]
    async fn test_load_surfaces_corrupt_session_file_as_serialization_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());
        let session = Session::new();
        let id = session.id().clone();
        store.save(&session).await?;

        fs::write(
            store.session_path(&id),
            b"{ not a serialized Session".as_slice(),
        )
        .await?;

        let error = store
            .load(&id)
            .await
            .expect_err("corrupt persisted Session bytes must fail load");
        assert!(
            matches!(error, SessionStoreError::Serialization(_)),
            "corrupt persisted Session bytes must remain a typed serialization error, got {error:?}"
        );
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

    /// The `.jsonl` session file is the SINGLE durable artifact: no metadata
    /// sidecar exists for `list`/reconcile to diverge toward.
    #[tokio::test]
    async fn test_jsonl_store_writes_no_metadata_sidecar() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Hello".to_string())));

        let id = session.id().clone();
        store.save(&session).await?;

        let meta_path = temp_dir.path().join(format!("{}.meta", id.0));
        assert!(
            !meta_path.exists(),
            "no derived metadata sidecar may be persisted alongside the session file"
        );

        // Listing is served from the index projection derived from the file.
        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].message_count, 1);
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

        // Remove the index file to force an index rebuild from the canonical
        // `.jsonl` session files.
        let index_path = store_path.join("session_index.sqlite3");
        fs::remove_file(&index_path).await?;

        let store = JsonlStore::new(store_path);
        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        Ok(())
    }

    /// A corrupt durable session file must surface as a typed
    /// `SessionStoreError` rather than being silently dropped (laundered) from
    /// the reconciliation set, which would otherwise yield an incomplete
    /// `list()` with no fault.
    #[tokio::test]
    async fn test_jsonl_store_list_surfaces_corrupt_session_file()
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

        // Corrupt the session file so reconciliation can no longer parse it.
        let session_path = store_path.join(format!("{}.jsonl", id.0));
        fs::write(&session_path, b"{ this is not a valid Session json").await?;

        // A fresh store forces index reconciliation, which derives metadata
        // from the session files. The corrupt file must propagate as a typed
        // serialization error instead of an incomplete `Ok(short Vec)`.
        let store = JsonlStore::new(store_path);
        let result = store.list(SessionFilter::default()).await;
        assert!(
            matches!(result, Err(SessionStoreError::Serialization(_))),
            "corrupt session file must surface as SessionStoreError::Serialization, got {result:?}"
        );
        Ok(())
    }

    /// Row #104 gate: the per-session `.jsonl` file is the SINGLE durable
    /// truth, and the SQLite index is a projection reconciled from it. A crash
    /// (or out-of-band write) that lands the session file but not the index
    /// insert cannot split durable truth: a fresh `open_index` re-derives the
    /// listing from the session files, so `list` and `load` agree again.
    #[tokio::test]
    async fn test_jsonl_session_file_is_single_source_of_truth_for_index()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store_path = temp_dir.path().to_path_buf();

        let (id, updated_json) = {
            let store = JsonlStore::new(store_path.clone());
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("Hello".to_string())));
            let id = session.id().clone();
            store.save(&session).await?;

            // Prepare a NEWER durable session state and write it directly to
            // the session file, bypassing the index insert — exactly the state
            // a crash between the session rename and the index insert leaves.
            session.push(Message::User(UserMessage::text("again".to_string())));
            (id, serde_json::to_string_pretty(&session)?)
        };

        let session_path = store_path.join(format!("{}.jsonl", id.0));
        fs::write(&session_path, updated_json).await?;

        // A fresh store must reconcile the listing from the session file: the
        // listing reflects the newer durable truth (2 messages), proving the
        // index cannot serve a stale parallel artifact.
        let store = JsonlStore::new(store_path);
        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(
            sessions[0].message_count, 2,
            "list() must re-derive metadata from the session file, not a stale projection"
        );
        let loaded = store.load(&id).await?.expect("session loads");
        assert_eq!(loaded.messages().len(), 2);
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
