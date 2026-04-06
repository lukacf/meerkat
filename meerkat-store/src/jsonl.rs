//! JSONL file-based session store
//!
//! Each session is stored as a single file with the session as JSON.

use crate::error::into_session_store_error;
use crate::{SessionFilter, SessionStore, SessionStoreError, StoreError};
use async_trait::async_trait;
use meerkat_core::{Session, SessionId, SessionMeta};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

#[derive(Default)]
struct JsonlMetadataIndex {
    entries: std::sync::RwLock<std::collections::HashMap<SessionId, SessionMeta>>,
}

impl JsonlMetadataIndex {
    fn read_entries(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, std::collections::HashMap<SessionId, SessionMeta>> {
        self.entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_entries(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, std::collections::HashMap<SessionId, SessionMeta>> {
        self.entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn from_sidecars(metas: Vec<SessionMeta>) -> Self {
        let mut entries = std::collections::HashMap::with_capacity(metas.len());
        for meta in metas {
            entries.insert(meta.id.clone(), meta);
        }
        Self {
            entries: std::sync::RwLock::new(entries),
        }
    }

    fn insert(&self, meta: SessionMeta) {
        let mut entries = self.write_entries();
        entries.insert(meta.id.clone(), meta);
    }

    fn remove(&self, id: &SessionId) {
        let mut entries = self.write_entries();
        entries.remove(id);
    }

    fn sorted_entries(&self) -> Vec<SessionMeta> {
        let entries = self.read_entries();
        let mut metas = entries.values().cloned().collect::<Vec<_>>();
        metas.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
        });
        metas
    }
}

/// File-based session store using JSONL format
pub struct JsonlStore {
    dir: PathBuf,
    /// Whether to use pretty-printed JSON (default: true for readability)
    pretty_print: bool,
    index: RwLock<Option<Arc<JsonlMetadataIndex>>>,
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
                Ok(meta) => {
                    let session_path = self.session_path(&meta.id);
                    match fs::try_exists(&session_path).await {
                        Ok(true) => sessions.push(meta),
                        Ok(false) => {
                            tracing::warn!(
                                session_id = %meta.id,
                                "pruning orphaned JSONL metadata sidecar without a payload file"
                            );
                            if let Err(err) = fs::remove_file(&path).await
                                && err.kind() != std::io::ErrorKind::NotFound
                            {
                                tracing::warn!(
                                    "failed to remove orphaned session metadata sidecar: {path:?}: {err}"
                                );
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                "failed to verify session payload for metadata sidecar: {path:?}: {err}"
                            );
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("failed to parse session metadata sidecar: {path:?}: {err}");
                }
            }
        }

        Ok(sessions)
    }

    async fn open_index(&self) -> Result<Arc<JsonlMetadataIndex>, StoreError> {
        self.init().await?;
        let metas = self.read_all_metadata_sidecars().await?;
        Ok(Arc::new(JsonlMetadataIndex::from_sidecars(metas)))
    }

    async fn index(&self) -> Result<Arc<JsonlMetadataIndex>, StoreError> {
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

    async fn refresh_index_from_sidecars(
        &self,
        index: &Arc<JsonlMetadataIndex>,
    ) -> Result<(), StoreError> {
        let metas = self.read_all_metadata_sidecars().await?;
        let mut seen_ids = std::collections::HashSet::with_capacity(metas.len());

        {
            let mut entries = index.write_entries();
            for meta in metas {
                seen_ids.insert(meta.id.clone());
                entries.insert(meta.id.clone(), meta);
            }
        }

        let cached_ids = {
            let entries = index.read_entries();
            entries.keys().cloned().collect::<Vec<_>>()
        };

        let mut stale_ids = Vec::new();
        for id in cached_ids {
            if seen_ids.contains(&id) {
                continue;
            }
            if !self.session_payload_exists(&id).await? {
                stale_ids.push(id);
            }
        }

        if !stale_ids.is_empty() {
            let mut entries = index.write_entries();
            for id in stale_ids {
                entries.remove(&id);
            }
        }

        Ok(())
    }

    fn session_path(&self, id: &SessionId) -> PathBuf {
        self.dir.join(format!("{}.jsonl", id.0))
    }

    fn metadata_path(&self, id: &SessionId) -> PathBuf {
        self.dir.join(format!("{}.meta", id.0))
    }

    async fn session_payload_exists(&self, id: &SessionId) -> Result<bool, StoreError> {
        fs::try_exists(self.session_path(id))
            .await
            .map_err(StoreError::from)
    }

    async fn prune_orphaned_metadata(
        &self,
        index: &Arc<JsonlMetadataIndex>,
        id: &SessionId,
    ) -> Result<(), StoreError> {
        index.remove(id);
        let meta_path = self.metadata_path(id);
        if let Err(err) = fs::remove_file(&meta_path).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "failed to remove orphaned session metadata sidecar: {meta_path:?}: {err}"
            );
        }
        Ok(())
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
        index.insert(meta);

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

        let session: Session =
            serde_json::from_str(&contents).map_err(StoreError::Serialization)?;

        Ok(Some(session))
    }

    async fn list_impl(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, StoreError> {
        let index = self.index().await?;
        self.refresh_index_from_sidecars(&index).await?;
        let limit = filter.limit.unwrap_or(usize::MAX);
        let mut offset = filter.offset.unwrap_or(0);
        let mut results = Vec::new();

        for meta in index.sorted_entries() {
            if !self.session_payload_exists(&meta.id).await? {
                tracing::warn!(
                    session_id = %meta.id,
                    "pruning orphaned JSONL metadata entry because the payload file is missing"
                );
                self.prune_orphaned_metadata(&index, &meta.id).await?;
                continue;
            }
            if let Some(updated_after) = filter.updated_after
                && meta.updated_at < updated_after
            {
                continue;
            }
            if let Some(created_after) = filter.created_after
                && meta.created_at < created_after
            {
                continue;
            }
            if offset > 0 {
                offset -= 1;
                continue;
            }

            results.push(meta);
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
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
        index.remove(id);

        Ok(())
    }
}

#[async_trait]
impl SessionStore for JsonlStore {
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError> {
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
    async fn test_jsonl_store_list_prunes_cached_orphaned_metadata_sidecar()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        // Create a session
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Hello".to_string())));
        store.save(&session).await?;

        // Delete the main session file but keep the metadata
        let session_path = temp_dir.path().join(format!("{}.jsonl", session.id().0));
        let meta_path = temp_dir.path().join(format!("{}.meta", session.id().0));
        fs::remove_file(&session_path).await?;

        // list() must not advertise a session whose payload has vanished.
        let sessions = store.list(SessionFilter::default()).await?;
        assert!(
            sessions.is_empty(),
            "list should prune orphaned metadata entries instead of reporting ghost sessions"
        );
        assert!(
            !fs::try_exists(&meta_path).await?,
            "orphaned metadata sidecar should be removed during pruning"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_rebuild_prunes_orphaned_metadata_sidecar_on_startup()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store_path = temp_dir.path().to_path_buf();

        let session_id = {
            let store = JsonlStore::new(store_path.clone());
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("Hello".to_string())));
            let session_id = session.id().clone();
            store.save(&session).await?;

            let session_path = temp_dir.path().join(format!("{}.jsonl", session_id.0));
            fs::remove_file(&session_path).await?;
            session_id
        };

        let meta_path = temp_dir.path().join(format!("{}.meta", session_id.0));
        let restarted = JsonlStore::new(store_path);
        let sessions = restarted.list(SessionFilter::default()).await?;
        assert!(
            sessions.is_empty(),
            "a fresh store should ignore orphaned metadata sidecars during index rebuild"
        );
        assert!(
            !fs::try_exists(&meta_path).await?,
            "startup reconciliation should prune the orphaned metadata sidecar"
        );
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

        // A fresh store instance should rebuild its in-memory metadata index
        // directly from the existing `.meta` sidecars.
        let store = JsonlStore::new(store_path);
        let sessions = store.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_list_refreshes_after_external_save()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store_path = temp_dir.path().to_path_buf();
        let store_a = JsonlStore::new(store_path.clone());
        let store_b = JsonlStore::new(store_path);

        // Prime store_a's in-memory cache while the directory is still empty.
        let sessions = store_a.list(SessionFilter::default()).await?;
        assert!(sessions.is_empty());

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text(
            "Hello from store_b".to_string(),
        )));
        let id = session.id().clone();
        store_b.save(&session).await?;

        let sessions = store_a.list(SessionFilter::default()).await?;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        Ok(())
    }

    #[tokio::test]
    async fn test_jsonl_store_does_not_create_redb_index_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let store = JsonlStore::new(temp_dir.path().to_path_buf());

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Hello".to_string())));
        store.save(&session).await?;
        let _ = store.list(SessionFilter::default()).await?;

        let index_path = temp_dir.path().join("session_index.redb");
        assert!(
            !tokio::fs::try_exists(&index_path).await?,
            "jsonl lane should not create a redb index file"
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
}
