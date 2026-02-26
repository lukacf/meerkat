#[cfg(feature = "jsonl")]
use crate::JsonlStore;
use crate::{RedbSessionStore, SessionStore, StoreError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant};
use uuid::Uuid;

/// Persistent backend for a realm.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealmBackend {
    #[cfg(feature = "jsonl")]
    Jsonl,
    Redb,
}

impl RealmBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            #[cfg(feature = "jsonl")]
            Self::Jsonl => "jsonl",
            Self::Redb => "redb",
        }
    }
}

/// Realm creation origin metadata.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealmOrigin {
    Generated,
    Workspace,
    Explicit,
    #[default]
    LegacyUnknown,
}

impl RealmOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generated => "generated",
            Self::Workspace => "workspace",
            Self::Explicit => "explicit",
            Self::LegacyUnknown => "legacy_unknown",
        }
    }
}

/// Realm-scoped manifest pinned at creation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmManifest {
    pub realm_id: String,
    pub backend: RealmBackend,
    #[serde(default)]
    pub origin: RealmOrigin,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct RealmPaths {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub config_path: PathBuf,
    pub sessions_redb_path: PathBuf,
    pub sessions_jsonl_dir: PathBuf,
}

pub const REALM_LEASE_HEARTBEAT_SECS: u64 = 5;
pub const REALM_LEASE_STALE_TTL_SECS: u64 = 30;
const MANIFEST_LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const MANIFEST_LOCK_RETRY_DELAY: Duration = Duration::from_millis(20);
const MANIFEST_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// One active lease heartbeat record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmLeaseRecord {
    pub realm_id: String,
    pub instance_id: String,
    pub surface: String,
    pub pid: u32,
    pub started_at: u64,
    pub heartbeat_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RealmLeaseStatus {
    pub active: Vec<RealmLeaseRecord>,
    pub stale: Vec<RealmLeaseRecord>,
}

pub struct RealmLeaseGuard {
    instance_id: String,
    lease_path: PathBuf,
    stop_tx: Option<oneshot::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

struct ManifestLockGuard {
    path: PathBuf,
}

impl ManifestLockGuard {
    async fn acquire(path: &Path, realm_id: &str) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(StoreError::Io)?;
        }

        let deadline = Instant::now() + MANIFEST_LOCK_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                return Err(StoreError::RealmManifestLockTimeout {
                    realm_id: realm_id.to_string(),
                });
            }

            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .await
            {
                Ok(mut file) => {
                    file.write_all(b"realm-manifest-lock")
                        .await
                        .map_err(StoreError::Io)?;
                    file.sync_all().await.map_err(StoreError::Io)?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Self::is_stale(path).await? {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                    tokio::time::sleep(MANIFEST_LOCK_RETRY_DELAY).await;
                }
                Err(err) => return Err(StoreError::Io(err)),
            }
        }
    }

    async fn is_stale(path: &Path) -> Result<bool, StoreError> {
        match tokio::fs::metadata(path).await {
            Ok(metadata) => {
                let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let age = SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default();
                Ok(age > MANIFEST_LOCK_STALE_AFTER)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(StoreError::Io(err)),
        }
    }
}

impl Drop for ManifestLockGuard {
    fn drop(&mut self) {
        remove_file_nonblocking_on_drop(self.path.clone());
    }
}

impl RealmLeaseGuard {
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn lease_path(&self) -> &Path {
        &self.lease_path
    }

    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop_tx.take() {
            let _ = stop.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for RealmLeaseGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.stop_tx.take() {
            let _ = stop.send(());
        }
        if let Some(join) = self.join.take() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                std::mem::drop(handle.spawn(async move {
                    let _ = join.await;
                }));
            } else {
                join.abort();
                let _ = std::fs::remove_file(&self.lease_path);
            }
        }
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

fn default_realms_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("meerkat")
        .join("realms")
}

pub fn sanitize_realm_id(realm_id: &str) -> String {
    realm_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub fn realm_paths_in(realms_root: &Path, realm_id: &str) -> RealmPaths {
    let safe = sanitize_realm_id(realm_id);
    let root = realms_root.join(safe);
    RealmPaths {
        manifest_path: root.join("realm_manifest.json"),
        config_path: root.join("config.toml"),
        sessions_redb_path: root.join("sessions.redb"),
        sessions_jsonl_dir: root.join("sessions_jsonl"),
        root,
    }
}

pub fn realm_paths(realm_id: &str) -> RealmPaths {
    realm_paths_in(&default_realms_root(), realm_id)
}

pub fn realm_lease_dir(paths: &RealmPaths) -> PathBuf {
    paths.root.join("leases")
}

fn sanitize_instance_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn write_lease_record(path: &Path, record: &RealmLeaseRecord) -> Result<(), StoreError> {
    let payload = serde_json::to_vec_pretty(record).map_err(StoreError::Serialization)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(StoreError::Io)?;
    let tmp_path = parent.join(format!(".lease.tmp.{}", Uuid::now_v7()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .await
        .map_err(StoreError::Io)?;
    file.write_all(&payload).await.map_err(StoreError::Io)?;
    file.sync_all().await.map_err(StoreError::Io)?;
    drop(file);
    tokio::fs::rename(&tmp_path, path)
        .await
        .map_err(StoreError::Io)?;
    Ok(())
}

pub async fn start_realm_lease(
    realm_id: &str,
    instance_id: Option<&str>,
    surface: &str,
) -> Result<RealmLeaseGuard, StoreError> {
    start_realm_lease_in(&default_realms_root(), realm_id, instance_id, surface).await
}

pub async fn start_realm_lease_in(
    realms_root: &Path,
    realm_id: &str,
    instance_id: Option<&str>,
    surface: &str,
) -> Result<RealmLeaseGuard, StoreError> {
    let paths = realm_paths_in(realms_root, realm_id);
    let lease_dir = realm_lease_dir(&paths);
    tokio::fs::create_dir_all(&lease_dir)
        .await
        .map_err(StoreError::Io)?;

    let pid = std::process::id();
    let resolved_instance = instance_id.map_or_else(|| format!("instance-{}", Uuid::now_v7()), ToOwned::to_owned);
    let lease_name = format!("{}.json", sanitize_instance_id(&resolved_instance));
    let lease_path = lease_dir.join(lease_name);

    let now = now_unix_secs();
    let mut record = RealmLeaseRecord {
        realm_id: realm_id.to_string(),
        instance_id: resolved_instance.clone(),
        surface: surface.to_string(),
        pid,
        started_at: now,
        heartbeat_at: now,
    };
    write_lease_record(&lease_path, &record).await?;

    let (stop_tx, mut stop_rx) = oneshot::channel();
    let heartbeat_path = lease_path.clone();
    let join = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(REALM_LEASE_HEARTBEAT_SECS));
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    break;
                }
                _ = interval.tick() => {
                    record.heartbeat_at = now_unix_secs();
                    let _ = write_lease_record(&heartbeat_path, &record).await;
                }
            }
        }
        let _ = tokio::fs::remove_file(&heartbeat_path).await;
    });

    Ok(RealmLeaseGuard {
        instance_id: resolved_instance,
        lease_path,
        stop_tx: Some(stop_tx),
        join: Some(join),
    })
}

pub async fn inspect_realm_leases(
    realm_id: &str,
    cleanup_stale: bool,
) -> Result<RealmLeaseStatus, StoreError> {
    inspect_realm_leases_in(&default_realms_root(), realm_id, cleanup_stale).await
}

pub async fn inspect_realm_leases_in(
    realms_root: &Path,
    realm_id: &str,
    cleanup_stale: bool,
) -> Result<RealmLeaseStatus, StoreError> {
    let paths = realm_paths_in(realms_root, realm_id);
    let lease_dir = realm_lease_dir(&paths);
    if !tokio::fs::try_exists(&lease_dir)
        .await
        .map_err(StoreError::Io)?
    {
        return Ok(RealmLeaseStatus::default());
    }

    let mut status = RealmLeaseStatus::default();
    let mut entries = tokio::fs::read_dir(&lease_dir)
        .await
        .map_err(StoreError::Io)?;
    let now = now_unix_secs();
    while let Some(entry) = entries.next_entry().await.map_err(StoreError::Io)? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match tokio::fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<RealmLeaseRecord>(&bytes) {
                Ok(record) => {
                    if now.saturating_sub(record.heartbeat_at) <= REALM_LEASE_STALE_TTL_SECS {
                        status.active.push(record);
                    } else {
                        status.stale.push(record);
                        if cleanup_stale {
                            let _ = tokio::fs::remove_file(&path).await;
                        }
                    }
                }
                Err(_) => {
                    if cleanup_stale {
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(StoreError::Io(err)),
        }
    }
    Ok(status)
}

#[derive(Debug, Clone)]
pub struct RealmManifestEntry {
    pub manifest: RealmManifest,
    pub root: PathBuf,
}

pub async fn list_realm_manifests_in(
    realms_root: &Path,
) -> Result<Vec<RealmManifestEntry>, StoreError> {
    if !tokio::fs::try_exists(realms_root)
        .await
        .map_err(StoreError::Io)?
    {
        return Ok(Vec::new());
    }

    let mut manifests = Vec::new();
    let mut entries = tokio::fs::read_dir(realms_root)
        .await
        .map_err(StoreError::Io)?;
    while let Some(entry) = entries.next_entry().await.map_err(StoreError::Io)? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("realm_manifest.json");
        if !tokio::fs::try_exists(&manifest_path)
            .await
            .map_err(StoreError::Io)?
        {
            continue;
        }
        let manifest = read_existing_manifest(&manifest_path).await?;
        manifests.push(RealmManifestEntry {
            manifest,
            root: path,
        });
    }
    manifests.sort_by(|a, b| a.manifest.realm_id.cmp(&b.manifest.realm_id));
    Ok(manifests)
}

pub async fn ensure_realm_manifest(
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<RealmManifest, StoreError> {
    ensure_realm_manifest_in(&default_realms_root(), realm_id, backend_hint, origin_hint).await
}

pub async fn ensure_realm_manifest_in(
    realms_root: &Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<RealmManifest, StoreError> {
    let paths = realm_paths_in(realms_root, realm_id);
    tokio::fs::create_dir_all(&paths.root)
        .await
        .map_err(StoreError::Io)?;

    if tokio::fs::try_exists(&paths.manifest_path)
        .await
        .map_err(StoreError::Io)?
    {
        let existing = read_existing_manifest(&paths.manifest_path).await?;
        validate_backend_hint(&existing, backend_hint)?;
        return Ok(existing);
    }

    let lock_path = paths.root.join(".realm_manifest.lock");
    let _lock = ManifestLockGuard::acquire(&lock_path, realm_id).await?;
    create_or_read_manifest_under_lock(&paths, realm_id, backend_hint, origin_hint).await
}

fn default_backend() -> RealmBackend {
    #[cfg(feature = "jsonl")]
    {
        RealmBackend::Jsonl
    }
    #[cfg(not(feature = "jsonl"))]
    {
        RealmBackend::Redb
    }
}

fn validate_backend_hint(
    manifest: &RealmManifest,
    backend_hint: Option<RealmBackend>,
) -> Result<(), StoreError> {
    if let Some(requested) = backend_hint
        && requested != manifest.backend
    {
        return Err(StoreError::RealmBackendMismatch {
            realm_id: manifest.realm_id.clone(),
            requested: requested.as_str().to_string(),
            existing: manifest.backend.as_str().to_string(),
        });
    }
    Ok(())
}

async fn create_or_read_manifest_under_lock(
    paths: &RealmPaths,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<RealmManifest, StoreError> {
    if tokio::fs::try_exists(&paths.manifest_path)
        .await
        .map_err(StoreError::Io)?
    {
        let existing = read_existing_manifest(&paths.manifest_path).await?;
        validate_backend_hint(&existing, backend_hint)?;
        return Ok(existing);
    }

    let manifest = RealmManifest {
        realm_id: realm_id.to_string(),
        backend: backend_hint.unwrap_or_else(default_backend),
        origin: origin_hint.unwrap_or(RealmOrigin::Explicit),
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string(),
    };

    write_manifest_atomically(paths, &manifest).await?;
    Ok(manifest)
}

async fn write_manifest_atomically(
    paths: &RealmPaths,
    manifest: &RealmManifest,
) -> Result<(), StoreError> {
    let payload = serde_json::to_vec_pretty(manifest).map_err(StoreError::Serialization)?;
    let tmp_name = format!(".realm_manifest.tmp.{}", Uuid::now_v7());
    let tmp_path = paths.root.join(tmp_name);

    let mut tmp = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .await
        .map_err(StoreError::Io)?;
    tmp.write_all(&payload).await.map_err(StoreError::Io)?;
    tmp.sync_all().await.map_err(StoreError::Io)?;
    drop(tmp);

    tokio::fs::rename(&tmp_path, &paths.manifest_path)
        .await
        .map_err(StoreError::Io)?;
    sync_dir(&paths.root).await?;
    Ok(())
}

async fn sync_dir(path: &Path) -> Result<(), StoreError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
        let file = std::fs::File::open(path)?;
        file.sync_all()
    })
    .await
    .map_err(StoreError::Join)?
    .map_err(StoreError::Io)
}

async fn read_existing_manifest(path: &Path) -> Result<RealmManifest, StoreError> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match tokio::fs::read(path).await {
            Ok(bytes) => match serde_json::from_slice::<RealmManifest>(&bytes) {
                Ok(manifest) => return Ok(manifest),
                Err(_err) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
                Err(err) => return Err(StoreError::Serialization(err)),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            Err(err) => return Err(StoreError::Io(err)),
        }
    }
}

pub async fn open_realm_session_store(
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<(RealmManifest, Arc<dyn SessionStore>), StoreError> {
    open_realm_session_store_in(&default_realms_root(), realm_id, backend_hint, origin_hint).await
}

pub async fn open_realm_session_store_in(
    realms_root: &Path,
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<(RealmManifest, Arc<dyn SessionStore>), StoreError> {
    let manifest =
        ensure_realm_manifest_in(realms_root, realm_id, backend_hint, origin_hint).await?;
    let paths = realm_paths_in(realms_root, realm_id);
    let store: Arc<dyn SessionStore> = match manifest.backend {
        #[cfg(feature = "jsonl")]
        RealmBackend::Jsonl => Arc::new(JsonlStore::new(paths.sessions_jsonl_dir)),
        RealmBackend::Redb => {
            if let Some(parent) = paths.sessions_redb_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(StoreError::Io)?;
            }
            let redb_path = paths.sessions_redb_path.clone();
            let redb_store = tokio::task::spawn_blocking(move || RedbSessionStore::open(redb_path))
                .await
                .map_err(StoreError::Join)??;
            Arc::new(redb_store)
        }
    };
    Ok((manifest, store))
}

pub fn fnv1a64_hex(input: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = OFFSET;
    for b in input.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

pub fn derive_workspace_realm_id(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = canonical.to_string_lossy();
    format!("ws-{}", fnv1a64_hex(&key))
}

pub fn generate_realm_id() -> String {
    format!("realm-{}", Uuid::now_v7())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ensure_realm_manifest_concurrent_create_is_consistent() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "race-test";

        let mut handles = Vec::new();
        for idx in 0..32 {
            let root = realms_root.clone();
            handles.push(tokio::spawn(async move {
                let _ = idx;
                let backend_hint = Some(RealmBackend::Redb);
                ensure_realm_manifest_in(
                    &root,
                    realm_id,
                    backend_hint,
                    Some(RealmOrigin::Generated),
                )
                .await
                .unwrap()
            }));
        }

        let mut manifests = Vec::new();
        for handle in handles {
            manifests.push(handle.await.unwrap());
        }

        let first_backend = manifests[0].backend;
        for manifest in manifests {
            assert_eq!(manifest.realm_id, realm_id);
            assert_eq!(manifest.backend, first_backend);
        }

        let on_disk =
            tokio::fs::read_to_string(realm_paths_in(&realms_root, realm_id).manifest_path)
                .await
                .unwrap();
        let parsed: RealmManifest = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed.realm_id, realm_id);
        assert_eq!(parsed.backend, first_backend);
    }

    #[tokio::test]
    async fn backend_hint_conflict_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "backend-conflict";

        let created = ensure_realm_manifest_in(
            &realms_root,
            realm_id,
            Some(RealmBackend::Redb),
            Some(RealmOrigin::Explicit),
        )
        .await
        .unwrap();
        assert_eq!(created.backend, RealmBackend::Redb);

        #[cfg(feature = "jsonl")]
        {
            let err = ensure_realm_manifest_in(
                &realms_root,
                realm_id,
                Some(RealmBackend::Jsonl),
                Some(RealmOrigin::Explicit),
            )
            .await
            .unwrap_err();
            assert!(matches!(err, StoreError::RealmBackendMismatch { .. }));
        }
    }

    #[tokio::test]
    async fn concurrent_conflicting_hints_pin_single_backend() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "conflict-race";

        let mut handles = Vec::new();
        for idx in 0..32 {
            let root = realms_root.clone();
            handles.push(tokio::spawn(async move {
                let backend_hint = if idx % 2 == 0 {
                    Some(RealmBackend::Redb)
                } else {
                    #[cfg(feature = "jsonl")]
                    {
                        Some(RealmBackend::Jsonl)
                    }
                    #[cfg(not(feature = "jsonl"))]
                    {
                        Some(RealmBackend::Redb)
                    }
                };
                ensure_realm_manifest_in(
                    &root,
                    realm_id,
                    backend_hint,
                    Some(RealmOrigin::Generated),
                )
                .await
            }));
        }

        let mut pinned_backend: Option<RealmBackend> = None;
        for handle in handles {
            match handle.await.unwrap() {
                Ok(manifest) => {
                    pinned_backend = Some(manifest.backend);
                }
                Err(StoreError::RealmBackendMismatch { existing, .. }) => {
                    if let Some(pinned) = pinned_backend {
                        assert_eq!(existing, pinned.as_str());
                    }
                }
                Err(err) => panic!("unexpected error: {err}"),
            }
        }

        let manifest = ensure_realm_manifest_in(&realms_root, realm_id, None, None)
            .await
            .unwrap();
        assert_eq!(pinned_backend, Some(manifest.backend));
    }

    #[tokio::test]
    async fn legacy_manifest_without_origin_maps_to_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "legacy-origin";
        let paths = realm_paths_in(&realms_root, realm_id);
        tokio::fs::create_dir_all(&paths.root).await.unwrap();
        let legacy = serde_json::json!({
            "realm_id": realm_id,
            "backend": "redb",
            "created_at": "1234567890"
        });
        tokio::fs::write(
            &paths.manifest_path,
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .await
        .unwrap();

        let manifest = ensure_realm_manifest_in(&realms_root, realm_id, None, None)
            .await
            .unwrap();
        assert_eq!(manifest.origin, RealmOrigin::LegacyUnknown);
    }

    #[tokio::test]
    async fn manifest_is_never_observed_partially_written() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "atomic-visibility";
        let paths = realm_paths_in(&realms_root, realm_id);

        let create = tokio::spawn({
            let root = realms_root.clone();
            async move {
                ensure_realm_manifest_in(
                    &root,
                    realm_id,
                    Some(RealmBackend::Redb),
                    Some(RealmOrigin::Generated),
                )
                .await
                .unwrap();
            }
        });

        let observe = tokio::spawn(async move {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                match tokio::fs::read(&paths.manifest_path).await {
                    Ok(bytes) => {
                        let parsed = serde_json::from_slice::<RealmManifest>(&bytes);
                        assert!(parsed.is_ok(), "manifest should always be valid JSON");
                        return;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                    Err(err) => assert!(
                        err.kind() == std::io::ErrorKind::NotFound,
                        "unexpected io error: {err}"
                    ),
                }
            }
            assert!(
                Instant::now() < deadline,
                "manifest was not created before deadline"
            );
        });

        create.await.unwrap();
        observe.await.unwrap();
    }

    #[tokio::test]
    async fn lease_stale_records_are_cleaned_up() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "lease-stale";
        let paths = realm_paths_in(&realms_root, realm_id);
        let lease_dir = realm_lease_dir(&paths);
        tokio::fs::create_dir_all(&lease_dir).await.unwrap();

        let stale = RealmLeaseRecord {
            realm_id: realm_id.to_string(),
            instance_id: "stale-instance".to_string(),
            surface: "rpc".to_string(),
            pid: 1,
            started_at: 1,
            heartbeat_at: now_unix_secs().saturating_sub(REALM_LEASE_STALE_TTL_SECS + 1),
        };
        let stale_path = lease_dir.join("stale-instance.json");
        write_lease_record(&stale_path, &stale).await.unwrap();

        let status = inspect_realm_leases_in(&realms_root, realm_id, true)
            .await
            .unwrap();
        assert!(status.active.is_empty());
        assert_eq!(status.stale.len(), 1);
        assert!(!tokio::fs::try_exists(&stale_path).await.unwrap());
    }

    #[tokio::test]
    async fn lease_guard_heartbeats_and_shutdown_clears() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "lease-live";

        let guard = start_realm_lease_in(&realms_root, realm_id, Some("live"), "rpc")
            .await
            .unwrap();
        let status = inspect_realm_leases_in(&realms_root, realm_id, true)
            .await
            .unwrap();
        assert_eq!(status.active.len(), 1);
        assert_eq!(status.active[0].instance_id, "live");

        guard.shutdown().await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        let after = inspect_realm_leases_in(&realms_root, realm_id, true)
            .await
            .unwrap();
        assert!(after.active.is_empty());
    }

    #[tokio::test]
    async fn lease_guard_drop_cleans_up_without_explicit_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "lease-drop";

        let guard = start_realm_lease_in(&realms_root, realm_id, Some("drop"), "rpc")
            .await
            .unwrap();
        let lease_path = guard.lease_path().to_path_buf();
        drop(guard);

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !tokio::fs::try_exists(&lease_path).await.unwrap() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "lease file remained after guard drop: {}",
            lease_path.display()
        );
    }
}
