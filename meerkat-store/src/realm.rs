#[cfg(feature = "jsonl")]
use crate::JsonlStore;
use crate::{RedbSessionStore, SessionStore, StoreError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealmOrigin {
    Generated,
    Workspace,
    Explicit,
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

impl Default for RealmOrigin {
    fn default() -> Self {
        Self::LegacyUnknown
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
    let acquire_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .await
        {
            Ok(lock_file) => {
                let result =
                    create_or_read_manifest_under_lock(&paths, realm_id, backend_hint, origin_hint)
                        .await;
                drop(lock_file);
                let _ = tokio::fs::remove_file(&lock_path).await;
                return result;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if tokio::fs::try_exists(&paths.manifest_path)
                    .await
                    .map_err(StoreError::Io)?
                {
                    let existing = read_existing_manifest(&paths.manifest_path).await?;
                    validate_backend_hint(&existing, backend_hint)?;
                    return Ok(existing);
                }
                if Instant::now() >= acquire_deadline {
                    // Best-effort stale lock cleanup.
                    let _ = tokio::fs::remove_file(&lock_path).await;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => return Err(StoreError::Io(err)),
        }
    }
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
    if let Some(requested) = backend_hint {
        if requested != manifest.backend {
            return Err(StoreError::RealmBackendMismatch {
                realm_id: manifest.realm_id.clone(),
                requested: requested.as_str().to_string(),
                existing: manifest.backend.as_str().to_string(),
            });
        }
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

async fn write_manifest_atomically(paths: &RealmPaths, manifest: &RealmManifest) -> Result<(), StoreError> {
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
    let manifest = ensure_realm_manifest_in(realms_root, realm_id, backend_hint, origin_hint).await?;
    let paths = realm_paths_in(realms_root, realm_id);
    let store: Arc<dyn SessionStore> = match manifest.backend {
        #[cfg(feature = "jsonl")]
        RealmBackend::Jsonl => Arc::new(JsonlStore::new(paths.sessions_jsonl_dir)),
        RealmBackend::Redb => {
            if let Some(parent) = paths.sessions_redb_path.parent() {
                std::fs::create_dir_all(parent).map_err(StoreError::Io)?;
            }
            Arc::new(RedbSessionStore::open(&paths.sessions_redb_path)?)
        }
    };
    Ok((manifest, store))
}

pub fn fnv1a64_hex(input: &str) -> String {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
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
                ensure_realm_manifest_in(&root, realm_id, backend_hint, Some(RealmOrigin::Generated))
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
                ensure_realm_manifest_in(&root, realm_id, backend_hint, Some(RealmOrigin::Generated))
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
            assert!(Instant::now() < deadline, "manifest was not created before deadline");
        });

        create.await.unwrap();
        observe.await.unwrap();
    }
}
