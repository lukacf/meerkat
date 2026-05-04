#[cfg(feature = "jsonl")]
use crate::JsonlStore;
#[cfg(feature = "sqlite")]
use crate::SqliteSessionStore;
use crate::{SessionStore, StoreError};
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
    #[cfg(feature = "sqlite")]
    Sqlite,
}

impl RealmBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            #[cfg(feature = "jsonl")]
            Self::Jsonl => "jsonl",
            #[cfg(feature = "sqlite")]
            Self::Sqlite => "sqlite",
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
///
/// Wave-c C-12 / C-1 follow-up: `realm_id: String` retyped to
/// `realm: RealmId` (matching the C-1 `AuthBindingRef` rename).
/// Serde-renamed to `"realm_id"` on-wire so existing persisted
/// manifests on disk remain readable byte-identical — the retype is
/// purely a domain-side typing improvement; the serialization
/// contract is unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmManifest {
    #[serde(rename = "realm_id")]
    pub realm: meerkat_core::RealmId,
    pub backend: RealmBackend,
    #[serde(default)]
    pub origin: RealmOrigin,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedRealmManifest {
    pub realm_id: String,
    pub backend: String,
    #[serde(default)]
    pub origin: RealmOrigin,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct RealmPaths {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub config_path: PathBuf,
    pub sessions_sqlite_path: PathBuf,
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
        sessions_sqlite_path: root.join("sessions.sqlite3"),
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
    let resolved_instance =
        instance_id.map_or_else(|| format!("instance-{}", Uuid::now_v7()), ToOwned::to_owned);
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
    manifests.sort_by(|a, b| a.manifest.realm.cmp(&b.manifest.realm));
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

fn default_backend() -> Result<RealmBackend, StoreError> {
    #[cfg(feature = "sqlite")]
    {
        Ok(RealmBackend::Sqlite)
    }
    #[cfg(all(not(feature = "sqlite"), feature = "jsonl"))]
    {
        Ok(RealmBackend::Jsonl)
    }
    #[cfg(all(not(feature = "sqlite"), not(feature = "jsonl")))]
    {
        Err(StoreError::Internal(
            "realm support requires at least one persistent backend".to_string(),
        ))
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
            realm_id: manifest.realm.as_str().to_owned(),
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

    let backend = match backend_hint {
        Some(backend) => backend,
        None => default_backend()?,
    };
    let origin = origin_hint.unwrap_or(RealmOrigin::Explicit);
    // Wave-c C-12: lift the `&str` realm-id argument into the typed
    // `RealmId` atom. Upstream callers (surface bootstrap, cli, etc.)
    // must have already validated the slug via
    // `meerkat_core::RealmConfig::selection_from_inputs` /
    // `validate_explicit_realm_id`, but the store-layer boundary
    // parses defensively — a failure here means an upstream caller
    // bypassed its validator.
    let realm = meerkat_core::RealmId::parse(realm_id)
        .map_err(|_| StoreError::InvalidRealmSlug(realm_id.to_string()))?;
    let manifest = RealmManifest {
        realm,
        backend,
        origin,
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
            Ok(bytes) => match parse_manifest_bytes(&bytes) {
                Ok(manifest) => return Ok(manifest),
                Err(err)
                    if manifest_parse_error_may_be_transient(&err) && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
                Err(err) => return Err(err),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            Err(err) => return Err(StoreError::Io(err)),
        }
    }
}

fn manifest_parse_error_may_be_transient(err: &StoreError) -> bool {
    matches!(err, StoreError::Serialization(_))
}

fn parse_manifest_bytes(bytes: &[u8]) -> Result<RealmManifest, StoreError> {
    let persisted: PersistedRealmManifest =
        serde_json::from_slice(bytes).map_err(StoreError::Serialization)?;
    let backend = parse_realm_backend(&persisted.realm_id, &persisted.backend)?;
    // Wave-c C-12: lift the persisted realm slug into the typed
    // `RealmId` atom. Validation failure at this boundary means the
    // on-disk manifest was hand-edited to an invalid slug; surface
    // via a dedicated typed error.
    let realm = meerkat_core::RealmId::parse(&persisted.realm_id)
        .map_err(|_| StoreError::InvalidRealmSlug(persisted.realm_id.clone()))?;
    Ok(RealmManifest {
        realm,
        backend,
        origin: persisted.origin,
        created_at: persisted.created_at,
    })
}

fn parse_realm_backend(realm_id: &str, backend: &str) -> Result<RealmBackend, StoreError> {
    match backend {
        #[cfg(feature = "jsonl")]
        "jsonl" => Ok(RealmBackend::Jsonl),
        #[cfg(feature = "sqlite")]
        "sqlite" => Ok(RealmBackend::Sqlite),
        _ => Err(StoreError::UnsupportedRealmBackend {
            realm_id: realm_id.to_string(),
            backend: backend.to_string(),
        }),
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
    #[cfg(not(any(feature = "jsonl", feature = "sqlite")))]
    {
        let _ = (realms_root, realm_id, backend_hint, origin_hint);
        Err(StoreError::Internal(
            "realm support requires at least one persistent backend".to_string(),
        ))
    }

    #[cfg(any(feature = "jsonl", feature = "sqlite"))]
    {
        let manifest =
            ensure_realm_manifest_in(realms_root, realm_id, backend_hint, origin_hint).await?;
        let paths = realm_paths_in(realms_root, realm_id);

        #[cfg(feature = "jsonl")]
        if manifest.backend == RealmBackend::Jsonl {
            return Ok((
                manifest,
                Arc::new(JsonlStore::new(paths.sessions_jsonl_dir)),
            ));
        }

        #[cfg(feature = "sqlite")]
        if manifest.backend == RealmBackend::Sqlite {
            let sqlite_store = SqliteSessionStore::open(paths.sessions_sqlite_path)?;
            return Ok((manifest, Arc::new(sqlite_store)));
        }

        Err(StoreError::Internal(
            "realm manifest resolved to a backend that is not compiled into this build".to_string(),
        ))
    }
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

    fn supported_backend() -> RealmBackend {
        #[cfg(feature = "sqlite")]
        {
            RealmBackend::Sqlite
        }
        #[cfg(all(not(feature = "sqlite"), feature = "jsonl"))]
        {
            RealmBackend::Jsonl
        }
    }

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
                let backend_hint = Some(supported_backend());
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
            assert_eq!(manifest.realm.as_str(), realm_id);
            assert_eq!(manifest.backend, first_backend);
        }

        let on_disk =
            tokio::fs::read_to_string(realm_paths_in(&realms_root, realm_id).manifest_path)
                .await
                .unwrap();
        let parsed: RealmManifest = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed.realm.as_str(), realm_id);
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
            Some(supported_backend()),
            Some(RealmOrigin::Explicit),
        )
        .await
        .unwrap();
        assert_eq!(created.backend, supported_backend());

        #[cfg(all(feature = "jsonl", feature = "sqlite"))]
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

        #[cfg(all(feature = "jsonl", feature = "sqlite"))]
        let mut handles = Vec::new();
        #[cfg(all(feature = "jsonl", feature = "sqlite"))]
        for idx in 0..32 {
            let root = realms_root.clone();
            handles.push(tokio::spawn(async move {
                let backend_hint = if idx % 2 == 0 {
                    Some(RealmBackend::Sqlite)
                } else {
                    Some(RealmBackend::Jsonl)
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

        #[cfg(all(feature = "jsonl", feature = "sqlite"))]
        {
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

        #[cfg(not(all(feature = "jsonl", feature = "sqlite")))]
        {
            let manifest = ensure_realm_manifest_in(&realms_root, realm_id, None, None)
                .await
                .unwrap();
            assert_eq!(manifest.backend, supported_backend());
        }
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
            "backend": supported_backend().as_str(),
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
    async fn unsupported_manifest_backend_is_rejected_before_store_open() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "unsupported-backend";
        let paths = realm_paths_in(&realms_root, realm_id);
        tokio::fs::create_dir_all(&paths.root).await.unwrap();
        let manifest = serde_json::json!({
            "realm_id": realm_id,
            "backend": "unsupported_backend",
            "created_at": "1234567890"
        });
        tokio::fs::write(
            &paths.manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let err = match open_realm_session_store_in(&realms_root, realm_id, None, None).await {
            Ok(_) => panic!("unsupported manifest backend should fail before opening a store"),
            Err(err) => err,
        };
        assert!(matches!(
            err,
            StoreError::UnsupportedRealmBackend { ref realm_id, ref backend }
                if realm_id == "unsupported-backend" && backend == "unsupported_backend"
        ));
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
                    Some(supported_backend()),
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
