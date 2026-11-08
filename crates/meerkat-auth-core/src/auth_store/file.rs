//! File-backed token store.
//!
//! Layout: default binding credentials live at
//! `<root>/<realm_id>/<binding_id>.json`; profile override credentials live at
//! `<root>/<realm_id>/<binding_id>@<profile_id>.json`; shared account
//! credentials live at `<root>/<realm_id>/account~<account_id>.json`.
//! Permissions: 0o600 on Unix; atomic rename (`.tmp` → target) on save.
//! Reference-CLI parity: Codex `AuthDotJson` (see
//! `codex-rs/login/src/auth/auth_dot_json.rs`).
//!
//! Every write is fenced: `save` and `clear` refuse a guard that does not
//! cover the credential fence C(r), `<root>/<realm_id>/admission.lock`, of
//! the written credential's realm, before any byte is written
//! ([`TokenStoreError::AdmissionFence`]).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use meerkat_core::AdmissionGuard;
use meerkat_core::auth::token_store::TokenStoreObservation;

use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError, require_credential_fence};

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
        let stem = key.storage_stem();
        self.realm_dir(key.realm().as_str())
            .join(format!("{stem}.json"))
    }

    /// Refuse a write `guard` does not cover: the fence of `key`'s realm
    /// under this store's root.
    pub(crate) fn require_fence(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
    ) -> Result<(), TokenStoreError> {
        require_credential_fence(guard, Some(&self.root), key)
    }

    /// Write `key`'s file, for a caller that already required its fence.
    pub(crate) async fn write(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        let dir = self.realm_dir(key.realm().as_str());
        tokio::fs::create_dir_all(&dir).await?;
        let final_path = self.path_for(key);
        let tmp_path = final_path.with_extension("json.tmp");

        let mut bytes = serde_json::to_vec_pretty(tokens)?;
        bytes.push(b'\n');

        write_file_with_mode(&tmp_path, &bytes).await?;
        tokio::fs::rename(&tmp_path, &final_path).await?;
        Ok(())
    }

    /// Remove `key`'s file, for a caller that already required its fence. A
    /// missing file is already clean.
    pub(crate) async fn remove(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let path = self.path_for(key);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(TokenStoreError::from(e)),
        }
    }

    /// Remove `key`'s file synchronously, for cleanup that runs on a thread
    /// with no async runtime (after an Auto keychain write). A missing file is
    /// already clean.
    #[cfg(feature = "keyring")]
    pub(crate) fn evict_blocking(
        &self,
        key: &TokenKey,
    ) -> impl FnOnce() -> Result<(), TokenStoreError> + Send + 'static {
        let path = self.path_for(key);
        move || match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(TokenStoreError::from(error)),
        }
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

    async fn save(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.require_fence(guard, key)?;
        self.write(key, tokens).await
    }

    async fn clear(&self, guard: &AdmissionGuard, key: &TokenKey) -> Result<(), TokenStoreError> {
        self.require_fence(guard, key)?;
        self.remove(key).await
    }

    /// A file read never raises operating-system UI: status reads the file.
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<TokenStoreObservation, TokenStoreError> {
        Ok(match self.load(key).await? {
            Some(tokens) => TokenStoreObservation::Present(Box::new(tokens)),
            None => TokenStoreObservation::Missing,
        })
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
                if let Some(key) = TokenKey::connector_client_secret_from_stem(&realm, stem)
                    .or_else(|| TokenKey::mcp_attachment_identity_key_from_stem(&realm, stem))
                    .or_else(|| TokenKey::mcp_service_secret_from_stem(&realm, stem))
                    .or_else(|| TokenKey::mcp_oauth_client_from_stem(&realm, stem))
                {
                    out.push(key);
                    continue;
                }
                if let Some(account_raw) = stem.strip_prefix("account~") {
                    let account =
                        match meerkat_core::connection::CredentialAccountId::parse(account_raw) {
                            Ok(account) => account,
                            Err(_) => continue,
                        };
                    out.push(TokenKey::from_credential_identity(
                        &meerkat_core::AuthCredentialIdentity::Account(
                            meerkat_core::CredentialAccountRef {
                                realm: realm.clone(),
                                account,
                            },
                        ),
                    ));
                    continue;
                }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::auth_store::PersistedAuthMode;
    use meerkat_core::connection::{
        AuthBindingRef, AuthCredentialIdentity, BindingId, BindingOrigin, CredentialAccountId,
        CredentialAccountRef, RealmId,
    };

    fn token(secret: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some(secret.into()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: vec![],
            account_id: None,
            metadata: serde_json::json!({}),
        }
    }

    /// A guard over the credential fence of `realm` under `root`, as the
    /// coordinator hands its closures.
    fn covering(root: &Path, realm: &str) -> std::sync::Arc<AdmissionGuard> {
        AdmissionGuard::try_acquire(vec![
            meerkat_core::AdmissionFence::credentials(root, &RealmId::parse(realm).unwrap())
                .unwrap(),
        ])
        .unwrap()
    }

    // RCT-19: the on-disk layout is keyed by the OWNING realm.
    #[tokio::test]
    async fn file_token_store_path_for_uses_owning_realm() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(dir.path());
        let guard = covering(dir.path(), "global");
        let key = TokenKey::new(
            RealmId::parse("global").unwrap(),
            BindingId::parse("anthropic").unwrap(),
        );
        store.save(&guard, &key, &token("sek")).await.unwrap();
        let expected = dir.path().join("global").join("anthropic.json");
        assert!(
            expected.exists(),
            "token must persist under the owning-realm directory"
        );
    }

    #[tokio::test]
    async fn account_credential_round_trips_and_lists_without_binding_collision() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(dir.path());
        let identity = AuthCredentialIdentity::Account(CredentialAccountRef {
            realm: RealmId::parse("global").unwrap(),
            account: CredentialAccountId::parse("github_copilot").unwrap(),
        });
        let account_key = TokenKey::from_credential_identity(&identity);
        let binding_key = TokenKey::new(
            RealmId::parse("global").unwrap(),
            BindingId::parse("account").unwrap(),
        );

        store
            .save(
                &covering(dir.path(), "global"),
                &account_key,
                &token("github"),
            )
            .await
            .unwrap();
        store
            .save(
                &covering(dir.path(), "global"),
                &binding_key,
                &token("binding"),
            )
            .await
            .unwrap();

        assert_eq!(
            store
                .load(&account_key)
                .await
                .unwrap()
                .and_then(|tokens| tokens.primary_secret),
            Some("github".to_string())
        );
        let listed = store.list().await.unwrap();
        assert!(listed.contains(&account_key));
        assert!(listed.contains(&binding_key));
        assert!(
            dir.path()
                .join("global")
                .join("account~github_copilot.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn connector_client_secret_round_trips_and_lists_beside_a_legacy_binding() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(dir.path());
        let realm = RealmId::parse("global").unwrap();
        let secret_key = TokenKey::connector_client_secret(
            realm.clone(),
            &format!("desktop@{}", "b".repeat(64)),
        )
        .unwrap();
        // The legacy binding name a reserved key would have overwritten.
        let legacy = TokenKey::new_with_profile(
            realm,
            BindingId::parse("connector-client").unwrap(),
            Some(meerkat_core::connection::ProfileId::parse("desktop").unwrap()),
        );
        let guard = covering(dir.path(), "global");
        store
            .save(&guard, &secret_key, &token("client-secret"))
            .await
            .unwrap();
        store.save(&guard, &legacy, &token("legacy")).await.unwrap();
        assert_eq!(
            store
                .load(&legacy)
                .await
                .unwrap()
                .and_then(|tokens| tokens.primary_secret),
            Some("legacy".to_string())
        );
        let listed = store.list().await.unwrap();
        assert!(listed.contains(&secret_key));
        assert!(listed.contains(&legacy));
    }

    #[tokio::test]
    async fn mcp_oauth_client_round_trips_and_lists_beside_its_slot_credential() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(dir.path());
        let realm = RealmId::parse("mcp-oauth").unwrap();
        let binding = BindingId::parse("remote-AbC123dEf456GhI7").unwrap();
        let client_key = TokenKey::mcp_oauth_client(realm.clone(), binding.clone());
        let credential_key = TokenKey::new(realm, binding);
        let guard = covering(dir.path(), "mcp-oauth");
        store
            .save(&guard, &client_key, &token("client-secret"))
            .await
            .unwrap();
        store
            .save(&guard, &credential_key, &token("access"))
            .await
            .unwrap();
        assert_eq!(
            store
                .load(&client_key)
                .await
                .unwrap()
                .and_then(|tokens| tokens.primary_secret),
            Some("client-secret".to_string())
        );
        assert_eq!(
            store
                .load(&credential_key)
                .await
                .unwrap()
                .and_then(|tokens| tokens.primary_secret),
            Some("access".to_string())
        );
        let listed = store.list().await.unwrap();
        assert!(listed.contains(&client_key));
        assert!(listed.contains(&credential_key));
        assert_eq!(listed.len(), 2);
    }

    // RCT-17: an inherited binding (owner = the defining realm, stamped by the
    // chain walk) resolves its credential at the OWNING realm's namespace; a
    // different (consuming) realm does not see it.
    #[tokio::test]
    async fn managed_store_inherited_binding_reads_owning_realm_token() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileTokenStore::new(dir.path());

        let owner = AuthBindingRef {
            realm: RealmId::parse("global").unwrap(),
            binding: BindingId::parse("anthropic").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        };
        let key = TokenKey::from_auth_binding(&owner);
        assert_eq!(key.realm().as_str(), "global");

        store
            .save(
                &covering(dir.path(), "global"),
                &key,
                &token("owner-secret"),
            )
            .await
            .unwrap();
        let loaded = store.load(&key).await.unwrap().expect("found at owner");
        assert_eq!(loaded.primary_secret.as_deref(), Some("owner-secret"));

        let consuming = TokenKey::new(
            RealmId::parse("child").unwrap(),
            BindingId::parse("anthropic").unwrap(),
        );
        assert!(
            store.load(&consuming).await.unwrap().is_none(),
            "a consuming child realm must not see the global-owned token"
        );
    }
}
