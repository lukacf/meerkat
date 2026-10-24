//! Realm-scoped config runtime with generation CAS semantics.
//!
//! This wraps a [`ConfigStore`] and maintains a monotonic generation counter in
//! a sidecar state file. Writes support expected-generation checks so clients
//! can do optimistic concurrency control across surfaces.

use crate::config::{Config, ConfigDelta, ConfigError};
use crate::config_store::{ConfigResolvedPaths, ConfigStore, ConfigStoreMetadata};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

const LOCK_RETRY_DELAY: Duration = Duration::from_millis(20);
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounded re-opens when the file a holder locked is no longer the file at
/// the lock path (removed from outside the convention).
const REOPEN_ATTEMPTS: u32 = 4;

/// Snapshot returned by config runtime operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub config: Config,
    pub generation: u64,
    pub metadata: Option<ConfigStoreMetadata>,
}

/// Wire envelope returned by config APIs across surfaces.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEnvelope {
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
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

/// Why an admitted config write ([`ConfigRuntime::patch_entered`]) did not
/// complete.
#[derive(Debug, thiserror::Error)]
pub enum EnteredConfigWriteError {
    /// Refused or failed before the entry check: nothing was written.
    #[error(transparent)]
    NotWritten(ConfigRuntimeError),
    /// The entry check refused the write: nothing was written.
    #[error(transparent)]
    NotAdmitted(crate::AdministrationEntryRefusal),
    /// The write began after the entry check and failed: the document and
    /// its generation may hold the earlier state or the new one.
    #[error("the config write's outcome is unknown: {0}")]
    OutcomeUnknown(ConfigRuntimeError),
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
    process_lock: Arc<Mutex<()>>,
}

impl ConfigRuntime {
    /// Create a config runtime using an explicit state file path.
    pub fn new(store: Arc<dyn ConfigStore>, state_path: PathBuf) -> Self {
        let lock_path = state_path.with_extension("lock");
        Self {
            store,
            state_path,
            lock_path,
            process_lock: Arc::new(Mutex::new(())),
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
        let _file_lock = ConfigReadLock::acquire(&self.lock_path).await?;
        let config = self.store.get().await?;
        let generation = self.read_generation().await?;
        Ok(ConfigSnapshot {
            config,
            generation,
            metadata: self.store.metadata(),
        })
    }

    /// Replace config with optional generation check.
    ///
    /// A full replace (see [`ConfigStore::set`]): in a realm document every
    /// field becomes an explicit override of the parent realm. Use
    /// [`Self::patch`] to change individual keys.
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

    /// The config [`Self::patch`] would commit for `delta`, without writing
    /// it (see [`ConfigStore::patch_preview`]). Surfaces validate this preview
    /// before committing, so it merges exactly as the store's patch does.
    pub async fn patch_preview(&self, delta: &ConfigDelta) -> Result<Config, ConfigRuntimeError> {
        let _file_lock = ConfigReadLock::acquire(&self.lock_path).await?;
        let (preview, _) = self.store.patch_preview(delta).await?;
        Ok(preview)
    }

    /// Apply JSON merge patch with optional generation check.
    ///
    /// Only the keys the patch names are written (see [`ConfigStore::patch`]).
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

    /// [`Self::patch`], admitted by `entry` at the write's entry: after the
    /// process lock, the document's file lock and the generation check,
    /// immediately before the document is written. A refusal there writes
    /// nothing.
    ///
    /// Once admitted, the write is settled by an owned task that holds both
    /// locks and the entered admission through the document write and the
    /// generation publication, and releases them only then: a caller that
    /// stops waiting neither releases them between the two nor abandons the
    /// settlement. Any failure after the admission is
    /// [`EnteredConfigWriteError::OutcomeUnknown`].
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn patch_entered(
        &self,
        delta: ConfigDelta,
        expected_generation: Option<u64>,
        entry: &dyn crate::AdministrationEntry,
    ) -> Result<ConfigSnapshot, EnteredConfigWriteError> {
        let process_guard = Arc::clone(&self.process_lock).lock_owned().await;
        let file_lock = self
            .acquire_file_lock()
            .await
            .map_err(EnteredConfigWriteError::NotWritten)?;
        let current = self
            .read_generation()
            .await
            .map_err(EnteredConfigWriteError::NotWritten)?;
        if let Some(expected) = expected_generation
            && expected != current
        {
            return Err(EnteredConfigWriteError::NotWritten(
                ConfigRuntimeError::GenerationConflict { expected, current },
            ));
        }
        let entered = entry
            .enter()
            .map_err(EnteredConfigWriteError::NotAdmitted)?;

        // No await between the admission and handing the custody to its
        // settling owner.
        let store = Arc::clone(&self.store);
        let state_path = self.state_path.clone();
        let settlement = tokio::spawn(async move {
            let _custody = (process_guard, file_lock, entered);
            let updated = store.patch(delta).await?;
            let next = current.saturating_add(1);
            write_generation_at(&state_path, next).await?;
            Ok::<_, ConfigRuntimeError>((updated, next))
        });
        let (config, generation) = settlement
            .await
            .map_err(|_| {
                EnteredConfigWriteError::OutcomeUnknown(ConfigRuntimeError::Io(
                    std::io::Error::other("the admitted config write did not run to its end"),
                ))
            })?
            .map_err(EnteredConfigWriteError::OutcomeUnknown)?;

        Ok(ConfigSnapshot {
            config,
            generation,
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
        write_generation_at(&self.state_path, generation).await
    }

    async fn acquire_file_lock(&self) -> Result<ConfigWriterLock, ConfigRuntimeError> {
        ConfigWriterLock::acquire(&self.lock_path).await
    }
}

/// Publish `generation` to the sidecar at `state_path` (a temporary file
/// renamed over it).
async fn write_generation_at(state_path: &Path, generation: u64) -> Result<(), ConfigRuntimeError> {
    if let Some(parent) = state_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let state = RuntimeState { generation };
    let body = serde_json::to_string_pretty(&state)?;
    let parent = state_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let tmp = parent.join(format!(
        ".config_state.tmp.{}",
        crate::time_compat::new_uuid_v7()
    ));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .await?;
    file.write_all(body.as_bytes()).await?;
    file.sync_all().await?;
    tokio::fs::rename(&tmp, state_path).await?;
    Ok(())
}

/// Custody of a config document: an OS advisory lock on the lock file beside
/// its generation sidecar. Writers hold it exclusively ([`ConfigWriterLock`])
/// across the document mutation and the generation publication; readers hold
/// it shared ([`ConfigReadLock`]), so a read excludes writers but not other
/// reads.
///
/// The lock file is created once and never unlinked, so every holder in every
/// process locks the same file. The convention requires every holder to
/// cooperate: none unlinks or replaces the file. Binaries using the earlier
/// create-new/unlink convention are incompatible with it and must not share a
/// config document with this one. After locking, a holder runs a post-lock
/// identity check that the file it locked is the one at the path, and
/// re-opens otherwise: the check sees only a pathname replacement that
/// happened before it ran, not an unlink or replacement after custody was
/// returned. The comparison is the physical
/// identity on Unix only; elsewhere it is unqualified (see `same_file`). The
/// OS releases a lock when its file closes, including when the holder's
/// process dies, so an abandoned lock needs no recovery and a lock file nobody
/// holds is no obstacle.
///
/// A holder releases its custody explicitly before its file closes
/// ([`release_custody`]): the lock belongs to the open file description, and
/// a process spawned meanwhile holds a copy of the holder's descriptor until
/// it executes its program, which would keep a custody released by the close
/// alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Custody {
    Shared,
    Exclusive,
}

/// Take `custody` of the lock file at `path` without waiting: `None` while it
/// is held incompatibly.
fn try_custody(path: &Path, custody: Custody) -> std::io::Result<Option<std::fs::File>> {
    for _ in 0..REOPEN_ATTEMPTS {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let locked = match custody {
            Custody::Shared => file.try_lock_shared(),
            Custody::Exclusive => file.try_lock(),
        };
        match locked {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        match locks_path(&file, path) {
            Ok(true) => return Ok(Some(file)),
            // Another file is at the path now: release this one, open again.
            Ok(false) => release_custody(&file),
            Err(error) => {
                release_custody(&file);
                return Err(error);
            }
        }
    }
    Ok(None)
}

/// Release the custody held through `file`, before it closes. The lock
/// belongs to the open file description, which every copy of the descriptor
/// shares; unlocking releases it for all of them, so a copy a process spawned
/// meanwhile still holds keeps nothing. Every custody opened its own
/// description ([`try_custody`]), so this releases no other holder's custody.
fn release_custody(file: &std::fs::File) {
    let _ = file.unlock();
}

/// Take `custody` of the lock file at `path`, waiting up to `timeout` for an
/// incompatible holder.
async fn acquire_custody(
    path: &Path,
    custody: Custody,
    timeout: Duration,
) -> Result<std::fs::File, ConfigRuntimeError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(file) = try_custody(path, custody)? {
            return Ok(file);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ConfigRuntimeError::LockTimeout);
        }
        tokio::time::sleep(LOCK_RETRY_DELAY).await;
    }
}

/// Whether `file` is still the file at `path`.
fn locks_path(file: &std::fs::File, path: &Path) -> std::io::Result<bool> {
    let held = file.metadata()?;
    match std::fs::metadata(path) {
        Ok(current) => Ok(same_file(&held, &current)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// A config document's exclusive (writer) custody; see `Custody`. Every
/// `ConfigRuntime` writer holds it across the document mutation and the
/// generation publication.
pub struct ConfigWriterLock {
    path: PathBuf,
    _file: std::fs::File,
}

impl std::fmt::Debug for ConfigWriterLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigWriterLock")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Drop for ConfigWriterLock {
    fn drop(&mut self) {
        release_custody(&self._file);
    }
}

impl ConfigWriterLock {
    /// Take the exclusive custody at `path`, waiting up to the lock timeout
    /// for current holders.
    pub async fn acquire(path: &Path) -> Result<Self, ConfigRuntimeError> {
        Self::acquire_within(path, LOCK_TIMEOUT).await
    }

    async fn acquire_within(path: &Path, timeout: Duration) -> Result<Self, ConfigRuntimeError> {
        let file = acquire_custody(path, Custody::Exclusive, timeout).await?;
        Ok(Self {
            path: path.to_path_buf(),
            _file: file,
        })
    }

    /// Take the exclusive custody at `path` now; `None` while any reader or
    /// writer holds it.
    pub fn try_hold(path: &Path) -> std::io::Result<Option<Self>> {
        Ok(try_custody(path, Custody::Exclusive)?.map(|file| Self {
            path: path.to_path_buf(),
            _file: file,
        }))
    }
}

/// A config document's shared (reader) custody; see `Custody`. It excludes
/// writers for as long as it is held, and never other readers.
pub struct ConfigReadLock {
    path: PathBuf,
    _file: std::fs::File,
}

impl std::fmt::Debug for ConfigReadLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigReadLock")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Drop for ConfigReadLock {
    fn drop(&mut self) {
        release_custody(&self._file);
    }
}

impl ConfigReadLock {
    /// Take the shared custody at `path`, waiting up to the lock timeout for
    /// a writer.
    pub async fn acquire(path: &Path) -> Result<Self, ConfigRuntimeError> {
        Self::acquire_within(path, LOCK_TIMEOUT).await
    }

    async fn acquire_within(path: &Path, timeout: Duration) -> Result<Self, ConfigRuntimeError> {
        let file = acquire_custody(path, Custody::Shared, timeout).await?;
        Ok(Self {
            path: path.to_path_buf(),
            _file: file,
        })
    }

    /// Take the shared custody at `path` now, never waiting; `None` while a
    /// writer holds it.
    pub fn try_hold(path: &Path) -> std::io::Result<Option<Self>> {
        Ok(try_custody(path, Custody::Shared)?.map(|file| Self {
            path: path.to_path_buf(),
            _file: file,
        }))
    }
}

/// Whether two metadata snapshots describe the same file. On Unix this is
/// the physical identity (device and inode). Elsewhere, including Windows,
/// stable std exposes no file identity, so this compares length and
/// timestamps: there it is an unqualified best effort, and the post-lock
/// identity check that custody is of the file at the lock path holds on Unix
/// only.
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        a.len() == b.len()
            && a.modified().ok() == b.modified().ok()
            && a.created().ok() == b.created().ok()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{Config, ConfigDelta, MemoryConfigStore};
    use serde_json::json;

    /// A full replace taken before the trusted host administered the
    /// governance section in the document loses to the committed section,
    /// even when its generation check passes (the host's own edit does not
    /// move the generation), and moves nothing.
    #[tokio::test]
    async fn a_stale_full_replace_loses_to_the_hosts_governance_change() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let section = "[[governance.review.rules]]\n\
target = { kind = \"mcp_server\", server = \"mail\" }\n\
tier = \"r2\"\n";
        tokio::fs::write(&path, section).await.unwrap();
        let store = Arc::new(crate::config_store::FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        ));
        let runtime = ConfigRuntime::new(store, temp.path().join("state.json"));
        let snapshot = runtime.get().await.unwrap();

        let administered = section.replace(
            "tier = \"r2\"",
            "tier = \"r3\"\nconsent_assurance = \"person_authenticated\"",
        );
        tokio::fs::write(&path, &administered).await.unwrap();
        let mut stale = snapshot.config.clone();
        stale.max_tokens = Some(512);
        let refused = runtime
            .set(stale, Some(snapshot.generation))
            .await
            .unwrap_err();
        assert!(matches!(
            refused,
            ConfigRuntimeError::Config(ConfigError::GovernanceAdministration)
        ));
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            administered
        );
        let current = runtime.get().await.unwrap();
        assert_eq!(current.generation, snapshot.generation);
        assert_eq!(
            current.config.governance.expect("stated").review.rules[0].tier,
            crate::StockReviewTier::R3
        );
    }

    #[tokio::test]
    async fn generation_conflict_is_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryConfigStore::new(
            Config::default(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        ));
        let runtime = ConfigRuntime::new(store, temp.path().join("state.json"));

        let baseline = runtime.get().await.unwrap();
        assert_eq!(baseline.generation, 0);

        let mut updated = baseline.config.clone();
        updated.agent.max_tokens_per_turn = Some(777);
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
        let store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
            Config::default(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        ));
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
            Err(ConfigRuntimeError::GenerationConflict { .. } | ConfigRuntimeError::LockTimeout)
        )) + usize::from(matches!(
            res_b,
            Err(ConfigRuntimeError::GenerationConflict { .. } | ConfigRuntimeError::LockTimeout)
        ));
        assert!(ok_count <= 1);
        assert_eq!(known_failure_count + ok_count, 2);
    }

    #[tokio::test]
    async fn a_writer_excludes_writers_and_readers_and_never_unlinks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config_state.lock");
        let held = ConfigWriterLock::acquire(&path).await.unwrap();
        let waiter = ConfigWriterLock::acquire_within(&path, Duration::from_millis(200)).await;
        assert!(matches!(waiter, Err(ConfigRuntimeError::LockTimeout)));
        assert!(ConfigWriterLock::try_hold(&path).unwrap().is_none());
        assert!(ConfigReadLock::try_hold(&path).unwrap().is_none());
        let reader = ConfigReadLock::acquire_within(&path, Duration::from_millis(200)).await;
        assert!(matches!(reader, Err(ConfigRuntimeError::LockTimeout)));
        drop(held);
        assert!(path.exists(), "the lock file keeps its identity");
        ConfigWriterLock::try_hold(&path)
            .unwrap()
            .expect("released custody is free");
    }

    #[tokio::test]
    async fn readers_share_custody_and_exclude_writers() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config_state.lock");
        let first = ConfigReadLock::acquire(&path).await.unwrap();
        let second = ConfigReadLock::try_hold(&path)
            .unwrap()
            .expect("a reader never excludes another reader");
        assert!(ConfigWriterLock::try_hold(&path).unwrap().is_none());
        drop(first);
        assert!(ConfigWriterLock::try_hold(&path).unwrap().is_none());
        drop(second);
        ConfigWriterLock::try_hold(&path)
            .unwrap()
            .expect("free once the last reader released");
    }

    /// Custody belongs to the open file description, which every copy of the
    /// holder's descriptor shares: one this process holds, and the one a
    /// process spawned meanwhile holds until it executes its program.
    /// Releasing custody unlocks it, so a released custody is free while such
    /// copies are still open, for writers and readers alike.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_released_custody_is_free_while_a_copy_of_its_descriptor_is_open() {
        use crate::process_isolation_tests::DescriptorCopy;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config_state.lock");

        let writer = ConfigWriterLock::acquire(&path).await.unwrap();
        let copy = writer._file.try_clone().unwrap();
        let spawned = DescriptorCopy::spawn(&writer._file);
        drop(writer);
        let writer = ConfigWriterLock::try_hold(&path)
            .unwrap()
            .expect("a released writer custody is free while copies of its descriptor are open");
        drop(writer);
        drop((copy, spawned));

        let reader = ConfigReadLock::acquire(&path).await.unwrap();
        let copy = reader._file.try_clone().unwrap();
        let spawned = DescriptorCopy::spawn(&reader._file);
        drop(reader);
        ConfigWriterLock::try_hold(&path)
            .unwrap()
            .expect("a released reader custody is free while copies of its descriptor are open");
        drop((copy, spawned));
    }

    #[tokio::test]
    async fn an_unheld_lock_file_is_no_obstacle() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config_state.lock");
        // A file nobody holds the OS lock on (its holder exited): custody is
        // free at once, with no age rule.
        std::fs::write(&path, b"left behind").unwrap();
        ConfigWriterLock::try_hold(&path)
            .unwrap()
            .expect("an unheld lock file is free");
    }

    #[tokio::test]
    async fn reads_overlap_and_wait_only_for_a_writer() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryConfigStore::new(
            Config::default(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        ));
        let state = temp.path().join("state.json");
        let runtime = ConfigRuntime::new(store, state.clone());
        let lock = state.with_extension("lock");

        let reader = ConfigReadLock::acquire(&lock).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), runtime.get())
            .await
            .expect("a read overlaps a held read")
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(2),
            runtime.patch_preview(&ConfigDelta(json!({}))),
        )
        .await
        .expect("a preview is a read")
        .unwrap();
        drop(reader);

        let writer = ConfigWriterLock::acquire(&lock).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(200), runtime.get())
                .await
                .is_err(),
            "a read waits for a held writer"
        );
        drop(writer);
        runtime
            .get()
            .await
            .expect("the read proceeds once released");
    }

    #[test]
    fn config_envelope_policy_controls_resolved_paths_exposure() {
        let snapshot = ConfigSnapshot {
            config: Config::default(),
            generation: 7,
            metadata: Some(ConfigStoreMetadata {
                realm_id: Some("team".to_string()),
                instance_id: Some("instance".to_string()),
                backend: Some("sqlite".to_string()),
                resolved_paths: Some(ConfigResolvedPaths {
                    root: "/tmp/root".to_string(),
                    manifest_path: "/tmp/root/realm_manifest.json".to_string(),
                    config_path: "/tmp/root/config.toml".to_string(),
                    sessions_sqlite_path: Some("/tmp/root/sessions.sqlite3".to_string()),
                    sessions_jsonl_dir: "/tmp/root/sessions_jsonl".to_string(),
                }),
            }),
        };

        let public = ConfigEnvelope::from_snapshot(snapshot.clone(), ConfigEnvelopePolicy::Public);
        assert!(public.resolved_paths.is_none());

        let diagnostic = ConfigEnvelope::from_snapshot(snapshot, ConfigEnvelopePolicy::Diagnostic);
        assert!(diagnostic.resolved_paths.is_some());
    }

    /// A file store whose patch publishes the document, reports that, and
    /// then waits for a release before it returns: its writer is held
    /// between the document and the generation.
    #[cfg(not(target_arch = "wasm32"))]
    struct HeldAfterPublish {
        inner: crate::config_store::FileConfigStore,
        published: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait::async_trait]
    impl ConfigStore for HeldAfterPublish {
        async fn get(&self) -> Result<Config, ConfigError> {
            self.inner.get().await
        }

        async fn set(&self, config: Config) -> Result<(), ConfigError> {
            self.inner.set(config).await
        }

        async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
            let patched = self.inner.patch(delta).await?;
            let published = self.published.lock().unwrap().take();
            if let Some(published) = published {
                let _ = published.send(());
            }
            let release = self.release.lock().unwrap().take();
            if let Some(release) = release {
                let _ = release.await;
            }
            Ok(patched)
        }
    }

    /// Admits, and is held while the write keeps its admission.
    #[cfg(not(target_arch = "wasm32"))]
    struct HeldAdmission(Arc<std::sync::atomic::AtomicBool>);

    #[cfg(not(target_arch = "wasm32"))]
    struct ReleasedOnDrop(Arc<std::sync::atomic::AtomicBool>);

    #[cfg(not(target_arch = "wasm32"))]
    impl Drop for ReleasedOnDrop {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    impl crate::AdministrationEntry for HeldAdmission {
        fn enter(&self) -> Result<crate::EnteredAdministration, crate::AdministrationEntryRefusal> {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(crate::EnteredAdministration::holding(ReleasedOnDrop(
                Arc::clone(&self.0),
            )))
        }
    }

    /// An admitted write whose caller stops waiting after the document is
    /// published, before its generation settles, still settles both under
    /// its custody and admission, released only then; the next reader and
    /// writer see the settled document and generation together.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn an_abandoned_admitted_write_settles_its_document_and_generation_together() {
        let temp = tempfile::tempdir().unwrap();
        let lock_path = temp.path().join("config_state.lock");
        let (published, on_published) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let store = Arc::new(HeldAfterPublish {
            inner: crate::config_store::FileConfigStore::new(
                temp.path().join("config.toml"),
                *crate::model_profile::test_catalog::TEST_CATALOG,
            ),
            published: std::sync::Mutex::new(Some(published)),
            release: std::sync::Mutex::new(Some(released)),
        });
        let runtime = Arc::new(ConfigRuntime::new(
            store,
            temp.path().join("config_state.json"),
        ));
        let held = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let caller = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            let entry = HeldAdmission(Arc::clone(&held));
            async move {
                runtime
                    .patch_entered(ConfigDelta(json!({ "max_tokens": 512 })), Some(0), &entry)
                    .await
            }
        });

        // The document is published; the caller stops waiting before the
        // generation settles.
        on_published.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(
            ConfigWriterLock::try_hold(&lock_path).unwrap().is_none(),
            "the abandoned write still holds the document's custody"
        );
        assert!(
            held.load(std::sync::atomic::Ordering::SeqCst),
            "the abandoned write still holds its admission"
        );

        // Released, it settles, and only then lets the custody go.
        let _ = release.send(());
        let custody = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(custody) = ConfigWriterLock::try_hold(&lock_path).unwrap() {
                    return custody;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the settled write releases the custody");
        assert!(!held.load(std::sync::atomic::Ordering::SeqCst));
        drop(custody);

        let settled = runtime.get().await.unwrap();
        assert_eq!(
            settled.generation, 1,
            "the generation settled with the document"
        );
        assert_eq!(settled.config.max_tokens, Some(512));
        let next = runtime
            .patch(ConfigDelta(json!({ "max_tokens": 1024 })), Some(1))
            .await
            .expect("the next writer is admitted at the settled generation");
        assert_eq!(next.generation, 2);
    }
}
