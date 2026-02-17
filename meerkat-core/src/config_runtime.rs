//! Realm-scoped config runtime with generation CAS semantics.
//!
//! This wraps a [`ConfigStore`] and maintains a monotonic generation counter in
//! a sidecar state file. Writes support expected-generation checks so clients
//! can do optimistic concurrency control across surfaces.

use crate::config::{Config, ConfigDelta, ConfigError};
use crate::config_store::{ConfigResolvedPaths, ConfigStore, ConfigStoreMetadata};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uuid::Uuid;

const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(20);
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Snapshot returned by config runtime operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub config: Config,
    pub generation: u64,
    pub metadata: Option<ConfigStoreMetadata>,
}

/// Wire envelope returned by config APIs across surfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEnvelope {
    pub config: Config,
    pub generation: u64,
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_paths: Option<ConfigResolvedPaths>,
}

/// Policy for exposing diagnostic filesystem paths in config envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigEnvelopePolicy {
    /// Public shape: omit resolved filesystem paths.
    Public,
    /// Diagnostic shape: include resolved filesystem paths when available.
    Diagnostic,
}

impl ConfigEnvelope {
    pub fn from_snapshot(snapshot: ConfigSnapshot, policy: ConfigEnvelopePolicy) -> Self {
        let metadata = snapshot.metadata;
        let resolved_paths = match policy {
            ConfigEnvelopePolicy::Public => None,
            ConfigEnvelopePolicy::Diagnostic => {
                metadata.as_ref().and_then(|m| m.resolved_paths.clone())
            }
        };
        Self {
            config: snapshot.config,
            generation: snapshot.generation,
            realm_id: metadata.as_ref().and_then(|m| m.realm_id.clone()),
            instance_id: metadata.as_ref().and_then(|m| m.instance_id.clone()),
            backend: metadata.as_ref().and_then(|m| m.backend.clone()),
            resolved_paths,
        }
    }
}

impl From<ConfigSnapshot> for ConfigEnvelope {
    fn from(snapshot: ConfigSnapshot) -> Self {
        Self::from_snapshot(snapshot, ConfigEnvelopePolicy::Diagnostic)
    }
}

/// Errors returned by [`ConfigRuntime`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigRuntimeError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("generation conflict: expected {expected}, current {current}")]
    GenerationConflict { expected: u64, current: u64 },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("timed out acquiring config lock")]
    LockTimeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeState {
    generation: u64,
}

/// File-backed config runtime with monotonic generation CAS.
pub struct ConfigRuntime {
    store: Arc<dyn ConfigStore>,
    state_path: PathBuf,
    lock_path: PathBuf,
    process_lock: Mutex<()>,
}

impl ConfigRuntime {
    /// Create a config runtime using an explicit state file path.
    pub fn new(store: Arc<dyn ConfigStore>, state_path: PathBuf) -> Self {
        let lock_path = state_path.with_extension("lock");
        Self {
            store,
            state_path,
            lock_path,
            process_lock: Mutex::new(()),
        }
    }

    /// Construct from store metadata when a resolved realm root is available.
    pub fn from_store_metadata(store: Arc<dyn ConfigStore>) -> Option<Self> {
        let root = store
            .metadata()
            .and_then(|m| m.resolved_paths)
            .map(|p| PathBuf::from(p.root))?;
        Some(Self::new(store, root.join("config_state.json")))
    }

    /// Read current config + generation.
    pub async fn get(&self) -> Result<ConfigSnapshot, ConfigRuntimeError> {
        let _guard = self.process_lock.lock().await;
        let _file_lock = self.acquire_file_lock().await?;
        let config = self.store.get().await?;
        let generation = self.read_generation().await?;
        Ok(ConfigSnapshot {
            config,
            generation,
            metadata: self.store.metadata(),
        })
    }

    /// Replace config with optional generation check.
    pub async fn set(
        &self,
        config: Config,
        expected_generation: Option<u64>,
    ) -> Result<ConfigSnapshot, ConfigRuntimeError> {
        let _guard = self.process_lock.lock().await;
        let _file_lock = self.acquire_file_lock().await?;
        let current = self.read_generation().await?;
        if let Some(expected) = expected_generation
            && expected != current
        {
            return Err(ConfigRuntimeError::GenerationConflict { expected, current });
        }

        self.store.set(config.clone()).await?;
        let next = current.saturating_add(1);
        self.write_generation(next).await?;

        Ok(ConfigSnapshot {
            config,
            generation: next,
            metadata: self.store.metadata(),
        })
    }

    /// Apply JSON merge patch with optional generation check.
    pub async fn patch(
        &self,
        delta: ConfigDelta,
        expected_generation: Option<u64>,
    ) -> Result<ConfigSnapshot, ConfigRuntimeError> {
        let _guard = self.process_lock.lock().await;
        let _file_lock = self.acquire_file_lock().await?;
        let current = self.read_generation().await?;
        if let Some(expected) = expected_generation
            && expected != current
        {
            return Err(ConfigRuntimeError::GenerationConflict { expected, current });
        }

        let updated = self.store.patch(delta).await?;
        let next = current.saturating_add(1);
        self.write_generation(next).await?;

        Ok(ConfigSnapshot {
            config: updated,
            generation: next,
            metadata: self.store.metadata(),
        })
    }

    async fn read_generation(&self) -> Result<u64, ConfigRuntimeError> {
        if !tokio::fs::try_exists(&self.state_path).await? {
            return Ok(0);
        }
        let raw = tokio::fs::read_to_string(&self.state_path).await?;
        let state: RuntimeState = serde_json::from_str(&raw)?;
        Ok(state.generation)
    }

    async fn write_generation(&self, generation: u64) -> Result<(), ConfigRuntimeError> {
        if let Some(parent) = self.state_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let state = RuntimeState { generation };
        let body = serde_json::to_string_pretty(&state)?;
        let parent = self
            .state_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let tmp = parent.join(format!(".config_state.tmp.{}", Uuid::now_v7()));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .await?;
        file.write_all(body.as_bytes()).await?;
        file.sync_all().await?;
        tokio::fs::rename(&tmp, &self.state_path).await?;
        Ok(())
    }

    async fn acquire_file_lock(&self) -> Result<LockGuard, ConfigRuntimeError> {
        LockGuard::acquire(&self.lock_path).await
    }
}

struct LockGuard {
    path: PathBuf,
}

impl LockGuard {
    async fn acquire(path: &Path) -> Result<Self, ConfigRuntimeError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let deadline = tokio::time::Instant::now() + LOCK_TIMEOUT;
        loop {
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .await
            {
                Ok(mut file) => {
                    file.write_all(b"config-runtime-lock").await?;
                    file.sync_all().await?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if LockGuard::is_stale(path).await? {
                        let _ = tokio::fs::remove_file(path).await;
                        continue;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Err(ConfigRuntimeError::LockTimeout);
                    }
                    tokio::time::sleep(LOCK_RETRY_DELAY).await;
                }
                Err(err) => return Err(ConfigRuntimeError::Io(err)),
            }
        }
    }

    async fn is_stale(path: &Path) -> Result<bool, ConfigRuntimeError> {
        match tokio::fs::metadata(path).await {
            Ok(metadata) => {
                let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let age = SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default();
                Ok(age > LOCK_STALE_AFTER)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(ConfigRuntimeError::Io(err)),
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        remove_file_nonblocking_on_drop(self.path.clone());
    }
}

fn remove_file_nonblocking_on_drop(path: PathBuf) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        std::mem::drop(handle.spawn(async move {
            let _ = tokio::fs::remove_file(path).await;
        }));
    } else {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{Config, ConfigDelta, MemoryConfigStore};
    use serde_json::json;

    #[tokio::test]
    async fn generation_conflict_is_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryConfigStore::new(Config::default()));
        let runtime = ConfigRuntime::new(store, temp.path().join("state.json"));

        let baseline = runtime.get().await.unwrap();
        assert_eq!(baseline.generation, 0);

        let mut updated = baseline.config.clone();
        updated.agent.max_tokens_per_turn = 777;
        let after = runtime.set(updated, Some(0)).await.unwrap();
        assert_eq!(after.generation, 1);

        let conflict = runtime
            .patch(
                ConfigDelta(json!({"agent": {"max_tokens_per_turn": 1000}})),
                Some(0),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            conflict,
            ConfigRuntimeError::GenerationConflict {
                expected: 0,
                current: 1
            }
        ));
    }

    #[tokio::test]
    async fn concurrent_writes_with_same_expected_generation_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(Config::default()));
        let runtime_a = Arc::new(ConfigRuntime::new(
            Arc::clone(&store),
            temp.path().join("state.json"),
        ));
        let runtime_b = Arc::new(ConfigRuntime::new(
            Arc::clone(&store),
            temp.path().join("state.json"),
        ));

        let task_a = {
            let runtime = Arc::clone(&runtime_a);
            tokio::spawn(async move {
                runtime
                    .patch(
                        ConfigDelta(json!({"agent": {"max_tokens_per_turn": 111}})),
                        Some(0),
                    )
                    .await
            })
        };
        let task_b = {
            let runtime = Arc::clone(&runtime_b);
            tokio::spawn(async move {
                runtime
                    .patch(
                        ConfigDelta(json!({"agent": {"max_tokens_per_turn": 222}})),
                        Some(0),
                    )
                    .await
            })
        };

        let res_a = task_a.await.unwrap();
        let res_b = task_b.await.unwrap();

        let ok_count = usize::from(res_a.is_ok()) + usize::from(res_b.is_ok());
        let known_failure_count = usize::from(matches!(
            res_a,
            Err(ConfigRuntimeError::GenerationConflict { .. })
                | Err(ConfigRuntimeError::LockTimeout)
        )) + usize::from(matches!(
            res_b,
            Err(ConfigRuntimeError::GenerationConflict { .. })
                | Err(ConfigRuntimeError::LockTimeout)
        ));
        assert!(ok_count <= 1);
        assert_eq!(known_failure_count + ok_count, 2);
    }

    #[test]
    fn config_envelope_policy_controls_resolved_paths_exposure() {
        let snapshot = ConfigSnapshot {
            config: Config::default(),
            generation: 7,
            metadata: Some(ConfigStoreMetadata {
                realm_id: Some("team".to_string()),
                instance_id: Some("instance".to_string()),
                backend: Some("redb".to_string()),
                resolved_paths: Some(ConfigResolvedPaths {
                    root: "/tmp/root".to_string(),
                    manifest_path: "/tmp/root/realm_manifest.json".to_string(),
                    config_path: "/tmp/root/config.toml".to_string(),
                    sessions_redb_path: "/tmp/root/sessions.redb".to_string(),
                    sessions_jsonl_dir: "/tmp/root/sessions_jsonl".to_string(),
                }),
            }),
        };

        let public = ConfigEnvelope::from_snapshot(snapshot.clone(), ConfigEnvelopePolicy::Public);
        assert!(public.resolved_paths.is_none());

        let diagnostic = ConfigEnvelope::from_snapshot(snapshot, ConfigEnvelopePolicy::Diagnostic);
        assert!(diagnostic.resolved_paths.is_some());
    }
}
