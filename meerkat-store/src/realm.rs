#[cfg(feature = "jsonl")]
use crate::JsonlStore;
use crate::{RedbSessionStore, SessionStore, StoreError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
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

/// Realm-scoped manifest pinned at creation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmManifest {
    pub realm_id: String,
    pub backend: RealmBackend,
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

pub fn realm_paths(realm_id: &str) -> RealmPaths {
    let safe = sanitize_realm_id(realm_id);
    let root = default_realms_root().join(safe);
    RealmPaths {
        manifest_path: root.join("realm_manifest.json"),
        config_path: root.join("config.toml"),
        sessions_redb_path: root.join("sessions.redb"),
        sessions_jsonl_dir: root.join("sessions_jsonl"),
        root,
    }
}

pub async fn ensure_realm_manifest(
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
) -> Result<RealmManifest, StoreError> {
    let paths = realm_paths(realm_id);
    if tokio::fs::try_exists(&paths.manifest_path)
        .await
        .map_err(StoreError::Io)?
    {
        let bytes = tokio::fs::read(&paths.manifest_path)
            .await
            .map_err(StoreError::Io)?;
        let manifest: RealmManifest =
            serde_json::from_slice(&bytes).map_err(StoreError::Serialization)?;
        return Ok(manifest);
    }

    tokio::fs::create_dir_all(&paths.root)
        .await
        .map_err(StoreError::Io)?;
    let manifest = RealmManifest {
        realm_id: realm_id.to_string(),
        backend: backend_hint.unwrap_or({
            #[cfg(feature = "jsonl")]
            {
                RealmBackend::Jsonl
            }
            #[cfg(not(feature = "jsonl"))]
            {
                RealmBackend::Redb
            }
        }),
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string(),
    };
    let payload = serde_json::to_vec_pretty(&manifest).map_err(StoreError::Serialization)?;
    tokio::fs::write(&paths.manifest_path, payload)
        .await
        .map_err(StoreError::Io)?;
    Ok(manifest)
}

pub async fn open_realm_session_store(
    realm_id: &str,
    backend_hint: Option<RealmBackend>,
) -> Result<(RealmManifest, Arc<dyn SessionStore>), StoreError> {
    let manifest = ensure_realm_manifest(realm_id, backend_hint).await?;
    let paths = realm_paths(realm_id);
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

pub fn fvn1a64_hex(input: &str) -> String {
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
    format!("ws-{}", fvn1a64_hex(&key))
}

pub fn generate_realm_id() -> String {
    format!("realm-{}", Uuid::now_v7())
}
