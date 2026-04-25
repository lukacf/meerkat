use async_trait::async_trait;
use meerkat_core::{
    ArtifactError, ArtifactHandle, ArtifactId, ArtifactListFilter, ArtifactRecord, ArtifactStore,
};
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::RwLock;

/// In-memory artifact store for ephemeral and test paths.
pub struct MemoryArtifactStore {
    artifacts: RwLock<HashMap<ArtifactId, ArtifactRecord>>,
}

impl MemoryArtifactStore {
    pub fn new() -> Self {
        Self {
            artifacts: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ArtifactStore for MemoryArtifactStore {
    async fn put(&self, record: ArtifactRecord) -> Result<ArtifactHandle, ArtifactError> {
        record
            .validate_public_metadata()
            .map_err(|err| ArtifactError::WriteFailed(err.to_string()))?;
        let handle = record.handle.clone();
        self.artifacts
            .write()
            .await
            .insert(record.artifact_id.clone(), record);
        Ok(handle)
    }

    async fn get(&self, artifact_id: &ArtifactId) -> Result<ArtifactRecord, ArtifactError> {
        self.artifacts
            .read()
            .await
            .get(artifact_id)
            .cloned()
            .ok_or_else(|| ArtifactError::NotFound(artifact_id.clone()))
    }

    async fn list(&self, filter: ArtifactListFilter) -> Result<Vec<ArtifactRecord>, ArtifactError> {
        let mut records: Vec<_> = self
            .artifacts
            .read()
            .await
            .values()
            .filter(|record| filter.matches(record))
            .cloned()
            .collect();
        records.sort_by(|left, right| left.artifact_id.cmp(&right.artifact_id));
        Ok(records)
    }

    async fn delete(&self, artifact_id: &ArtifactId) -> Result<(), ArtifactError> {
        self.artifacts.write().await.remove(artifact_id);
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct FsArtifactStore {
    root: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FsArtifactStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path_for(&self, artifact_id: &ArtifactId) -> PathBuf {
        self.root.join(format!("{}.json", artifact_id.as_str()))
    }

    async fn ensure_parent_dir(path: &Path) -> Result<(), ArtifactError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| ArtifactError::WriteFailed(err.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl ArtifactStore for FsArtifactStore {
    async fn put(&self, record: ArtifactRecord) -> Result<ArtifactHandle, ArtifactError> {
        record
            .validate_public_metadata()
            .map_err(|err| ArtifactError::WriteFailed(err.to_string()))?;
        let path = self.path_for(&record.artifact_id);
        Self::ensure_parent_dir(&path).await?;
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|err| ArtifactError::WriteFailed(err.to_string()))?;
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|err| ArtifactError::WriteFailed(err.to_string()))?;
        Ok(record.handle.clone())
    }

    async fn get(&self, artifact_id: &ArtifactId) -> Result<ArtifactRecord, ArtifactError> {
        let path = self.path_for(artifact_id);
        let bytes = tokio::fs::read(&path).await.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                ArtifactError::NotFound(artifact_id.clone())
            } else {
                ArtifactError::ReadFailed(err.to_string())
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|err| ArtifactError::ReadFailed(err.to_string()))
    }

    async fn list(&self, filter: ArtifactListFilter) -> Result<Vec<ArtifactRecord>, ArtifactError> {
        let mut records = Vec::new();
        match tokio::fs::read_dir(&self.root).await {
            Ok(mut entries) => {
                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .map_err(|err| ArtifactError::ReadFailed(err.to_string()))?
                {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let bytes = tokio::fs::read(&path)
                        .await
                        .map_err(|err| ArtifactError::ReadFailed(err.to_string()))?;
                    let record: ArtifactRecord = serde_json::from_slice(&bytes)
                        .map_err(|err| ArtifactError::ReadFailed(err.to_string()))?;
                    if filter.matches(&record) {
                        records.push(record);
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(ArtifactError::ReadFailed(err.to_string())),
        }
        records.sort_by(|left, right| left.artifact_id.cmp(&right.artifact_id));
        Ok(records)
    }

    async fn delete(&self, artifact_id: &ArtifactId) -> Result<(), ArtifactError> {
        let path = self.path_for(artifact_id);
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(ArtifactError::WriteFailed(err.to_string())),
        }
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::{ArtifactContentHandle, ArtifactType, BlobId, BlobRef, SurfaceMetadata};
    use std::collections::BTreeMap;

    fn record(id: &str, session_id: Option<&str>) -> ArtifactRecord {
        let mut record = ArtifactRecord::new(
            ArtifactId::new(id).unwrap(),
            ArtifactType::Json,
            "Report".to_string(),
            "application/json".to_string(),
            2,
            Some("sha256:payload".to_string()),
            ArtifactContentHandle::Blob(BlobRef {
                blob_id: BlobId::new(format!("sha256:{id}")),
                media_type: "application/json".to_string(),
            }),
        )
        .unwrap();
        record.owner.session_id = session_id.map(ToString::to_string);
        record.metadata = SurfaceMetadata {
            labels: BTreeMap::from([("client.thread_id".to_string(), "thread-1".to_string())]),
            app_context: None,
        };
        record
    }

    #[tokio::test]
    async fn memory_artifact_store_lists_and_gets_records() {
        let store = MemoryArtifactStore::new();
        store
            .put(record("artifact-2", Some("session-b")))
            .await
            .unwrap();
        store
            .put(record("artifact-1", Some("session-a")))
            .await
            .unwrap();

        let listed = store
            .list(ArtifactListFilter {
                session_id: Some("session-a".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].artifact_id.as_str(), "artifact-1");
        assert_eq!(
            store
                .get(&ArtifactId::new("artifact-1").unwrap())
                .await
                .unwrap()
                .handle
                .artifact_id
                .as_str(),
            "artifact-1"
        );
    }

    #[tokio::test]
    async fn memory_artifact_store_rejects_reserved_metadata_spoofing() {
        let store = MemoryArtifactStore::new();
        let mut spoofed = record("artifact-1", None);
        spoofed.metadata.labels = BTreeMap::from([("meerkat.runtime".into(), "spoof".into())]);

        assert!(matches!(
            store.put(spoofed).await,
            Err(ArtifactError::WriteFailed(_))
        ));
    }

    #[tokio::test]
    async fn memory_artifact_store_missing_get_is_typed_not_found() {
        let store = MemoryArtifactStore::new();

        assert!(matches!(
            store.get(&ArtifactId::new("missing").unwrap()).await,
            Err(ArtifactError::NotFound(_))
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn fs_artifact_store_persists_records_without_path_identity() {
        let temp = tempfile::tempdir().unwrap();
        let store = FsArtifactStore::new(temp.path().join("artifacts"));
        store
            .put(record("artifact-1", Some("session-a")))
            .await
            .unwrap();

        let restarted = FsArtifactStore::new(temp.path().join("artifacts"));
        let restored = restarted
            .get(&ArtifactId::new("artifact-1").unwrap())
            .await
            .unwrap();
        let encoded = serde_json::to_string(&restored).unwrap();

        assert_eq!(restored.artifact_id.as_str(), "artifact-1");
        assert!(!encoded.contains(temp.path().to_string_lossy().as_ref()));
    }
}
