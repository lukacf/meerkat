//! File-backed token store.
//!
//! Layout: default binding credentials live at
//! `<root>/<realm_id>/<binding_id>.json`; profile override credentials live at
//! `<root>/<realm_id>/<binding_id>@<profile_id>.json`.
//! Permissions: 0o600 on Unix; atomic rename (`.tmp` → target) on save.
//! Reference-CLI parity: Codex `AuthDotJson` (see
//! `codex-rs/login/src/auth/auth_dot_json.rs`).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

pub struct FileTokenStore {
    root: PathBuf,
}

impl FileTokenStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn realm_dir(&self, realm: &str) -> PathBuf {
        self.root.join(realm)
    }

    fn path_for(&self, key: &TokenKey) -> PathBuf {
        let stem = match &key.profile {
            Some(profile) => format!("{}@{}", key.binding.as_str(), profile.as_str()),
            None => key.binding.as_str().to_string(),
        };
        self.realm_dir(key.realm.as_str())
            .join(format!("{stem}.json"))
    }
}

#[async_trait]
impl TokenStore for FileTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let path = self.path_for(key);
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(TokenStoreError::from(e)),
        };
        let tokens: PersistedTokens = serde_json::from_slice(&bytes)?;
        Ok(Some(tokens))
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        let dir = self.realm_dir(key.realm.as_str());
        tokio::fs::create_dir_all(&dir).await?;
        let final_path = self.path_for(key);
        let tmp_path = final_path.with_extension("json.tmp");

        let mut bytes = serde_json::to_vec_pretty(tokens)?;
        bytes.push(b'\n');

        write_file_with_mode(&tmp_path, &bytes).await?;
        tokio::fs::rename(&tmp_path, &final_path).await?;
        Ok(())
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let path = self.path_for(key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(TokenStoreError::from(e)),
        }
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        let mut out = Vec::new();
        let mut realms = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(TokenStoreError::from(e)),
        };
        while let Some(realm_entry) = realms.next_entry().await? {
            let realm_path = realm_entry.path();
            if !realm_path.is_dir() {
                continue;
            }
            let realm_raw = match realm_path.file_name().and_then(|n| n.to_str()) {
                Some(s) => s,
                None => continue,
            };
            // Skip directories whose names are not valid realm slugs —
            // they cannot have been written by this store, so they
            // cannot be returned as TokenKeys.
            let realm = match meerkat_core::connection::RealmId::parse(realm_raw) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let mut bindings = match tokio::fs::read_dir(&realm_path).await {
                Ok(rd) => rd,
                Err(e) => return Err(TokenStoreError::from(e)),
            };
            while let Some(entry) = bindings.next_entry().await? {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let stem = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s,
                    None => continue,
                };
                let (binding_raw, profile_raw) = match stem.split_once('@') {
                    Some((binding, profile)) => (binding, Some(profile)),
                    None => (stem, None),
                };
                let binding = match meerkat_core::connection::BindingId::parse(binding_raw) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let profile = match profile_raw {
                    Some(raw) => match meerkat_core::connection::ProfileId::parse(raw) {
                        Ok(profile) => Some(profile),
                        Err(_) => continue,
                    },
                    None => None,
                };
                out.push(TokenKey::new_with_profile(realm.clone(), binding, profile));
            }
        }
        Ok(out)
    }

    fn backend_name(&self) -> &'static str {
        "file"
    }
}

#[cfg(unix)]
async fn write_file_with_mode(path: &Path, bytes: &[u8]) -> Result<(), TokenStoreError> {
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    Ok(())
}

#[cfg(not(unix))]
async fn write_file_with_mode(path: &Path, bytes: &[u8]) -> Result<(), TokenStoreError> {
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    Ok(())
}
