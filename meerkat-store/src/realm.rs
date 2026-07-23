#[cfg(feature = "jsonl")]
use crate::JsonlStore;
#[cfg(feature = "memory")]
use crate::MemoryStore;
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
    #[cfg(feature = "memory")]
    Memory,
    #[cfg(feature = "sqlite")]
    Sqlite,
}

impl RealmBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            #[cfg(feature = "jsonl")]
            Self::Jsonl => "jsonl",
            #[cfg(feature = "memory")]
            Self::Memory => "memory",
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
    /// Manifest format version. 1 (implicit, not serialized) is the
    /// original shape; 2 adds `provider` / `ephemeral_domains`. Readers
    /// refuse formats newer than [`SUPPORTED_MANIFEST_FORMAT`] typed
    /// ([`StoreError::ManifestFromTheFuture`]) instead of silently ignoring
    /// fields they do not understand.
    #[serde(
        default = "default_manifest_format",
        skip_serializing_if = "is_default_manifest_format"
    )]
    pub manifest_format: u32,
    /// External storage-provider discriminator. `None` = the built-in disk
    /// backends. Written together with an `external:<name>` backend string
    /// so pre-v2 binaries reject the realm typed
    /// (`UnsupportedRealmBackend`) rather than opening a renamed-away path
    /// and creating an empty twin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Storage domains this realm explicitly declares ephemeral. A durable
    /// slot resolving to a non-persistent store without its domain listed
    /// here is a startup error (fail-closed durability).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ephemeral_domains: Vec<String>,
}

/// Newest manifest format this binary understands.
pub const SUPPORTED_MANIFEST_FORMAT: u32 = 2;

fn default_manifest_format() -> u32 {
    1
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde skip_serializing_if signature
fn is_default_manifest_format(format: &u32) -> bool {
    *format == 1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedRealmManifest {
    pub realm_id: String,
    pub backend: String,
    #[serde(default)]
    pub origin: RealmOrigin,
    pub created_at: String,
    #[serde(default = "default_manifest_format")]
    pub manifest_format: u32,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub ephemeral_domains: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RealmPaths {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub config_path: PathBuf,
    pub sessions_sqlite_path: PathBuf,
    pub sessions_jsonl_dir: PathBuf,
    pub runtime_sqlite_path: PathBuf,
}

pub const REALM_LEASE_HEARTBEAT_SECS: u64 = 5;
pub const REALM_LEASE_STALE_TTL_SECS: u64 = 30;
pub(crate) const MANIFEST_LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
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
    /// Corrupt-but-present lease files that failed to parse. Their liveness
    /// cannot be ruled out, so they are recorded here and NEVER auto-deleted.
    pub unparseable: Vec<PathBuf>,
}

impl RealmLeaseStatus {
    /// True when any lease is live OR cannot be ruled out as live.
    ///
    /// A corrupt lease is an unknown-state datum (e.g. a still-live instance's
    /// heartbeat caught mid-write), not proof of absence, so its presence must
    /// block destructive prune just like a live lease does.
    pub fn blocks_destructive_prune(&self) -> bool {
        !self.active.is_empty() || !self.unparseable.is_empty()
    }
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
    meerkat_core::default_state_root()
}

/// Sanitize a realm id into its on-disk directory name.
/// (Canonical copy lives in `meerkat_core::runtime_bootstrap`; re-exported
/// here for compatibility.)
pub use meerkat_core::sanitize_realm_id;

pub fn realm_paths_in(realms_root: &Path, realm_id: &str) -> RealmPaths {
    let safe = sanitize_realm_id(realm_id);
    let root = realms_root.join(safe);
    RealmPaths {
        manifest_path: root.join("realm_manifest.json"),
        config_path: root.join("config.toml"),
        sessions_sqlite_path: root.join("sessions.sqlite3"),
        sessions_jsonl_dir: root.join("sessions_jsonl"),
        runtime_sqlite_path: root.join("runtime.sqlite3"),
        root,
    }
}

#[deprecated(
    since = "0.8.4",
    note = "ambient state-root resolution is deprecated; use the `_in` variant with an explicit root (see meerkat_core::StorageLayout)"
)]
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

#[deprecated(
    since = "0.8.4",
    note = "ambient state-root resolution is deprecated; use the `_in` variant with an explicit root (see meerkat_core::StorageLayout)"
)]
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

#[deprecated(
    since = "0.8.4",
    note = "ambient state-root resolution is deprecated; use the `_in` variant with an explicit root (see meerkat_core::StorageLayout)"
)]
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
                // A lease that fails serde parse is malformed/unknown-state, not
                // proof of absence. It may be a still-live instance's heartbeat
                // caught mid-write. Record it as unparseable and NEVER delete it,
                // even under cleanup_stale -- a corrupt lease must block
                // destructive prune, not silently vanish.
                Err(_) => {
                    status.unparseable.push(path.clone());
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

#[deprecated(
    since = "0.8.4",
    note = "ambient state-root resolution is deprecated; use the `_in` variant with an explicit root (see meerkat_core::StorageLayout)"
)]
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
    // Parse the requested realm identity into the typed `RealmId` atom
    // BEFORE deriving any filesystem path. The on-disk path is a lossy
    // sanitization (`.` -> `_`, etc.), so two distinct realm slugs can
    // alias one manifest directory (e.g. raw `a.b` and `a_b` both
    // sanitize to `a_b`). Parsing first lets every existing-manifest
    // branch fail closed on identity inequality instead of silently
    // handing back another realm's manifest.
    let requested_realm = meerkat_core::RealmId::parse(realm_id)
        .map_err(|_| StoreError::InvalidRealmSlug(realm_id.to_string()))?;

    let paths = realm_paths_in(realms_root, realm_id);
    tokio::fs::create_dir_all(&paths.root)
        .await
        .map_err(StoreError::Io)?;

    if tokio::fs::try_exists(&paths.manifest_path)
        .await
        .map_err(StoreError::Io)?
    {
        let existing = read_existing_manifest(&paths.manifest_path).await?;
        validate_realm_identity(&requested_realm, &existing)?;
        validate_backend_hint(&existing, backend_hint)?;
        return Ok(existing);
    }

    let lock_path = paths.root.join(".realm_manifest.lock");
    let _lock = ManifestLockGuard::acquire(&lock_path, realm_id).await?;
    create_or_read_manifest_under_lock(&paths, &requested_realm, backend_hint, origin_hint).await
}

/// Provider-aware manifest ensure: the entry point for
/// [`RealmStorageProvider`](https://docs.rkat.ai)-composed opens.
///
/// `provider = None` is the built-in disk composition (identical to
/// [`ensure_realm_manifest_in`], returned as a Builtin pin — external pins
/// refuse typed). `provider = Some(name)` accepts exactly realms pinned to
/// that provider: an existing manifest pinned elsewhere (builtin backend or
/// a different provider) is a typed
/// [`StoreError::RealmProviderMismatch`]; a fresh realm writes a format-2
/// manifest pinned to the provider (with the `external:<name>` backend
/// string pre-v2 binaries reject typed instead of reopening a relocated
/// realm as an empty twin).
pub async fn ensure_realm_manifest_pin_in(
    realms_root: &Path,
    realm_id: &str,
    provider: Option<&str>,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<RealmManifestPin, StoreError> {
    let Some(provider) = provider else {
        return Ok(RealmManifestPin::Builtin(
            ensure_realm_manifest_in(realms_root, realm_id, backend_hint, origin_hint).await?,
        ));
    };

    let requested_realm = meerkat_core::RealmId::parse(realm_id)
        .map_err(|_| StoreError::InvalidRealmSlug(realm_id.to_string()))?;
    let paths = realm_paths_in(realms_root, realm_id);
    tokio::fs::create_dir_all(&paths.root)
        .await
        .map_err(StoreError::Io)?;

    let read_pin = |bytes: &[u8]| parse_manifest_pin_bytes(bytes);
    let existing = match tokio::fs::read(&paths.manifest_path).await {
        Ok(bytes) => Some(read_pin(&bytes)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(StoreError::Io(err)),
    };
    if let Some(pin) = existing {
        if *pin.realm() != requested_realm {
            return Err(StoreError::RealmIdentityMismatch {
                requested: requested_realm.as_str().to_string(),
                existing: pin.realm().as_str().to_string(),
            });
        }
        return match pin.provider_name() {
            Some(found) if found == provider => Ok(pin),
            found => Err(StoreError::RealmProviderMismatch {
                realm_id: realm_id.to_string(),
                expected: provider.to_string(),
                found: found.unwrap_or("built-in disk backend").to_string(),
            }),
        };
    }

    let lock_path = paths.root.join(".realm_manifest.lock");
    let _lock = ManifestLockGuard::acquire(&lock_path, realm_id).await?;
    // Re-check under the lock (another creator may have won the race).
    if let Ok(bytes) = tokio::fs::read(&paths.manifest_path).await {
        let pin = read_pin(&bytes)?;
        if *pin.realm() != requested_realm {
            return Err(StoreError::RealmIdentityMismatch {
                requested: requested_realm.as_str().to_string(),
                existing: pin.realm().as_str().to_string(),
            });
        }
        return match pin.provider_name() {
            Some(found) if found == provider => Ok(pin),
            found => Err(StoreError::RealmProviderMismatch {
                realm_id: realm_id.to_string(),
                expected: provider.to_string(),
                found: found.unwrap_or("built-in disk backend").to_string(),
            }),
        };
    }
    let manifest = ExternalRealmManifest {
        realm: requested_realm,
        provider: provider.to_string(),
        manifest_format: SUPPORTED_MANIFEST_FORMAT,
        ephemeral_domains: Vec::new(),
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string(),
    };
    let persisted = PersistedRealmManifest {
        realm_id: manifest.realm.as_str().to_string(),
        backend: format!("external:{provider}"),
        origin: origin_hint.unwrap_or(RealmOrigin::Explicit),
        created_at: manifest.created_at.clone(),
        manifest_format: manifest.manifest_format,
        provider: Some(provider.to_string()),
        ephemeral_domains: manifest.ephemeral_domains.clone(),
    };
    write_persisted_manifest_atomically(&paths, &persisted).await?;
    Ok(RealmManifestPin::External(manifest))
}

/// Errors from the cross-candidate first-start reservation
/// ([`ensure_realm_manifest_pin_with_candidates`]).
#[derive(Debug, thiserror::Error)]
pub enum RealmFirstStartError {
    #[error(transparent)]
    Store(#[from] StoreError),
    /// While this open was first-starting the realm under `requested_root`,
    /// a concurrent first-start materialized it under a different candidate
    /// root. Re-resolve: the realm now exists, and dual-root resolution
    /// routes to it where it lies instead of creating a twin.
    #[error(
        "realm '{realm_id}' was concurrently materialized under '{}' while this \
         surface was opening it under '{}'; re-resolve the realm root \
         (resolution now routes to the existing copy)",
        .existing_root.display(), .requested_root.display()
    )]
    MaterializedElsewhere {
        realm_id: String,
        requested_root: PathBuf,
        existing_root: PathBuf,
    },
    /// The first-start reservation stayed contended past the bounded wait
    /// without any candidate producing a manifest (e.g. the reservation
    /// holder is still mid-first-start, or crashed recently enough that its
    /// markers have not aged out yet).
    #[error(
        "realm '{realm_id}' first-start reservation is contended and no manifest \
         appeared under any candidate root within the bounded wait; retry once \
         the concurrent first start completes (crashed markers age out after \
         {} seconds)",
        MANIFEST_LOCK_STALE_AFTER.as_secs()
    )]
    Contention { realm_id: String },
}

/// On-disk payload of a first-start reservation marker. The embedded
/// timestamp drives staleness (mtime is only the fallback for a torn
/// write), so a crashed first start ages out deterministically.
#[derive(Debug, Serialize, Deserialize)]
struct FirstStartMarker {
    realm_id: String,
    pid: u32,
    created_at_unix: u64,
}

fn first_start_marker_path(candidate_root: &Path, realm_id: &str) -> PathBuf {
    candidate_root.join(format!(
        ".realm-first-start.{}.lock",
        sanitize_realm_id(realm_id)
    ))
}

async fn create_first_start_marker(path: &Path, realm_id: &str) -> Result<(), std::io::Error> {
    let payload = serde_json::to_vec(&FirstStartMarker {
        realm_id: realm_id.to_string(),
        pid: std::process::id(),
        created_at_unix: now_unix_secs(),
    })
    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    file.write_all(&payload).await?;
    file.sync_all().await?;
    Ok(())
}

/// Age-based takeover check so a crashed first start cannot brick the
/// realm: payload timestamp first, mtime as the fallback for a torn write.
async fn first_start_marker_is_stale(path: &Path) -> Result<bool, StoreError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            if let Ok(marker) = serde_json::from_slice::<FirstStartMarker>(&bytes) {
                let age = now_unix_secs().saturating_sub(marker.created_at_unix);
                return Ok(age > MANIFEST_LOCK_STALE_AFTER.as_secs());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(StoreError::Io(err)),
    }
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

/// Cross-candidate first-start reservation: `create_new` markers in every
/// candidate root serialize the FIRST materialization of a realm across
/// roots, so two surfaces with different default roots cannot each
/// manufacture a manifest for the same realm.
struct FirstStartReservation {
    markers: Vec<PathBuf>,
}

impl FirstStartReservation {
    /// Acquire markers in every candidate root (deterministic sorted order,
    /// so exactly one contender leads). `Ok(None)` = live contention.
    async fn try_acquire(
        candidates: &[PathBuf],
        realm_id: &str,
    ) -> Result<Option<Self>, StoreError> {
        let mut acquired = Self {
            markers: Vec::new(),
        };
        for candidate in candidates {
            tokio::fs::create_dir_all(candidate)
                .await
                .map_err(StoreError::Io)?;
            let marker_path = first_start_marker_path(candidate, realm_id);
            // One stale-takeover retry per candidate: markers left by a
            // crashed first start must not brick the realm.
            let mut takeover_attempted = false;
            loop {
                match create_first_start_marker(&marker_path, realm_id).await {
                    Ok(()) => {
                        acquired.markers.push(marker_path);
                        break;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                        if !takeover_attempted && first_start_marker_is_stale(&marker_path).await? {
                            takeover_attempted = true;
                            let _ = tokio::fs::remove_file(&marker_path).await;
                            continue;
                        }
                        acquired.release().await;
                        return Ok(None);
                    }
                    Err(err) => {
                        acquired.release().await;
                        return Err(StoreError::Io(err));
                    }
                }
            }
        }
        Ok(Some(acquired))
    }

    async fn release(&mut self) {
        for path in self.markers.drain(..) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

impl Drop for FirstStartReservation {
    fn drop(&mut self) {
        // Crash-safety net for early-return paths; the normal paths release
        // explicitly. Markers left by a hard kill age out via staleness.
        for path in std::mem::take(&mut self.markers) {
            remove_file_nonblocking_on_drop(path);
        }
    }
}

/// True when `root` holds a manifest pinning exactly `realm`. A manifest
/// for a *different* identity aliasing the same sanitized directory is not
/// this realm (resolution refuses that collision before opening); a
/// present-but-unreadable manifest propagates typed.
async fn manifest_present_for(
    root: &Path,
    realm: &meerkat_core::RealmId,
) -> Result<bool, StoreError> {
    let manifest_path = realm_paths_in(root, realm.as_str()).manifest_path;
    let bytes = match tokio::fs::read(&manifest_path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(StoreError::Io(err)),
    };
    let pin = parse_manifest_pin_bytes(&bytes)?;
    Ok(pin.realm() == realm)
}

/// [`ensure_realm_manifest_pin_in`] guarded by the cross-candidate
/// first-start reservation. `candidate_roots` are every state-root
/// candidate resolution probed
/// ([`meerkat_core::StorageLayout::realm_root_candidates`]); `realms_root`
/// is the chosen root, where materialization lands. With zero or one
/// distinct candidate the reservation is skipped entirely (the single-root
/// path is unchanged).
///
/// First-materialization protocol:
/// 1. reserve: `create_new` a marker in EVERY candidate root (sorted order;
///    stale markers from crashed first starts age out);
/// 2. re-probe every candidate for the realm's manifest under reservation —
///    a realm that appeared under a *different* candidate is a typed
///    [`RealmFirstStartError::MaterializedElsewhere`] refusal, never a twin;
/// 3. materialize under the existing per-root manifest lock, then release
///    the markers.
///
/// On reservation contention the loser bounded-waits re-probing (the
/// winner's manifest will appear) and fails typed
/// ([`RealmFirstStartError::Contention`]) if the reservation stays
/// ambiguous.
pub async fn ensure_realm_manifest_pin_with_candidates(
    realms_root: &Path,
    candidate_roots: &[PathBuf],
    realm_id: &str,
    provider: Option<&str>,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<RealmManifestPin, RealmFirstStartError> {
    let mut candidates: Vec<PathBuf> = candidate_roots.to_vec();
    if !candidates.iter().any(|root| root == realms_root) {
        candidates.push(realms_root.to_path_buf());
    }
    candidates.sort();
    candidates.dedup();
    if candidates.len() <= 1 {
        return ensure_realm_manifest_pin_in(
            realms_root,
            realm_id,
            provider,
            backend_hint,
            origin_hint,
        )
        .await
        .map_err(Into::into);
    }
    let requested_realm = meerkat_core::RealmId::parse(realm_id)
        .map_err(|_| StoreError::InvalidRealmSlug(realm_id.to_string()))?;
    let other_roots: Vec<PathBuf> = candidates
        .iter()
        .filter(|root| root.as_path() != realms_root)
        .cloned()
        .collect();

    let materialized_elsewhere =
        |existing_root: PathBuf| RealmFirstStartError::MaterializedElsewhere {
            realm_id: realm_id.to_string(),
            requested_root: realms_root.to_path_buf(),
            existing_root,
        };

    let deadline = Instant::now() + MANIFEST_LOCK_TIMEOUT;
    loop {
        // A manifest under the chosen root: open it (the per-root ensure
        // path validates identity/backend); under another candidate: typed
        // refusal instead of manufacturing a twin.
        if manifest_present_for(realms_root, &requested_realm).await? {
            return ensure_realm_manifest_pin_in(
                realms_root,
                realm_id,
                provider,
                backend_hint,
                origin_hint,
            )
            .await
            .map_err(Into::into);
        }
        for other in &other_roots {
            if manifest_present_for(other, &requested_realm).await? {
                return Err(materialized_elsewhere(other.clone()));
            }
        }
        match FirstStartReservation::try_acquire(&candidates, realm_id).await? {
            Some(mut reservation) => {
                // Re-probe under reservation: a winner may have completed
                // between the probes above and marker acquisition.
                let mut existing_elsewhere = None;
                for other in &other_roots {
                    if manifest_present_for(other, &requested_realm).await? {
                        existing_elsewhere = Some(other.clone());
                        break;
                    }
                }
                if let Some(existing_root) = existing_elsewhere {
                    reservation.release().await;
                    return Err(materialized_elsewhere(existing_root));
                }
                let result = ensure_realm_manifest_pin_in(
                    realms_root,
                    realm_id,
                    provider,
                    backend_hint,
                    origin_hint,
                )
                .await;
                reservation.release().await;
                return result.map_err(Into::into);
            }
            None => {
                if Instant::now() >= deadline {
                    return Err(RealmFirstStartError::Contention {
                        realm_id: realm_id.to_string(),
                    });
                }
                tokio::time::sleep(MANIFEST_LOCK_RETRY_DELAY).await;
            }
        }
    }
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

/// Reject a path-aliased open where the requested realm identity differs
/// from the identity the existing manifest pins. Two raw slugs that
/// sanitize to the same directory (e.g. `a.b` and `a_b`) must never
/// silently share one manifest; fail closed on inequality before any
/// backend-hint validation.
fn validate_realm_identity(
    requested: &meerkat_core::RealmId,
    manifest: &RealmManifest,
) -> Result<(), StoreError> {
    if *requested != manifest.realm {
        return Err(StoreError::RealmIdentityMismatch {
            requested: requested.as_str().to_string(),
            existing: manifest.realm.as_str().to_string(),
        });
    }
    Ok(())
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
    requested_realm: &meerkat_core::RealmId,
    backend_hint: Option<RealmBackend>,
    origin_hint: Option<RealmOrigin>,
) -> Result<RealmManifest, StoreError> {
    if tokio::fs::try_exists(&paths.manifest_path)
        .await
        .map_err(StoreError::Io)?
    {
        let existing = read_existing_manifest(&paths.manifest_path).await?;
        validate_realm_identity(requested_realm, &existing)?;
        validate_backend_hint(&existing, backend_hint)?;
        return Ok(existing);
    }

    let backend = match backend_hint {
        Some(backend) => backend,
        None => default_backend()?,
    };
    let origin = origin_hint.unwrap_or(RealmOrigin::Explicit);
    // Wave-c C-12: the realm-id argument is already the typed `RealmId`
    // atom, parsed at the `ensure_realm_manifest_in` boundary before any
    // filesystem path was derived. Upstream callers (surface bootstrap,
    // cli, etc.) must have already validated the slug via
    // `meerkat_core::RealmConfig::selection_from_inputs` /
    // `validate_explicit_realm_id`; the store-layer boundary parses
    // defensively — a failure there means an upstream caller bypassed
    // its validator.
    let manifest = RealmManifest {
        realm: requested_realm.clone(),
        backend,
        origin,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string(),
        manifest_format: default_manifest_format(),
        provider: None,
        ephemeral_domains: Vec::new(),
    };

    write_manifest_atomically(paths, &manifest).await?;
    Ok(manifest)
}

async fn write_persisted_manifest_atomically(
    paths: &RealmPaths,
    persisted: &PersistedRealmManifest,
) -> Result<(), StoreError> {
    let payload = serde_json::to_vec_pretty(persisted).map_err(StoreError::Serialization)?;
    write_manifest_payload_atomically(paths, payload).await
}

async fn write_manifest_atomically(
    paths: &RealmPaths,
    manifest: &RealmManifest,
) -> Result<(), StoreError> {
    let payload = serde_json::to_vec_pretty(manifest).map_err(StoreError::Serialization)?;
    write_manifest_payload_atomically(paths, payload).await
}

async fn write_manifest_payload_atomically(
    paths: &RealmPaths,
    payload: Vec<u8>,
) -> Result<(), StoreError> {
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

/// A parsed realm manifest: either a built-in disk backend or a realm
/// pinned to an external storage provider. The disk composition refuses
/// external pins typed; provider-aware opens
/// ([`ensure_realm_manifest_pin_in`]) accept exactly their own provider.
#[derive(Debug, Clone)]
pub enum RealmManifestPin {
    Builtin(RealmManifest),
    External(ExternalRealmManifest),
}

/// Manifest of a realm pinned to an external storage provider (persisted
/// with the `external:<name>` backend string pre-v2 binaries reject typed).
#[derive(Debug, Clone)]
pub struct ExternalRealmManifest {
    pub realm: meerkat_core::RealmId,
    pub provider: String,
    pub manifest_format: u32,
    pub ephemeral_domains: Vec<String>,
    pub created_at: String,
}

impl RealmManifestPin {
    pub fn realm(&self) -> &meerkat_core::RealmId {
        match self {
            Self::Builtin(manifest) => &manifest.realm,
            Self::External(manifest) => &manifest.realm,
        }
    }

    pub fn ephemeral_domains(&self) -> &[String] {
        match self {
            Self::Builtin(manifest) => &manifest.ephemeral_domains,
            Self::External(manifest) => &manifest.ephemeral_domains,
        }
    }

    pub fn provider_name(&self) -> Option<&str> {
        match self {
            Self::Builtin(_) => None,
            Self::External(manifest) => Some(&manifest.provider),
        }
    }

    pub fn as_builtin(&self) -> Option<&RealmManifest> {
        match self {
            Self::Builtin(manifest) => Some(manifest),
            Self::External(_) => None,
        }
    }
}

/// Provider-aware manifest parse: external pins are returned, not refused.
fn parse_manifest_pin_bytes(bytes: &[u8]) -> Result<RealmManifestPin, StoreError> {
    let persisted: PersistedRealmManifest =
        serde_json::from_slice(bytes).map_err(StoreError::Serialization)?;
    if persisted.manifest_format > SUPPORTED_MANIFEST_FORMAT {
        // Refuse rather than ignore: an unknown future format may have
        // relocated storage this binary would otherwise recreate empty.
        return Err(StoreError::ManifestFromTheFuture {
            realm_id: persisted.realm_id.clone(),
            found: persisted.manifest_format,
            supported: SUPPORTED_MANIFEST_FORMAT,
        });
    }
    let realm = meerkat_core::RealmId::parse(&persisted.realm_id)
        .map_err(|_| StoreError::InvalidRealmSlug(persisted.realm_id.clone()))?;
    if let Some(provider) = persisted
        .provider
        .clone()
        .or_else(|| externals_backend_provider(&persisted.backend))
    {
        return Ok(RealmManifestPin::External(ExternalRealmManifest {
            realm,
            provider,
            manifest_format: persisted.manifest_format,
            ephemeral_domains: persisted.ephemeral_domains,
            created_at: persisted.created_at,
        }));
    }
    let backend = parse_realm_backend(&persisted.realm_id, &persisted.backend)?;
    Ok(RealmManifestPin::Builtin(RealmManifest {
        realm,
        backend,
        origin: persisted.origin,
        created_at: persisted.created_at,
        manifest_format: persisted.manifest_format,
        provider: None,
        ephemeral_domains: persisted.ephemeral_domains,
    }))
}

fn parse_manifest_bytes(bytes: &[u8]) -> Result<RealmManifest, StoreError> {
    match parse_manifest_pin_bytes(bytes)? {
        RealmManifestPin::Builtin(manifest) => Ok(manifest),
        // Realm pinned to an external provider: the built-in disk
        // composition must not open (or re-materialize) it.
        RealmManifestPin::External(manifest) => Err(StoreError::ExternalProviderRealm {
            realm_id: manifest.realm.as_str().to_string(),
            provider: manifest.provider,
        }),
    }
}

/// `external:<name>` backend strings mark provider-pinned realms; pre-v2
/// binaries reject them via `UnsupportedRealmBackend`.
fn externals_backend_provider(backend: &str) -> Option<String> {
    backend
        .strip_prefix("external:")
        .map(|name| name.to_string())
}

fn parse_realm_backend(realm_id: &str, backend: &str) -> Result<RealmBackend, StoreError> {
    match backend {
        #[cfg(feature = "jsonl")]
        "jsonl" => Ok(RealmBackend::Jsonl),
        #[cfg(feature = "memory")]
        "memory" => Ok(RealmBackend::Memory),
        #[cfg(feature = "sqlite")]
        "sqlite" => Ok(RealmBackend::Sqlite),
        _ => Err(StoreError::UnsupportedRealmBackend {
            realm_id: realm_id.to_string(),
            backend: backend.to_string(),
        }),
    }
}

#[deprecated(
    since = "0.8.4",
    note = "ambient state-root resolution is deprecated; use the `_in` variant with an explicit root (see meerkat_core::StorageLayout)"
)]
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
    #[cfg(not(any(feature = "jsonl", feature = "memory", feature = "sqlite")))]
    {
        let _ = (realms_root, realm_id, backend_hint, origin_hint);
        Err(StoreError::Internal(
            "realm support requires at least one backend".to_string(),
        ))
    }

    #[cfg(any(feature = "jsonl", feature = "memory", feature = "sqlite"))]
    {
        let manifest =
            ensure_realm_manifest_in(realms_root, realm_id, backend_hint, origin_hint).await?;
        // Only the jsonl/sqlite backends consume realm paths; the memory backend
        // is path-free, so binding `paths` under a memory-only build would be dead.
        #[cfg(any(feature = "jsonl", feature = "sqlite"))]
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

        #[cfg(feature = "memory")]
        if manifest.backend == RealmBackend::Memory {
            return Ok((manifest, Arc::new(MemoryStore::new())));
        }

        Err(StoreError::Internal(
            "realm manifest resolved to a backend that is not compiled into this build".to_string(),
        ))
    }
}

/// Realm-identity derivation helpers. (Canonical copies live in
/// `meerkat_core::runtime_bootstrap`; re-exported here for compatibility —
/// the previous local `generate_realm_id` used `Uuid::now_v7()` directly,
/// core's uses the time-compat shim; both mint `realm-<uuidv7>`.)
pub use meerkat_core::{derive_workspace_realm_id, fnv1a64_hex, generate_realm_id};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn manifest_v1_shape_is_preserved_on_write() {
        let manifest = RealmManifest {
            realm: meerkat_core::RealmId::parse("team").expect("realm id"),
            backend: supported_backend(),
            origin: RealmOrigin::Explicit,
            created_at: "0".to_string(),
            manifest_format: 1,
            provider: None,
            ephemeral_domains: Vec::new(),
        };
        let json = serde_json::to_value(&manifest).expect("serialize");
        let object = json.as_object().expect("object");
        // v1 realms keep byte-compatible manifests: no v2 keys serialized.
        assert!(!object.contains_key("manifest_format"), "{json}");
        assert!(!object.contains_key("provider"), "{json}");
        assert!(!object.contains_key("ephemeral_domains"), "{json}");
    }

    #[test]
    fn manifest_from_the_future_is_refused_typed() {
        let bytes = serde_json::json!({
            "realm_id": "team",
            "backend": "sqlite",
            "created_at": "0",
            "manifest_format": SUPPORTED_MANIFEST_FORMAT + 1,
        })
        .to_string();
        let err = parse_manifest_bytes(bytes.as_bytes()).expect_err("must refuse");
        assert!(
            matches!(err, StoreError::ManifestFromTheFuture { found, .. } if found == SUPPORTED_MANIFEST_FORMAT + 1),
            "{err}"
        );
    }

    #[test]
    fn external_provider_realm_is_refused_typed() {
        for manifest in [
            serde_json::json!({
                "realm_id": "team",
                "backend": "external:bigquery",
                "created_at": "0",
            }),
            serde_json::json!({
                "realm_id": "team",
                "backend": "sqlite",
                "created_at": "0",
                "manifest_format": 2,
                "provider": "bigquery",
            }),
        ] {
            let bytes = manifest.to_string();
            let err = parse_manifest_bytes(bytes.as_bytes()).expect_err("must refuse");
            assert!(
                matches!(
                    &err,
                    StoreError::ExternalProviderRealm { provider, .. } if provider == "bigquery"
                ),
                "{err}"
            );
        }
    }

    #[test]
    fn ephemeral_domains_round_trip() {
        // Feature-aware backend string: this lane may build without the
        // sqlite feature, and unknown backend values reject typed.
        let backend = serde_json::to_value(supported_backend()).expect("backend string");
        let bytes = serde_json::json!({
            "realm_id": "team",
            "backend": backend,
            "created_at": "0",
            "manifest_format": 2,
            "ephemeral_domains": ["blobs"],
        })
        .to_string();
        let manifest = parse_manifest_bytes(bytes.as_bytes()).expect("parse");
        assert_eq!(manifest.ephemeral_domains, vec!["blobs".to_string()]);
        assert_eq!(manifest.manifest_format, 2);
    }

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
    async fn path_aliased_realm_identity_is_rejected() {
        // `a.b` and `a_b` are two distinct realm identities (both parse as
        // valid `RealmId` slugs — `.` is allowed by the grammar) but they
        // sanitize to the same on-disk directory (`a_b`). Opening the
        // second one must NOT silently hand back the first realm's
        // manifest; it must fail closed with a typed identity mismatch.
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");

        let created = ensure_realm_manifest_in(
            &realms_root,
            "a.b",
            Some(supported_backend()),
            Some(RealmOrigin::Explicit),
        )
        .await
        .unwrap();
        assert_eq!(created.realm.as_str(), "a.b");

        // Sanity: both raw slugs must resolve to the same manifest path,
        // otherwise the aliasing premise of the test does not hold.
        assert_eq!(
            realm_paths_in(&realms_root, "a.b").manifest_path,
            realm_paths_in(&realms_root, "a_b").manifest_path,
        );

        let err = ensure_realm_manifest_in(&realms_root, "a_b", Some(supported_backend()), None)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                StoreError::RealmIdentityMismatch { ref requested, ref existing }
                    if requested == "a_b" && existing == "a.b"
            ),
            "expected typed identity mismatch, got: {err:?}"
        );

        // The original realm's manifest must still be intact and openable.
        let reopened = ensure_realm_manifest_in(&realms_root, "a.b", None, None)
            .await
            .unwrap();
        assert_eq!(reopened.realm.as_str(), "a.b");
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

    mod first_start_reservation {
        use super::*;

        fn candidates(a: &Path, b: &Path) -> Vec<PathBuf> {
            vec![a.to_path_buf(), b.to_path_buf()]
        }

        #[tokio::test]
        async fn single_candidate_skips_reservation_entirely() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("realms");
            let pin = ensure_realm_manifest_pin_with_candidates(
                &root,
                std::slice::from_ref(&root),
                "team",
                None,
                Some(supported_backend()),
                Some(RealmOrigin::Explicit),
            )
            .await
            .unwrap();
            assert_eq!(pin.realm().as_str(), "team");
            assert!(
                !tokio::fs::try_exists(&first_start_marker_path(&root, "team"))
                    .await
                    .unwrap(),
                "single-root path must not create reservation markers"
            );
        }

        #[tokio::test]
        async fn first_start_materializes_in_chosen_root_and_cleans_markers() {
            let temp = tempfile::tempdir().unwrap();
            let root_a = temp.path().join("a");
            let root_b = temp.path().join("b");
            let pin = ensure_realm_manifest_pin_with_candidates(
                &root_a,
                &candidates(&root_a, &root_b),
                "team",
                None,
                Some(supported_backend()),
                Some(RealmOrigin::Explicit),
            )
            .await
            .unwrap();
            assert_eq!(pin.realm().as_str(), "team");
            assert!(
                tokio::fs::try_exists(&realm_paths_in(&root_a, "team").manifest_path)
                    .await
                    .unwrap()
            );
            assert!(
                !tokio::fs::try_exists(&realm_paths_in(&root_b, "team").manifest_path)
                    .await
                    .unwrap(),
                "the non-chosen candidate must not gain a manifest"
            );
            for root in [&root_a, &root_b] {
                assert!(
                    !tokio::fs::try_exists(&first_start_marker_path(root, "team"))
                        .await
                        .unwrap(),
                    "markers must be released after materialization"
                );
            }
        }

        #[tokio::test]
        async fn realm_materialized_under_other_candidate_is_a_typed_refusal() {
            let temp = tempfile::tempdir().unwrap();
            let root_a = temp.path().join("a");
            let root_b = temp.path().join("b");
            // Simulate the race: after this surface resolved root_a, a
            // concurrent first start materialized the realm under root_b.
            ensure_realm_manifest_in(
                &root_b,
                "team",
                Some(supported_backend()),
                Some(RealmOrigin::Explicit),
            )
            .await
            .unwrap();
            let err = ensure_realm_manifest_pin_with_candidates(
                &root_a,
                &candidates(&root_a, &root_b),
                "team",
                None,
                Some(supported_backend()),
                Some(RealmOrigin::Explicit),
            )
            .await
            .unwrap_err();
            match &err {
                RealmFirstStartError::MaterializedElsewhere {
                    realm_id,
                    requested_root,
                    existing_root,
                } => {
                    assert_eq!(realm_id, "team");
                    assert_eq!(requested_root, &root_a);
                    assert_eq!(existing_root, &root_b);
                }
                other => panic!("wrong error: {other}"),
            }
            assert!(
                !tokio::fs::try_exists(&realm_paths_in(&root_a, "team").manifest_path)
                    .await
                    .unwrap(),
                "the refusal must not manufacture a twin"
            );
        }

        #[tokio::test]
        async fn concurrent_first_starts_with_different_defaults_converge_on_one_root() {
            let temp = tempfile::tempdir().unwrap();
            let root_a = temp.path().join("a");
            let root_b = temp.path().join("b");
            let mut handles = Vec::new();
            for idx in 0..16 {
                let (chosen, other) = if idx % 2 == 0 {
                    (root_a.clone(), root_b.clone())
                } else {
                    (root_b.clone(), root_a.clone())
                };
                handles.push(tokio::spawn(async move {
                    ensure_realm_manifest_pin_with_candidates(
                        &chosen,
                        &[chosen.clone(), other],
                        "team",
                        None,
                        Some(supported_backend()),
                        Some(RealmOrigin::Explicit),
                    )
                    .await
                }));
            }
            let mut ok = 0usize;
            for handle in handles {
                match handle.await.unwrap() {
                    Ok(pin) => {
                        assert_eq!(pin.realm().as_str(), "team");
                        ok += 1;
                    }
                    Err(RealmFirstStartError::MaterializedElsewhere { .. }) => {}
                    // Bounded-wait contention is a legal (rare) outcome when
                    // the scheduler starves a loser past the deadline.
                    Err(RealmFirstStartError::Contention { .. }) => {}
                    Err(other) => panic!("unexpected error: {other}"),
                }
            }
            assert!(ok >= 1, "at least the winner must open the realm");
            let in_a = tokio::fs::try_exists(&realm_paths_in(&root_a, "team").manifest_path)
                .await
                .unwrap();
            let in_b = tokio::fs::try_exists(&realm_paths_in(&root_b, "team").manifest_path)
                .await
                .unwrap();
            assert!(
                in_a != in_b,
                "exactly one candidate root may materialize the realm (a: {in_a}, b: {in_b})"
            );
        }

        #[tokio::test]
        async fn stale_marker_from_crashed_first_start_is_taken_over() {
            let temp = tempfile::tempdir().unwrap();
            let root_a = temp.path().join("a");
            let root_b = temp.path().join("b");
            tokio::fs::create_dir_all(&root_b).await.unwrap();
            // A crashed first start left a marker older than the stale TTL.
            let stale = FirstStartMarker {
                realm_id: "team".to_string(),
                pid: 0,
                created_at_unix: now_unix_secs()
                    .saturating_sub(MANIFEST_LOCK_STALE_AFTER.as_secs() + 5),
            };
            tokio::fs::write(
                first_start_marker_path(&root_b, "team"),
                serde_json::to_vec(&stale).unwrap(),
            )
            .await
            .unwrap();
            let pin = ensure_realm_manifest_pin_with_candidates(
                &root_a,
                &candidates(&root_a, &root_b),
                "team",
                None,
                Some(supported_backend()),
                Some(RealmOrigin::Explicit),
            )
            .await
            .expect("stale markers must not brick the realm");
            assert_eq!(pin.realm().as_str(), "team");
            assert!(
                !tokio::fs::try_exists(&first_start_marker_path(&root_b, "team"))
                    .await
                    .unwrap(),
                "the stale marker must be gone after takeover"
            );
        }

        // Paused clock: the bounded wait elapses virtually instead of
        // stalling the lane for the full 5s reservation timeout.
        #[tokio::test(start_paused = true)]
        async fn fresh_foreign_marker_bounded_waits_then_fails_typed() {
            let temp = tempfile::tempdir().unwrap();
            let root_a = temp.path().join("a");
            let root_b = temp.path().join("b");
            tokio::fs::create_dir_all(&root_b).await.unwrap();
            // A live (fresh) reservation held by someone who never
            // materializes: the loser must fail typed, not guess a root.
            let fresh = FirstStartMarker {
                realm_id: "team".to_string(),
                pid: 0,
                created_at_unix: now_unix_secs(),
            };
            tokio::fs::write(
                first_start_marker_path(&root_b, "team"),
                serde_json::to_vec(&fresh).unwrap(),
            )
            .await
            .unwrap();
            let err = ensure_realm_manifest_pin_with_candidates(
                &root_a,
                &candidates(&root_a, &root_b),
                "team",
                None,
                Some(supported_backend()),
                Some(RealmOrigin::Explicit),
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, RealmFirstStartError::Contention { ref realm_id } if realm_id == "team"),
                "expected typed contention, got: {err}"
            );
            assert!(
                !tokio::fs::try_exists(&realm_paths_in(&root_a, "team").manifest_path)
                    .await
                    .unwrap(),
                "an ambiguous reservation must not materialize"
            );
        }

        #[tokio::test]
        async fn foreign_identity_directory_in_other_candidate_does_not_block() {
            let temp = tempfile::tempdir().unwrap();
            let root_a = temp.path().join("a");
            let root_b = temp.path().join("b");
            // root_b holds realm `a.b`, whose directory aliases `a_b`'s.
            ensure_realm_manifest_in(
                &root_b,
                "a.b",
                Some(supported_backend()),
                Some(RealmOrigin::Explicit),
            )
            .await
            .unwrap();
            // First-starting `a_b` under root_a is a different realm: the
            // aliased directory in the other candidate is not "materialized
            // elsewhere" (identity-colliding *routing* is refused earlier,
            // at resolution).
            let pin = ensure_realm_manifest_pin_with_candidates(
                &root_a,
                &candidates(&root_a, &root_b),
                "a_b",
                None,
                Some(supported_backend()),
                Some(RealmOrigin::Explicit),
            )
            .await
            .unwrap();
            assert_eq!(pin.realm().as_str(), "a_b");
        }
    }

    #[test]
    fn global_realm_config_doc_routes_to_the_injected_global_doc() {
        // Empty catalog: path routing does not consult model data.
        let catalog = meerkat_core::ModelCatalog {
            entries: &[],
            capabilities: &[],
            provider_defaults: &[],
            image_generation_models: &[],
            providers: &[],
            default_models: &[],
            image_generation_defaults: &[],
            global_default_model: "",
            provider_priority: &[],
        };
        let source = FilesystemRealmConfigSource::new(
            "/state/realms",
            "/home/user/.rkat/config.toml",
            catalog,
        );
        let global = meerkat_core::connection::RealmId::global();
        assert_eq!(
            source.config_doc_path(&global),
            PathBuf::from("/home/user/.rkat/config.toml"),
            "the reserved global realm's doc must resolve through the \
             user-global root regardless of the state root"
        );
        let team = meerkat_core::connection::RealmId::parse("team").unwrap();
        assert_eq!(
            source.config_doc_path(&team),
            realm_paths_in(Path::new("/state/realms"), "team").config_path,
        );
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
        assert!(status.unparseable.is_empty());
        assert!(!tokio::fs::try_exists(&stale_path).await.unwrap());
    }

    #[tokio::test]
    async fn lease_unparseable_is_recorded_and_not_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let realms_root = temp.path().join("realms");
        let realm_id = "lease-corrupt";
        let paths = realm_paths_in(&realms_root, realm_id);
        let lease_dir = realm_lease_dir(&paths);
        tokio::fs::create_dir_all(&lease_dir).await.unwrap();

        // A corrupt lease (e.g. a heartbeat write caught mid-rename) is an
        // unknown-state datum -- not proof of absence.
        let corrupt_path = lease_dir.join("corrupt-instance.json");
        tokio::fs::write(&corrupt_path, b"{ not json")
            .await
            .unwrap();

        // cleanup_stale=true is what every destructive/listing caller passes.
        let status = inspect_realm_leases_in(&realms_root, realm_id, true)
            .await
            .unwrap();
        assert!(status.active.is_empty());
        assert!(status.stale.is_empty());
        assert_eq!(status.unparseable.len(), 1);
        // The corrupt file must NOT be auto-deleted even under cleanup_stale=true.
        assert!(tokio::fs::try_exists(&corrupt_path).await.unwrap());
        assert!(status.blocks_destructive_prune());
    }

    #[test]
    fn blocks_destructive_prune_true_when_only_unparseable() {
        let status = RealmLeaseStatus {
            active: Vec::new(),
            stale: Vec::new(),
            unparseable: vec![PathBuf::from("/tmp/corrupt.json")],
        };
        assert!(status.blocks_destructive_prune());

        let empty = RealmLeaseStatus::default();
        assert!(!empty.blocks_destructive_prune());
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

/// Filesystem-backed [`meerkat_core::RealmConfigSource`] shared by every
/// runtime-backed surface (CLI, REST, RPC, MCP). It is the single owner of the
/// realm→config-document filesystem projection: the reserved `global` realm maps
/// to a home-rooted document (`global_doc`), every other realm to its per-realm
/// `<state_root>/<realm>/config.toml`. Returns `None` for an absent document so
/// composition never folds a `Config::default()` clobber. Surfaces inject this
/// rather than each re-deriving the mapping.
#[cfg(not(target_arch = "wasm32"))]
pub struct FilesystemRealmConfigSource {
    state_root: PathBuf,
    global_doc: PathBuf,
    catalog: meerkat_core::ModelCatalog,
}

#[cfg(not(target_arch = "wasm32"))]
impl FilesystemRealmConfigSource {
    pub fn new(
        state_root: impl Into<PathBuf>,
        global_doc: impl Into<PathBuf>,
        catalog: meerkat_core::ModelCatalog,
    ) -> Self {
        Self {
            state_root: state_root.into(),
            global_doc: global_doc.into(),
            catalog,
        }
    }

    /// The config document a realm resolves to: the reserved `global` realm
    /// maps to the injected home-rooted global doc **regardless of which
    /// state root materialized the realm**, every other realm to its
    /// per-realm `<state_root>/<realm>/config.toml`.
    ///
    /// Writers must route through this too: writing the reserved realm's
    /// doc at `<state_root>/global/config.toml` would shadow the user-global
    /// document that child realms actually inherit from.
    pub fn config_doc_path(&self, realm: &meerkat_core::connection::RealmId) -> PathBuf {
        if realm.is_global() {
            self.global_doc.clone()
        } else {
            realm_paths_in(&self.state_root, realm.as_str()).config_path
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl meerkat_core::RealmConfigSource for FilesystemRealmConfigSource {
    async fn config_for_realm(
        &self,
        realm: &meerkat_core::connection::RealmId,
    ) -> Result<Option<meerkat_core::Config>, meerkat_core::config::ConfigError> {
        use meerkat_core::ConfigStore;
        let path = self.config_doc_path(realm);
        // Fail closed on an undeterminable existence probe (EACCES/ELOOP/ENOTDIR
        // on the path or an ancestor): a present-but-unstat-able config doc must
        // NOT silently drop its inherited credentials/limits from the fold — only
        // a confirmed `Ok(false)` means the doc is genuinely absent.
        match tokio::fs::try_exists(&path).await {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(err) => return Err(err.into()),
        }
        // FileConfigStore::get parses the toml; the existence check above means a
        // present-but-absent doc never collapses to Config::default().
        let store = meerkat_core::FileConfigStore::new(path, self.catalog);
        store.get().await.map(Some)
    }

    async fn raw_config_for_realm(
        &self,
        realm: &meerkat_core::connection::RealmId,
    ) -> Result<Option<toml::Value>, meerkat_core::config::ConfigError> {
        // Same path resolution as `config_for_realm`; return the parsed raw TOML
        // so composition can honor presence (a child explicitly setting a scalar
        // to its struct default still wins). Absent doc -> None.
        let path = self.config_doc_path(realm);
        // Fail closed on an undeterminable existence probe (see config_for_realm):
        // only a confirmed `Ok(false)` means absent; an IO fault propagates so a
        // present-but-unreadable doc cannot silently drop its inherited presence.
        match tokio::fs::try_exists(&path).await {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(err) => return Err(err.into()),
        }
        let bytes = tokio::fs::read(&path).await?;
        let text = String::from_utf8(bytes)?;
        let value: toml::Value = toml::from_str(&text)?;
        Ok(Some(value))
    }
}
