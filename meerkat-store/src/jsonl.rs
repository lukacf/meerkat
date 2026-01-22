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
}

impl JsonlStore {
    /// Create a new JSONL store in the given directory
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Ensure storage directory exists
    pub async fn init(&self) -> Result<(), StoreError> {
        fs::create_dir_all(&self.dir).await?;
        Ok(())
    }

    fn session_path(&self, id: &SessionId) -> PathBuf {
        self.dir.join(format!("{}.jsonl", id.0))
    }
}

#[async_trait]
impl SessionStore for JsonlStore {
    async fn save(&self, session: &Session) -> Result<(), StoreError> {
        // Ensure directory exists
        self.init().await?;

        let path = self.session_path(session.id());

        // Serialize session as JSON
        let json = serde_json::to_string_pretty(session)
            .map_err(|e| StoreError::Serialization(e.to_string()))?;

        // Write atomically (write to temp, then rename)
        let temp_path = path.with_extension("jsonl.tmp");
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(json.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        fs::rename(&temp_path, &path).await?;

        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let path = self.session_path(id);

        if !path.exists() {
            return Ok(None);
        }

        let mut file = fs::File::open(&path).await?;
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
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            // Load session to get metadata
            if let Ok(mut file) = fs::File::open(&path).await {
                let mut contents = String::new();
                if file.read_to_string(&mut contents).await.is_ok() {
                    if let Ok(session) = serde_json::from_str::<Session>(&contents) {
                        let meta = SessionMeta::from(&session);

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

        if path.exists() {
            fs::remove_file(&path).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{Message, UserMessage};

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
        let sessions = store.list(SessionFilter { limit: Some(3), ..Default::default() }).await.unwrap();
        assert_eq!(sessions.len(), 3);

        // Test offset
        let sessions = store.list(SessionFilter { offset: Some(2), ..Default::default() }).await.unwrap();
        assert_eq!(sessions.len(), 3);
    }
}
