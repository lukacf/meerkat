//! Auto-detecting token store: tries the OS keyring first (when the
//! `native-keyring` feature is enabled), falls back to the file store on
//! keyring failure, unavailability, or missing entry. Writes go to the
//! keyring when available; fall back to file when not. Reads try keyring
//! first, then file.

use std::path::{Path, PathBuf};
#[cfg(feature = "keyring")]
use std::sync::Arc;

use async_trait::async_trait;

use super::file::FileTokenStore;
#[cfg(feature = "keyring")]
use super::keyring::KeyringTokenStore;
#[cfg(feature = "keyring")]
use super::{PersistedAuthLeaseBinding, PersistedAuthMode};
use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

pub struct AutoTokenStore {
    file: FileTokenStore,
    #[cfg(feature = "keyring")]
    keyring: Arc<dyn AutoKeyringBackend>,
    #[cfg(not(feature = "keyring"))]
    _service_name: String,
}

#[cfg(feature = "keyring")]
#[async_trait]
trait AutoKeyringBackend: Send + Sync {
    fn load_unlocked(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError>;
    fn save_unlocked(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError>;
    fn clear_unlocked(&self, key: &TokenKey) -> Result<(), TokenStoreError>;
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError>;
}

#[cfg(feature = "keyring")]
#[async_trait]
impl AutoKeyringBackend for KeyringTokenStore {
    fn load_unlocked(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        KeyringTokenStore::load_unlocked(self, key)
    }

    fn save_unlocked(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        KeyringTokenStore::save_unlocked(self, key, tokens)
    }

    fn clear_unlocked(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        KeyringTokenStore::clear_unlocked(self, key)
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        TokenStore::list(self).await
    }
}

#[cfg(feature = "keyring")]
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AutoClearTombstone {
    Any,
    Lease {
        auth_mode: PersistedAuthMode,
        auth_lease: PersistedAuthLeaseBinding,
    },
}

#[cfg(feature = "keyring")]
impl AutoClearTombstone {
    fn from_tokens(tokens: &PersistedTokens) -> Option<Self> {
        Some(Self::Lease {
            auth_mode: tokens.auth_mode,
            auth_lease: tokens.auth_lease.clone()?,
        })
    }

    fn matches_tokens(&self, tokens: &PersistedTokens) -> bool {
        match self {
            Self::Any => true,
            Self::Lease {
                auth_mode,
                auth_lease,
            } => tokens.auth_mode == *auth_mode && tokens.auth_lease.as_ref() == Some(auth_lease),
        }
    }
}

#[cfg(feature = "keyring")]
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct AutoFileFallbackMarker {
    auth_mode: PersistedAuthMode,
    auth_lease: Option<PersistedAuthLeaseBinding>,
}

#[cfg(feature = "keyring")]
impl AutoFileFallbackMarker {
    fn from_tokens(tokens: &PersistedTokens) -> Self {
        Self {
            auth_mode: tokens.auth_mode,
            auth_lease: tokens.auth_lease.clone(),
        }
    }

    fn matches_tokens(&self, tokens: &PersistedTokens) -> bool {
        tokens.auth_mode == self.auth_mode && tokens.auth_lease == self.auth_lease
    }
}

impl AutoTokenStore {
    #[cfg(feature = "keyring")]
    pub fn new(root: impl Into<PathBuf>, service_name: impl Into<String>) -> Self {
        Self {
            file: FileTokenStore::new(root),
            keyring: Arc::new(KeyringTokenStore::new(service_name)),
        }
    }

    #[cfg(all(test, feature = "keyring"))]
    fn new_with_keyring_backend(
        root: impl Into<PathBuf>,
        keyring: Arc<dyn AutoKeyringBackend>,
    ) -> Self {
        Self {
            file: FileTokenStore::new(root),
            keyring,
        }
    }

    #[cfg(not(feature = "keyring"))]
    pub fn new(root: impl Into<PathBuf>, service_name: impl Into<String>) -> Self {
        Self {
            file: FileTokenStore::new(root),
            _service_name: service_name.into(),
        }
    }

    pub fn root(&self) -> &Path {
        self.file.root()
    }

    #[cfg(feature = "keyring")]
    fn file_shadow_path(&self, key: &TokenKey) -> PathBuf {
        let stem = match &key.profile {
            Some(profile) => format!("{}@{}", key.binding.as_str(), profile.as_str()),
            None => key.binding.as_str().to_string(),
        };
        self.file
            .root()
            .join(key.realm.as_str())
            .join(format!("{stem}.json"))
    }

    #[cfg(feature = "keyring")]
    fn clear_tombstone_path(&self, key: &TokenKey) -> PathBuf {
        self.file_shadow_path(key).with_extension("json.cleared")
    }

    #[cfg(feature = "keyring")]
    async fn load_clear_tombstone(
        &self,
        key: &TokenKey,
    ) -> Result<Option<AutoClearTombstone>, TokenStoreError> {
        let path = self.clear_tombstone_path(key);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(TokenStoreError::from(e)),
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    #[cfg(feature = "keyring")]
    async fn save_clear_tombstone(
        &self,
        key: &TokenKey,
        tombstone: &AutoClearTombstone,
    ) -> Result<(), TokenStoreError> {
        let path = self.clear_tombstone_path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut bytes = serde_json::to_vec_pretty(tombstone)?;
        bytes.push(b'\n');
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }

    #[cfg(feature = "keyring")]
    async fn clear_clear_tombstone(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let path = self.clear_tombstone_path(key);
        clear_sidecar_path(&path).await
    }

    #[cfg(feature = "keyring")]
    fn file_fallback_marker_path(&self, key: &TokenKey) -> PathBuf {
        self.file_shadow_path(key).with_extension("json.fallback")
    }

    #[cfg(feature = "keyring")]
    async fn load_file_fallback_marker(
        &self,
        key: &TokenKey,
    ) -> Result<Option<AutoFileFallbackMarker>, TokenStoreError> {
        let path = self.file_fallback_marker_path(key);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(TokenStoreError::from(e)),
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    #[cfg(feature = "keyring")]
    async fn save_file_fallback_marker(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        let path = self.file_fallback_marker_path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut bytes = serde_json::to_vec_pretty(&AutoFileFallbackMarker::from_tokens(tokens))?;
        bytes.push(b'\n');
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }

    #[cfg(feature = "keyring")]
    async fn clear_file_fallback_marker(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let path = self.file_fallback_marker_path(key);
        clear_sidecar_path(&path).await
    }

    #[cfg(feature = "keyring")]
    async fn save_clear_tombstone_and_clear_marker(
        &self,
        key: &TokenKey,
        tombstone: &AutoClearTombstone,
    ) -> Result<(), TokenStoreError> {
        let wrote_tombstone = match self.load_clear_tombstone(key).await? {
            Some(existing) if existing != *tombstone => {
                return Err(TokenStoreError::KeyringUnavailable(
                    "different clear tombstone is already pending for hidden keyring material"
                        .into(),
                ));
            }
            Some(_) => false,
            None => {
                self.save_clear_tombstone(key, tombstone).await?;
                true
            }
        };
        if let Err(marker_error) = self.clear_file_fallback_marker(key).await {
            if !wrote_tombstone {
                return Err(marker_error);
            }
            return match self.clear_clear_tombstone(key).await {
                Ok(()) => Err(marker_error),
                Err(rollback_error) => Err(TokenStoreError::Unavailable(format!(
                    "clear tombstone was written but fallback marker cleanup failed: \
                     {marker_error}; tombstone rollback failed: {rollback_error}"
                ))),
            };
        }
        Ok(())
    }

    #[cfg(feature = "keyring")]
    async fn fail_if_unmarked_divergent_auth_lease_file(
        &self,
        key: &TokenKey,
        keyring_tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        match self.file.load_unlocked(key).await {
            Ok(Some(file_tokens))
                if file_tokens != *keyring_tokens && has_auth_lease_binding(&file_tokens, key) =>
            {
                Err(TokenStoreError::Unavailable(
                    "divergent auth lease material exists in file and keyring without a file fallback marker"
                        .into(),
                ))
            }
            Ok(_) | Err(_) => Ok(()),
        }
    }

    #[cfg(feature = "keyring")]
    fn keyring_rollback(
        &self,
        key: &TokenKey,
        previous_keyring_tokens: Option<&PersistedTokens>,
    ) -> Result<(), TokenStoreError> {
        match previous_keyring_tokens {
            Some(previous) => self.keyring.save_unlocked(key, previous),
            None => self.keyring.clear_unlocked(key),
        }
    }

    #[cfg(feature = "keyring")]
    async fn load_marked_file_fallback(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let Some(marker) = self.load_file_fallback_marker(key).await? else {
            return Ok(None);
        };
        match self.file.load_unlocked(key).await {
            Ok(Some(file_tokens)) if marker.matches_tokens(&file_tokens) => Ok(Some(file_tokens)),
            Ok(Some(_) | None) => Err(TokenStoreError::Unavailable(
                "file fallback marker is present but matching file material is not readable".into(),
            )),
            Err(e) => Err(TokenStoreError::Unavailable(format!(
                "file fallback marker is present but matching file material is not readable: {e}"
            ))),
        }
    }

    #[cfg(feature = "keyring")]
    async fn load_unmarked_file_material(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let Some(file_tokens) = self.file.load_unlocked(key).await? else {
            return Ok(None);
        };
        if has_auth_lease_binding(&file_tokens, key) {
            return Err(TokenStoreError::Unavailable(
                "unmarked auth lease material exists in file without keyring or fallback marker"
                    .into(),
            ));
        }
        Ok(Some(file_tokens))
    }

    #[cfg(feature = "keyring")]
    async fn load_keyring_unavailable_cas_current(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
            return Ok(Some(file_tokens));
        }
        if let Some(tombstone) = self.load_clear_tombstone(key).await? {
            return self
                .reconcile_clear_tombstone_without_keyring(key, &tombstone, false)
                .await;
        }
        self.load_unmarked_file_material(key).await
    }

    #[cfg(feature = "keyring")]
    async fn load_keyring_unavailable_initial_cas_current(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
            return Ok(Some(file_tokens));
        }
        self.load_unmarked_file_material(key).await
    }

    #[cfg(feature = "keyring")]
    async fn clear_file_matching_tombstone(
        &self,
        key: &TokenKey,
        tombstone: &AutoClearTombstone,
    ) -> Result<bool, TokenStoreError> {
        match self.file.load_unlocked(key).await {
            Ok(Some(file_tokens)) if tombstone.matches_tokens(&file_tokens) => {
                self.file.clear_unlocked(key).await?;
                Ok(true)
            }
            Ok(Some(_)) => Ok(false),
            Ok(None) => Ok(true),
            Err(TokenStoreError::Serde(_)) if matches!(tombstone, AutoClearTombstone::Any) => {
                self.file.clear_unlocked(key).await?;
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }

    #[cfg(feature = "keyring")]
    async fn clear_file_after_tombstone_commit(&self, key: &TokenKey) {
        let _ = self.file.clear_unlocked(key).await;
    }

    #[cfg(feature = "keyring")]
    async fn reconcile_clear_tombstone_without_keyring(
        &self,
        key: &TokenKey,
        tombstone: &AutoClearTombstone,
        keyring_available: bool,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let tombstone_applied = self.clear_file_matching_tombstone(key, tombstone).await?;
        if tombstone_applied {
            self.clear_file_fallback_marker(key).await?;
            if keyring_available {
                self.clear_clear_tombstone(key).await?;
            }
            return Ok(None);
        }
        if keyring_available {
            self.clear_clear_tombstone(key).await?;
        }
        self.load_unmarked_file_material(key).await
    }

    #[cfg(feature = "keyring")]
    async fn reconcile_clear_tombstone_before_cas(
        &self,
        key: &TokenKey,
        keyring_tokens: &mut Option<PersistedTokens>,
    ) -> Result<(), TokenStoreError> {
        let Some(tombstone) = self.load_clear_tombstone(key).await? else {
            return Ok(());
        };
        match keyring_tokens.as_ref() {
            Some(tokens) if tombstone.matches_tokens(tokens) => {
                let tombstone_applied_to_file =
                    self.clear_file_matching_tombstone(key, &tombstone).await?;
                self.keyring.clear_unlocked(key)?;
                self.clear_clear_tombstone(key).await?;
                if tombstone_applied_to_file {
                    self.clear_file_fallback_marker(key).await?;
                }
                *keyring_tokens = None;
            }
            Some(_) => {
                self.clear_clear_tombstone(key).await?;
            }
            None => {
                let _ = self
                    .reconcile_clear_tombstone_without_keyring(key, &tombstone, true)
                    .await?;
            }
        }
        Ok(())
    }

    async fn load_unlocked(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(Some(t)) => {
                    if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
                        self.save_keyring_and_file_shadow(key, Some(&t), &file_tokens)
                            .await?;
                        return Ok(Some(file_tokens));
                    }
                    if let Some(tombstone) = self.load_clear_tombstone(key).await? {
                        if tombstone.matches_tokens(&t) {
                            let tombstone_applied_to_file =
                                self.clear_file_matching_tombstone(key, &tombstone).await?;
                            self.keyring.clear_unlocked(key)?;
                            self.clear_clear_tombstone(key).await?;
                            if tombstone_applied_to_file {
                                self.clear_file_fallback_marker(key).await?;
                                return Ok(None);
                            }
                            return self.load_unmarked_file_material(key).await;
                        }
                        self.clear_clear_tombstone(key).await?;
                    }
                    return self.reconcile_keyring_load(key, t).await.map(Some);
                }
                Ok(None) => {
                    if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
                        let _ = self.clear_clear_tombstone(key).await;
                        return Ok(Some(file_tokens));
                    }
                    if let Some(tombstone) = self.load_clear_tombstone(key).await? {
                        return self
                            .reconcile_clear_tombstone_without_keyring(key, &tombstone, true)
                            .await;
                    }
                    return self.load_unmarked_file_material(key).await;
                }
                Err(TokenStoreError::KeyringUnavailable(_)) => {
                    if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
                        let _ = self.clear_clear_tombstone(key).await;
                        return Ok(Some(file_tokens));
                    }
                    if let Some(tombstone) = self.load_clear_tombstone(key).await? {
                        return self
                            .reconcile_clear_tombstone_without_keyring(key, &tombstone, false)
                            .await;
                    }
                    return match self.load_unmarked_file_material(key).await {
                        Ok(None) => Err(TokenStoreError::KeyringUnavailable(
                            "keyring unavailable and no file fallback material is present".into(),
                        )),
                        other => other,
                    };
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(not(feature = "keyring"))]
        {
            self.file.load_unlocked(key).await
        }
    }

    async fn save_unlocked(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(previous) => {
                    return self
                        .save_keyring_and_file_shadow(key, previous.as_ref(), tokens)
                        .await;
                }
                Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        #[cfg(feature = "keyring")]
        {
            let previous_file_tokens = self.file.load_unlocked(key).await.ok().flatten();
            self.save_file_fallback_material(key, previous_file_tokens.as_ref(), tokens)
                .await?;
        }
        #[cfg(not(feature = "keyring"))]
        {
            self.file.save_unlocked(key, tokens).await?;
        }
        Ok(())
    }

    async fn clear_unlocked(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(_) | Err(TokenStoreError::Serde(_)) => {
                    self.keyring.clear_unlocked(key)?;
                    self.clear_file_fallback_marker(key).await?;
                    self.file.clear_unlocked(key).await?;
                    let _ = self.clear_clear_tombstone(key).await;
                    Ok(())
                }
                Err(TokenStoreError::KeyringUnavailable(_)) => {
                    self.save_clear_tombstone_and_clear_marker(key, &AutoClearTombstone::Any)
                        .await?;
                    self.clear_file_after_tombstone_commit(key).await;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        #[cfg(not(feature = "keyring"))]
        {
            self.file.clear_unlocked(key).await?;
            Ok(())
        }
    }

    #[cfg(feature = "keyring")]
    async fn save_file_shadow_if_current_or_unreadable(
        &self,
        key: &TokenKey,
        replacement: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.file.save_unlocked(key, replacement).await
    }

    #[cfg(feature = "keyring")]
    async fn save_file_fallback_material(
        &self,
        key: &TokenKey,
        previous_file_tokens: Option<&PersistedTokens>,
        replacement: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.file.save_unlocked(key, replacement).await?;
        if let Err(marker_error) = self.save_file_fallback_marker(key, replacement).await {
            let rollback = match previous_file_tokens {
                Some(previous) => self.file.save_unlocked(key, previous).await,
                None => self.file.clear_unlocked(key).await,
            };
            return match rollback {
                Ok(()) => Err(marker_error),
                Err(rollback_error) => Err(TokenStoreError::Unavailable(format!(
                    "file fallback marker update failed after file save: {marker_error}; \
                     file rollback failed: {rollback_error}"
                ))),
            };
        }
        let _ = self.clear_clear_tombstone(key).await;
        Ok(())
    }

    #[cfg(feature = "keyring")]
    async fn reconcile_keyring_load(
        &self,
        key: &TokenKey,
        keyring_tokens: PersistedTokens,
    ) -> Result<PersistedTokens, TokenStoreError> {
        if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
            self.save_keyring_and_file_shadow(key, Some(&keyring_tokens), &file_tokens)
                .await?;
            return Ok(file_tokens);
        }

        match self.file.load_unlocked(key).await {
            Ok(Some(file_tokens)) if file_tokens == keyring_tokens => Ok(keyring_tokens),
            Ok(Some(file_tokens)) if has_auth_lease_binding(&file_tokens, key) => {
                Err(TokenStoreError::Unavailable(
                    "divergent auth lease material exists in file and keyring without a file fallback marker"
                        .into(),
                ))
            }
            Ok(Some(_) | None) | Err(_) => {
                self.save_file_shadow_if_current_or_unreadable(key, &keyring_tokens)
                    .await?;
                Ok(keyring_tokens)
            }
        }
    }

    #[cfg(feature = "keyring")]
    async fn save_keyring_and_file_shadow(
        &self,
        key: &TokenKey,
        previous_keyring_tokens: Option<&PersistedTokens>,
        replacement: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        let fallback_marker = self.load_file_fallback_marker(key).await?;
        let fallback_marker_matches_replacement = fallback_marker
            .as_ref()
            .is_some_and(|marker| marker.matches_tokens(replacement));
        let pending_clear_tombstone = self.load_clear_tombstone(key).await?;
        let previous_file_fallback_tokens = if fallback_marker.is_some() {
            self.load_marked_file_fallback(key).await?
        } else {
            None
        };
        let previous_file_tokens = if pending_clear_tombstone.is_some() {
            if fallback_marker.is_some() {
                previous_file_fallback_tokens.clone()
            } else {
                self.file.load_unlocked(key).await?
            }
        } else {
            previous_file_fallback_tokens.clone()
        };
        self.keyring.save_unlocked(key, replacement)?;
        match self
            .save_file_shadow_if_current_or_unreadable(key, replacement)
            .await
        {
            Ok(()) => {
                let mut fallback_marker_removed = false;
                if !fallback_marker_matches_replacement && fallback_marker.is_some() {
                    match self.clear_file_fallback_marker(key).await {
                        Ok(()) => {
                            fallback_marker_removed = true;
                        }
                        Err(marker_error) => {
                            let file_rollback_result =
                                if let Some(previous) = previous_file_fallback_tokens.as_ref() {
                                    self.file.save_unlocked(key, previous).await
                                } else {
                                    Ok(())
                                };
                            let keyring_rollback =
                                self.keyring_rollback(key, previous_keyring_tokens);
                            return match (file_rollback_result, keyring_rollback) {
                                (Ok(()), Ok(())) => Err(marker_error),
                                (Err(file_rollback_error), Ok(())) => {
                                    Err(TokenStoreError::Unavailable(format!(
                                        "file fallback marker cleanup failed after keyring save: \
                                         {marker_error}; file rollback failed: {file_rollback_error}"
                                    )))
                                }
                                (Ok(()), Err(rollback_error)) => {
                                    Err(TokenStoreError::Unavailable(format!(
                                        "file fallback marker cleanup failed after keyring save: \
                                         {marker_error}; keyring rollback failed: {rollback_error}"
                                    )))
                                }
                                (Err(file_rollback_error), Err(rollback_error)) => {
                                    Err(TokenStoreError::Unavailable(format!(
                                        "file fallback marker cleanup failed after keyring save: \
                                         {marker_error}; file rollback failed: {file_rollback_error}; \
                                         keyring rollback failed: {rollback_error}"
                                    )))
                                }
                            };
                        }
                    }
                }
                let tombstone_cleanup_result = if pending_clear_tombstone.is_some() {
                    self.clear_clear_tombstone(key).await
                } else {
                    Ok(())
                };
                if let Err(tombstone_error) = tombstone_cleanup_result {
                    let file_rollback_result = if let Some(previous) = previous_file_tokens.as_ref()
                    {
                        self.file.save_unlocked(key, previous).await
                    } else {
                        self.file.clear_unlocked(key).await
                    };
                    let marker_restore_result = if fallback_marker_removed {
                        if let Some(previous) = previous_file_fallback_tokens.as_ref() {
                            self.save_file_fallback_marker(key, previous).await
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    };
                    let keyring_rollback = self.keyring_rollback(key, previous_keyring_tokens);
                    let mut rollback_errors = Vec::new();
                    if let Err(file_rollback_error) = file_rollback_result {
                        rollback_errors
                            .push(format!("file rollback failed: {file_rollback_error}"));
                    }
                    if let Err(marker_restore_error) = marker_restore_result {
                        rollback_errors.push(format!(
                            "fallback marker restore failed: {marker_restore_error}"
                        ));
                    }
                    if let Err(rollback_error) = keyring_rollback {
                        rollback_errors.push(format!("keyring rollback failed: {rollback_error}"));
                    }
                    if rollback_errors.is_empty() {
                        return Err(tombstone_error);
                    }
                    return Err(TokenStoreError::Unavailable(format!(
                        "clear tombstone cleanup failed after keyring save: \
                         {tombstone_error}; {}",
                        rollback_errors.join("; ")
                    )));
                }
                if fallback_marker_matches_replacement {
                    let _ = self.clear_file_fallback_marker(key).await;
                }
                Ok(())
            }
            Err(shadow_error) => {
                let rollback = match previous_keyring_tokens {
                    Some(previous) => self.keyring.save_unlocked(key, previous),
                    None => self.keyring.clear_unlocked(key),
                };
                match rollback {
                    Ok(()) => Err(shadow_error),
                    Err(rollback_error) => Err(TokenStoreError::Unavailable(format!(
                        "file shadow update failed after keyring save: {shadow_error}; \
                         keyring rollback failed: {rollback_error}"
                    ))),
                }
            }
        }
    }

    #[cfg(feature = "keyring")]
    async fn clear_file_shadow_if_current_or_unreadable(
        &self,
        key: &TokenKey,
        _expected: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.file.clear_unlocked(key).await
    }
}

#[cfg(feature = "keyring")]
fn has_auth_lease_binding(tokens: &PersistedTokens, key: &TokenKey) -> bool {
    tokens
        .auth_lease
        .as_ref()
        .is_some_and(|binding| binding.token_key == *key)
}

#[cfg(feature = "keyring")]
fn has_finalized_auth_lease_binding(tokens: &PersistedTokens, key: &TokenKey) -> bool {
    tokens.auth_lease.as_ref().is_some_and(|binding| {
        binding.token_key == *key && binding.pending_owner_generation.is_none()
    })
}

#[cfg(feature = "keyring")]
async fn clear_sidecar_path(path: &Path) -> Result<(), TokenStoreError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => {
            tokio::fs::remove_dir_all(path).await?;
            Ok(())
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                match tokio::fs::metadata(path).await {
                    Ok(metadata) if metadata.is_dir() => {
                        tokio::fs::remove_dir_all(path).await?;
                        return Ok(());
                    }
                    Ok(_) | Err(_) => {}
                }
            }
            Err(TokenStoreError::from(e))
        }
    }
}

#[async_trait]
impl TokenStore for AutoTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;
        self.load_unlocked(key).await
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;
        self.save_unlocked(key, tokens).await
    }

    async fn save_if_current(
        &self,
        key: &TokenKey,
        expected: &PersistedTokens,
        replacement: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;

        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(mut keyring_tokens) => {
                    if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
                        if &file_tokens != expected {
                            return Ok(false);
                        }
                        self.save_keyring_and_file_shadow(
                            key,
                            keyring_tokens.as_ref(),
                            replacement,
                        )
                        .await?;
                        return Ok(true);
                    }
                    self.reconcile_clear_tombstone_before_cas(key, &mut keyring_tokens)
                        .await?;
                    let Some(keyring_tokens) = keyring_tokens else {
                        if self.load_unmarked_file_material(key).await?.as_ref() != Some(expected) {
                            return Ok(false);
                        }
                        self.save_keyring_and_file_shadow(key, None, replacement)
                            .await?;
                        return Ok(true);
                    };
                    if &keyring_tokens == expected {
                        self.fail_if_unmarked_divergent_auth_lease_file(key, &keyring_tokens)
                            .await?;
                    }
                    if &keyring_tokens != expected {
                        let file_tokens = self.file.load_unlocked(key).await?;
                        let fallback_marker_matches = self
                            .load_file_fallback_marker(key)
                            .await?
                            .as_ref()
                            .is_some_and(|marker| marker.matches_tokens(expected));
                        if file_tokens.as_ref() == Some(expected) && fallback_marker_matches {
                            self.save_keyring_and_file_shadow(
                                key,
                                Some(&keyring_tokens),
                                replacement,
                            )
                            .await?;
                            return Ok(true);
                        }
                        return Ok(false);
                    }
                    self.save_keyring_and_file_shadow(key, Some(&keyring_tokens), replacement)
                        .await?;
                    return Ok(true);
                }
                Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }

        #[cfg(feature = "keyring")]
        {
            if self
                .load_keyring_unavailable_cas_current(key)
                .await?
                .as_ref()
                != Some(expected)
            {
                return Ok(false);
            }
            self.save_file_fallback_material(key, Some(expected), replacement)
                .await?;
            return Ok(true);
        }
        #[cfg(not(feature = "keyring"))]
        {
            if self.file.load_unlocked(key).await?.as_ref() != Some(expected) {
                return Ok(false);
            }
            self.file.save_unlocked(key, replacement).await?;
            Ok(true)
        }
    }

    async fn save_if_current_optional(
        &self,
        key: &TokenKey,
        expected: Option<&PersistedTokens>,
        replacement: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;

        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(mut keyring_tokens) => {
                    if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
                        if expected != Some(&file_tokens) {
                            return Ok(false);
                        }
                        self.save_keyring_and_file_shadow(
                            key,
                            keyring_tokens.as_ref(),
                            replacement,
                        )
                        .await?;
                        return Ok(true);
                    }
                    self.reconcile_clear_tombstone_before_cas(key, &mut keyring_tokens)
                        .await?;
                    let Some(keyring_tokens) = keyring_tokens else {
                        if self.load_unmarked_file_material(key).await?.as_ref() != expected {
                            return Ok(false);
                        }
                        self.save_keyring_and_file_shadow(key, None, replacement)
                            .await?;
                        return Ok(true);
                    };
                    if Some(&keyring_tokens) == expected {
                        self.fail_if_unmarked_divergent_auth_lease_file(key, &keyring_tokens)
                            .await?;
                    }
                    if Some(&keyring_tokens) != expected {
                        let file_can_drive_replacement = if let Some(expected_tokens) = expected {
                            let file_tokens = self.file.load_unlocked(key).await?;
                            let fallback_marker_matches = self
                                .load_file_fallback_marker(key)
                                .await?
                                .as_ref()
                                .is_some_and(|marker| marker.matches_tokens(expected_tokens));
                            file_tokens.as_ref() == Some(expected_tokens) && fallback_marker_matches
                        } else {
                            false
                        };
                        if file_can_drive_replacement {
                            self.save_keyring_and_file_shadow(
                                key,
                                Some(&keyring_tokens),
                                replacement,
                            )
                            .await?;
                            return Ok(true);
                        }
                        return Ok(false);
                    }
                    self.save_keyring_and_file_shadow(key, Some(&keyring_tokens), replacement)
                        .await?;
                    return Ok(true);
                }
                Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }

        #[cfg(feature = "keyring")]
        {
            if let Some(expected_tokens) = expected {
                if self
                    .load_keyring_unavailable_cas_current(key)
                    .await?
                    .as_ref()
                    != Some(expected_tokens)
                {
                    return Ok(false);
                }
            } else if self
                .load_keyring_unavailable_initial_cas_current(key)
                .await?
                .is_some()
            {
                return Ok(false);
            }
            self.save_file_fallback_material(key, expected, replacement)
                .await?;
            return Ok(true);
        }
        #[cfg(not(feature = "keyring"))]
        {
            if self.file.load_unlocked(key).await?.as_ref() != expected {
                return Ok(false);
            }
            self.file.save_unlocked(key, replacement).await?;
            Ok(true)
        }
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;
        self.clear_unlocked(key).await
    }

    async fn clear_if_unreadable(&self, key: &TokenKey) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;

        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(_) => {}
                Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(TokenStoreError::Serde(_)) => {
                    self.clear_unlocked(key).await?;
                    return Ok(true);
                }
                Err(e) => return Err(e),
            }
            match self.load_marked_file_fallback(key).await {
                Ok(Some(_)) => return Ok(false),
                Ok(None) => {}
                Err(TokenStoreError::Unavailable(_reason) | TokenStoreError::Serde(_reason)) => {
                    self.clear_unlocked(key).await?;
                    return Ok(true);
                }
                Err(e) => return Err(e),
            }
            match self.file.load_unlocked(key).await {
                Ok(_) => return Ok(false),
                Err(TokenStoreError::Serde(_)) => {
                    self.clear_unlocked(key).await?;
                    return Ok(true);
                }
                Err(e) => return Err(e),
            }
        }

        #[cfg(not(feature = "keyring"))]
        {
            match self.file.load_unlocked(key).await {
                Ok(_) => Ok(false),
                Err(TokenStoreError::Serde(_)) => {
                    self.file.clear_unlocked(key).await?;
                    Ok(true)
                }
                Err(e) => Err(e),
            }
        }
    }

    async fn clear_if_current(
        &self,
        key: &TokenKey,
        expected: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;

        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(mut keyring_tokens) => {
                    if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
                        if &file_tokens != expected {
                            return Ok(false);
                        }
                        self.keyring.clear_unlocked(key)?;
                        self.file.clear_unlocked(key).await?;
                        let _ = self.clear_file_fallback_marker(key).await;
                        let _ = self.clear_clear_tombstone(key).await;
                        return Ok(true);
                    }
                    self.reconcile_clear_tombstone_before_cas(key, &mut keyring_tokens)
                        .await?;
                    let Some(keyring_tokens) = keyring_tokens else {
                        if self.load_unmarked_file_material(key).await?.as_ref() != Some(expected) {
                            return Ok(false);
                        }
                        self.clear_clear_tombstone(key).await?;
                        self.clear_file_fallback_marker(key).await?;
                        self.file.clear_unlocked(key).await?;
                        return Ok(true);
                    };
                    if &keyring_tokens == expected {
                        self.fail_if_unmarked_divergent_auth_lease_file(key, &keyring_tokens)
                            .await?;
                    }
                    let mut cleared = false;
                    if &keyring_tokens == expected {
                        self.keyring.clear_unlocked(key)?;
                        self.clear_file_shadow_if_current_or_unreadable(key, expected)
                            .await?;
                        let _ = self.clear_file_fallback_marker(key).await;
                        let _ = self.clear_clear_tombstone(key).await;
                        cleared = true;
                    } else if has_auth_lease_binding(expected, key) {
                        let () = self
                            .clear_file_shadow_if_current_or_unreadable(key, expected)
                            .await?;
                    }
                    return Ok(cleared);
                }
                Err(TokenStoreError::KeyringUnavailable(_)) => {
                    if let Some(file_tokens) = self.load_marked_file_fallback(key).await? {
                        if &file_tokens != expected {
                            return Ok(false);
                        }
                        let tombstone = AutoClearTombstone::from_tokens(expected).ok_or_else(|| {
                            TokenStoreError::KeyringUnavailable(
                                "conditional clear during keyring outage requires auth lease binding"
                                    .into(),
                            )
                        })?;
                        self.save_clear_tombstone_and_clear_marker(key, &tombstone)
                            .await?;
                        self.clear_file_after_tombstone_commit(key).await;
                        return Ok(true);
                    }
                    if self.load_clear_tombstone(key).await?.is_none() {
                        let file_tokens = self.file.load_unlocked(key).await?;
                        if file_tokens.as_ref() == Some(expected)
                            && has_auth_lease_binding(expected, key)
                        {
                            return Err(TokenStoreError::Unavailable(
                                "unmarked auth lease material cannot authorize conditional clear during keyring outage"
                                    .into(),
                            ));
                        }
                        if file_tokens.as_ref() != Some(expected)
                            && !has_finalized_auth_lease_binding(expected, key)
                        {
                            return Ok(false);
                        }
                    }
                    let tombstone = AutoClearTombstone::from_tokens(expected).ok_or_else(|| {
                        TokenStoreError::KeyringUnavailable(
                            "conditional clear during keyring outage requires auth lease binding"
                                .into(),
                        )
                    })?;
                    self.save_clear_tombstone_and_clear_marker(key, &tombstone)
                        .await?;
                    let _ = self.clear_file_matching_tombstone(key, &tombstone).await;
                    return Ok(true);
                }
                Err(e) => return Err(e),
            }
        }

        #[cfg(not(feature = "keyring"))]
        {
            if self.file.load_unlocked(key).await?.as_ref() != Some(expected) {
                return Ok(false);
            }
            self.file.clear_unlocked(key).await?;
            Ok(true)
        }
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        let mut combined = Vec::new();
        #[cfg(feature = "keyring")]
        {
            match self.keyring.list().await {
                Ok(mut v) => combined.append(&mut v),
                Err(TokenStoreError::KeyringUnavailable(_) | TokenStoreError::Unavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let mut file_keys = self.file.list().await?;
        combined.append(&mut file_keys);
        combined.sort();
        combined.dedup();
        Ok(combined)
    }

    fn backend_name(&self) -> &'static str {
        "auto"
    }
}

#[cfg(all(test, feature = "keyring"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryKeyringBackend {
        tokens: Mutex<BTreeMap<TokenKey, PersistedTokens>>,
        unavailable: Mutex<bool>,
        save_calls: Mutex<usize>,
        fail_save_call: Mutex<Option<usize>>,
        clear_calls: Mutex<usize>,
        fail_clear_call: Mutex<Option<usize>>,
        list_unavailable: Mutex<bool>,
    }

    impl MemoryKeyringBackend {
        fn insert(&self, key: &TokenKey, tokens: PersistedTokens) {
            self.tokens
                .lock()
                .expect("memory keyring lock")
                .insert(key.clone(), tokens);
        }

        fn set_unavailable(&self, unavailable: bool) {
            *self.unavailable.lock().expect("memory keyring lock") = unavailable;
        }

        fn fail_save_call(&self, call: usize) {
            *self.fail_save_call.lock().expect("memory keyring lock") = Some(call);
        }

        fn fail_clear_call(&self, call: usize) {
            *self.fail_clear_call.lock().expect("memory keyring lock") = Some(call);
        }

        fn set_list_unavailable(&self, unavailable: bool) {
            *self.list_unavailable.lock().expect("memory keyring lock") = unavailable;
        }
    }

    #[async_trait]
    impl AutoKeyringBackend for MemoryKeyringBackend {
        fn load_unlocked(
            &self,
            key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, TokenStoreError> {
            if *self.unavailable.lock().expect("memory keyring lock") {
                return Err(TokenStoreError::KeyringUnavailable(
                    "injected keyring outage".into(),
                ));
            }
            Ok(self
                .tokens
                .lock()
                .expect("memory keyring lock")
                .get(key)
                .cloned())
        }

        fn save_unlocked(
            &self,
            key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            if *self.unavailable.lock().expect("memory keyring lock") {
                return Err(TokenStoreError::KeyringUnavailable(
                    "injected keyring outage".into(),
                ));
            }
            let mut save_calls = self.save_calls.lock().expect("memory keyring lock");
            *save_calls += 1;
            if self
                .fail_save_call
                .lock()
                .expect("memory keyring lock")
                .is_some_and(|fail_call| fail_call == *save_calls)
            {
                return Err(TokenStoreError::KeyringUnavailable(
                    "injected keyring save failure".into(),
                ));
            }
            self.tokens
                .lock()
                .expect("memory keyring lock")
                .insert(key.clone(), tokens.clone());
            Ok(())
        }

        fn clear_unlocked(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
            if *self.unavailable.lock().expect("memory keyring lock") {
                return Err(TokenStoreError::KeyringUnavailable(
                    "injected keyring outage".into(),
                ));
            }
            let mut clear_calls = self.clear_calls.lock().expect("memory keyring lock");
            *clear_calls += 1;
            if self
                .fail_clear_call
                .lock()
                .expect("memory keyring lock")
                .is_some_and(|fail_call| fail_call == *clear_calls)
            {
                return Err(TokenStoreError::KeyringUnavailable(
                    "injected keyring clear failure".into(),
                ));
            }
            self.tokens.lock().expect("memory keyring lock").remove(key);
            Ok(())
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            if *self.list_unavailable.lock().expect("memory keyring lock") {
                return Err(TokenStoreError::Unavailable(
                    "injected keyring list unsupported".into(),
                ));
            }
            Ok(self
                .tokens
                .lock()
                .expect("memory keyring lock")
                .keys()
                .cloned()
                .collect())
        }
    }

    fn key() -> TokenKey {
        TokenKey::parse("dev", "x").expect("valid test key")
    }

    fn token(value: &str) -> PersistedTokens {
        PersistedTokens::api_key(value)
    }

    fn file_shadow_path(root: &Path, key: &TokenKey) -> PathBuf {
        root.join(key.realm.as_str())
            .join(format!("{}.json", key.binding.as_str()))
    }

    fn write_malformed_shadow(root: &Path, key: &TokenKey) {
        let path = file_shadow_path(root, key);
        std::fs::create_dir_all(path.parent().expect("token path has parent"))
            .expect("create token dir");
        std::fs::write(path, "{ malformed token json").expect("write malformed token");
    }

    #[cfg(unix)]
    fn make_unclearable_sidecar_dir(path: &Path) -> std::fs::Permissions {
        use std::os::unix::fs::PermissionsExt;

        std::fs::create_dir_all(path).expect("create sidecar dir");
        std::fs::write(path.join("child"), "x").expect("write sidecar child");
        let original_permissions = std::fs::metadata(path)
            .expect("sidecar dir metadata")
            .permissions();
        let mut readonly_permissions = original_permissions.clone();
        readonly_permissions.set_mode(0o500);
        std::fs::set_permissions(path, readonly_permissions).expect("make sidecar dir unclearable");
        original_permissions
    }

    #[tokio::test]
    async fn keyring_save_shadow_replaces_malformed_file_without_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AutoTokenStore::new(temp.path().to_path_buf(), "meerkat-test-shadow-save");
        let key = key();
        let replacement = token("replacement");

        write_malformed_shadow(temp.path(), &key);
        store
            .save_file_shadow_if_current_or_unreadable(&key, &replacement)
            .await
            .expect("shadow save");

        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn keyring_clear_shadow_removes_malformed_file_before_primary_clear() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AutoTokenStore::new(temp.path().to_path_buf(), "meerkat-test-shadow-clear");
        let key = key();
        let expected = token("expected");

        write_malformed_shadow(temp.path(), &key);
        store
            .clear_file_shadow_if_current_or_unreadable(&key, &expected)
            .await
            .expect("malformed shadow clear");

        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            None
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_unreadable_tombstones_keyring_during_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, expected.clone());
        write_malformed_shadow(temp.path(), &key);
        keyring.set_unavailable(true);

        let cleared = store
            .clear_if_unreadable(&key)
            .await
            .expect("malformed file shadow cleanup during keyring outage");

        assert!(cleared);
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_some(),
            "malformed file-shadow cleanup during keyring outage must tombstone hidden keyring material"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            None,
            "malformed file shadow should still be removed"
        );
        keyring.set_unavailable(false);
        assert_eq!(
            store.load(&key).await.expect("load after keyring returns"),
            None,
            "clear tombstone must suppress hidden keyring material after the outage"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "hidden keyring material must be cleared when the tombstone is reconciled"
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_unreadable_tombstones_marked_fallback_during_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &file_fallback)
            .await
            .expect("seed file fallback");
        store
            .save_file_fallback_marker(&key, &file_fallback)
            .await
            .expect("seed fallback marker");
        write_malformed_shadow(temp.path(), &key);
        keyring.set_unavailable(true);

        let cleared = store
            .clear_if_unreadable(&key)
            .await
            .expect("marked fallback cleanup during keyring outage");

        assert!(cleared);
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_none(),
            "fallback marker must be removed after a durable outage tombstone is written"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            None,
            "malformed fallback material should be removed"
        );
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_some(),
            "outage clear must tombstone hidden keyring material before removing the fallback marker"
        );
        keyring.set_unavailable(false);
        assert_eq!(
            store.load(&key).await.expect("load after keyring returns"),
            None,
            "clear tombstone must suppress hidden keyring material after marked fallback cleanup"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "stale keyring material must be cleared by tombstone reconciliation"
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_unreadable_clears_mismatched_file_fallback_marker_before_keyring() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 0);
        let file_tokens = token("file").with_auth_lease_binding(key.clone(), 1);
        let marker_tokens = token("marker").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, stale_keyring);
        store
            .file
            .save_unlocked(&key, &file_tokens)
            .await
            .expect("seed file material");
        store
            .save_file_fallback_marker(&key, &marker_tokens)
            .await
            .expect("seed mismatched fallback marker");

        let cleared = store
            .clear_if_unreadable(&key)
            .await
            .expect("clear fail-closed fallback state");

        assert!(cleared);
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            None
        );
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_none(),
            "mismatched fallback marker must be removed with unreadable material"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "stale keyring material must not outrank fail-closed fallback marker cleanup"
        );
    }

    #[tokio::test]
    async fn keyring_shadow_helpers_replace_or_clear_different_readable_file_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AutoTokenStore::new(temp.path().to_path_buf(), "meerkat-test-shadow-stale");
        let key = key();
        let expected = token("expected");
        let other = token("other");
        let replacement = token("replacement");

        store
            .file
            .save_unlocked(&key, &other)
            .await
            .expect("seed file shadow");
        store
            .save_file_shadow_if_current_or_unreadable(&key, &replacement)
            .await
            .expect("shadow save");
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(replacement)
        );

        store
            .file
            .save_unlocked(&key, &other)
            .await
            .expect("seed divergent file shadow");
        store
            .clear_file_shadow_if_current_or_unreadable(&key, &expected)
            .await
            .expect("different material is cleared");
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            None
        );
    }

    #[tokio::test]
    async fn keyring_list_falls_back_to_file_when_keyring_enumeration_is_unavailable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed file shadow");
        keyring.set_list_unavailable(true);

        let listed = store.list().await.expect("list falls back to file");

        assert_eq!(listed, vec![key]);
    }

    #[tokio::test]
    async fn keyring_optional_initial_save_writes_keyring_and_file_shadow_when_combined_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let replacement = token("replacement");

        let saved = store
            .save_if_current_optional(&key, None, &replacement)
            .await
            .expect("initial conditional save");

        assert!(saved);
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement.clone())
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn keyring_optional_initial_save_rejects_existing_file_fallback_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let other = token("other");
        let replacement = token("replacement");
        store
            .file
            .save_unlocked(&key, &other)
            .await
            .expect("seed file fallback");

        let saved = store
            .save_if_current_optional(&key, None, &replacement)
            .await
            .expect("initial conditional save");

        assert!(!saved);
        assert_eq!(keyring.load_unlocked(&key).expect("keyring load"), None);
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(other)
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_promotes_matching_file_fallback_to_keyring() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected");
        let replacement = token("replacement");
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed file fallback");

        let saved = store
            .save_if_current(&key, &expected, &replacement)
            .await
            .expect("conditional save");

        assert!(saved);
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement.clone())
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_replaces_divergent_file_shadow() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected");
        let other = token("other-file-shadow");
        let replacement = token("replacement");
        keyring
            .save_unlocked(&key, &expected)
            .expect("seed keyring");
        store
            .file
            .save_unlocked(&key, &other)
            .await
            .expect("seed divergent file shadow");

        let saved = store
            .save_if_current(&key, &expected, &replacement)
            .await
            .expect("conditional save");

        assert!(saved);
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement.clone())
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn keyring_load_promotes_newer_file_material_after_transient_keyring_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);

        let saved = store
            .save_if_current(&key, &expected, &replacement)
            .await
            .expect("file fallback save during keyring outage");
        keyring.set_unavailable(false);
        let loaded = store.load(&key).await.expect("load after keyring returns");

        assert!(saved);
        assert_eq!(loaded, Some(replacement.clone()));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement),
            "fresh file material must be promoted instead of hidden behind stale keyring material"
        );
    }

    #[tokio::test]
    async fn keyring_load_promotes_file_material_with_lower_restart_generation_after_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let previous_run = token("previous-run").with_auth_lease_binding(key.clone(), 100);
        let replacement_after_restart =
            token("replacement-after-restart").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, previous_run.clone());
        store
            .file
            .save_unlocked(&key, &previous_run)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &previous_run)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);

        let saved = store
            .save_if_current(&key, &previous_run, &replacement_after_restart)
            .await
            .expect("file fallback save during keyring outage");
        keyring.set_unavailable(false);
        let loaded = store.load(&key).await.expect("load after keyring returns");

        assert!(saved);
        assert_eq!(loaded, Some(replacement_after_restart.clone()));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement_after_restart),
            "file fallback material written after restart must not lose to a stale higher generation from a previous process"
        );
    }

    #[tokio::test]
    async fn keyring_load_rejects_unmarked_divergent_finalized_file_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let keyring_tokens = token("keyring").with_auth_lease_binding(key.clone(), 1);
        let file_tokens = token("file").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, keyring_tokens.clone());
        store
            .file
            .save_unlocked(&key, &file_tokens)
            .await
            .expect("seed unmarked divergent file material");

        let err = store
            .load(&key)
            .await
            .expect_err("unmarked divergent finalized material must fail closed");

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(keyring_tokens),
            "unmarked file material must not replace keyring material by generation ordering"
        );
    }

    #[tokio::test]
    async fn keyring_load_rejects_finalized_keyring_over_unmarked_pending_file_shadow() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let pending_file = token("fresh").with_auth_pending_owner_binding(key.clone(), 0);
        let finalized_keyring = token("fresh").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, finalized_keyring.clone());
        store
            .file
            .save_unlocked(&key, &pending_file)
            .await
            .expect("seed unmarked pending file material");

        let err = store.load(&key).await.expect_err(
            "finalized keyring material must not override unmarked pending file material",
        );

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(finalized_keyring),
            "unmarked pending file material must not be overwritten by finalized keyring material"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(pending_file)
        );
    }

    #[tokio::test]
    async fn keyring_load_fails_closed_when_keyring_unavailable_and_no_file_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let hidden_keyring = token("hidden-keyring").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, hidden_keyring.clone());
        keyring.set_unavailable(true);

        let err = store
            .load(&key)
            .await
            .expect_err("hidden keyring state must not look empty during outage");
        keyring.set_unavailable(false);

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert_eq!(
            store.load(&key).await.expect("load after keyring returns"),
            Some(hidden_keyring),
            "hidden keyring material remains durable and must not be released as absent"
        );
    }

    #[tokio::test]
    async fn keyring_load_rejects_finalized_file_after_cleanup_misses_stale_keyring() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring");
        let finalized_file = token("fresh").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &finalized_file)
            .await
            .expect("seed finalized file material");

        let cleared = store
            .clear_if_current(&key, &finalized_file)
            .await
            .expect("cleanup attempt over stale keyring");
        keyring.set_unavailable(true);
        let load_during_keyring_outage = store.load(&key).await;
        keyring.set_unavailable(false);

        assert!(!cleared);
        assert!(
            matches!(
                load_during_keyring_outage,
                Err(TokenStoreError::KeyringUnavailable(_))
            ),
            "finalized file material from a reported failed save must not be bootstrappable during keyring outage"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(stale_keyring),
            "stale keyring material remains physically present but non-authoritative"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            None,
            "failed-finalization cleanup must clear the leaked finalized file shadow even when keyring holds different material"
        );
    }

    #[tokio::test]
    async fn keyring_load_tombstone_reconciliation_rejects_nonmatching_auth_lease_file_after_keyring_clear()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let tombstoned_keyring = token("tombstoned").with_auth_lease_binding(key.clone(), 1);
        let different_file = token("different-file").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, tombstoned_keyring.clone());
        store
            .file
            .save_unlocked(&key, &different_file)
            .await
            .expect("seed divergent file material");
        store
            .save_clear_tombstone(
                &key,
                &AutoClearTombstone::from_tokens(&tombstoned_keyring).expect("lease tombstone"),
            )
            .await
            .expect("seed clear tombstone");

        let first_load = store.load(&key).await;
        let second_load = store.load(&key).await;

        assert!(
            matches!(first_load, Err(TokenStoreError::Unavailable(_))),
            "tombstone reconciliation must not return divergent auth-lease file material"
        );
        assert!(
            matches!(second_load, Err(TokenStoreError::Unavailable(_))),
            "divergent unmarked auth-lease file material must stay fail-closed after tombstone consumption"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "matching tombstone must still clear the stale keyring material"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(different_file)
        );
    }

    #[tokio::test]
    async fn keyring_load_tombstone_reconciliation_rejects_nonmatching_auth_lease_file_without_keyring_material()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let tombstoned = token("tombstoned").with_auth_lease_binding(key.clone(), 1);
        let different_file = token("different-file").with_auth_lease_binding(key.clone(), 2);
        store
            .file
            .save_unlocked(&key, &different_file)
            .await
            .expect("seed divergent file material");
        store
            .save_clear_tombstone(
                &key,
                &AutoClearTombstone::from_tokens(&tombstoned).expect("lease tombstone"),
            )
            .await
            .expect("seed clear tombstone");

        let first_load = store.load(&key).await;
        let second_load = store.load(&key).await;

        assert!(
            matches!(first_load, Err(TokenStoreError::Unavailable(_))),
            "tombstone reconciliation without keyring material must not return divergent auth-lease file material"
        );
        assert!(
            matches!(second_load, Err(TokenStoreError::Unavailable(_))),
            "divergent unmarked auth-lease file material must remain fail-closed"
        );
        assert_eq!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load"),
            None,
            "nonmatching tombstone can be discarded only after file material remains fail-closed"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(different_file)
        );
    }

    #[tokio::test]
    async fn keyring_load_tombstone_reconciliation_rejects_nonmatching_auth_lease_file_during_keyring_outage()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let tombstoned = token("tombstoned").with_auth_lease_binding(key.clone(), 1);
        let different_file = token("different-file").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, tombstoned.clone());
        store
            .file
            .save_unlocked(&key, &different_file)
            .await
            .expect("seed divergent file material");
        store
            .save_clear_tombstone(
                &key,
                &AutoClearTombstone::from_tokens(&tombstoned).expect("lease tombstone"),
            )
            .await
            .expect("seed clear tombstone");
        keyring.set_unavailable(true);

        let load_during_outage = store.load(&key).await;
        keyring.set_unavailable(false);
        let load_after_keyring_returns = store.load(&key).await;

        assert!(
            matches!(load_during_outage, Err(TokenStoreError::Unavailable(_))),
            "keyring-outage tombstone reconciliation must not return divergent auth-lease file material"
        );
        assert!(
            matches!(
                load_after_keyring_returns,
                Err(TokenStoreError::Unavailable(_))
            ),
            "pending tombstone plus divergent file material must stay fail-closed after keyring returns"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "matching tombstone must clear keyring material when keyring returns"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(different_file)
        );
    }

    async fn seed_nonmatching_tombstone_reconciliation_cas_material(
        store: &AutoTokenStore,
        keyring: &Arc<MemoryKeyringBackend>,
        key: &TokenKey,
    ) -> (PersistedTokens, PersistedTokens, PersistedTokens) {
        let tombstoned_keyring = token("tombstoned").with_auth_lease_binding(key.clone(), 1);
        let different_file = token("different-file").with_auth_lease_binding(key.clone(), 2);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 3);
        keyring.insert(key, tombstoned_keyring.clone());
        store
            .file
            .save_unlocked(key, &different_file)
            .await
            .expect("seed divergent file material");
        store
            .save_clear_tombstone(
                key,
                &AutoClearTombstone::from_tokens(&tombstoned_keyring).expect("lease tombstone"),
            )
            .await
            .expect("seed clear tombstone");
        (tombstoned_keyring, different_file, replacement)
    }

    #[tokio::test]
    async fn keyring_save_if_current_rejects_file_material_after_nonmatching_tombstone_reconciliation()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let (_tombstoned_keyring, different_file, replacement) =
            seed_nonmatching_tombstone_reconciliation_cas_material(&store, &keyring, &key).await;

        let err = store
            .save_if_current(&key, &different_file, &replacement)
            .await
            .expect_err("CAS must fail closed over divergent unmarked file material after tombstone reconciliation");

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "matching tombstone may be finalized, but divergent file material must not become authoritative"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(different_file),
            "divergent file material must not be replaced by CAS"
        );
        assert_eq!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load"),
            None
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_optional_rejects_file_material_after_nonmatching_tombstone_reconciliation()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let (_tombstoned_keyring, different_file, replacement) =
            seed_nonmatching_tombstone_reconciliation_cas_material(&store, &keyring, &key).await;

        let err = store
            .save_if_current_optional(&key, Some(&different_file), &replacement)
            .await
            .expect_err("optional CAS must fail closed over divergent unmarked file material after tombstone reconciliation");

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "matching tombstone may be finalized, but divergent file material must not become authoritative"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(different_file),
            "divergent file material must not be replaced by optional CAS"
        );
        assert_eq!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load"),
            None
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_current_rejects_file_material_after_nonmatching_tombstone_reconciliation()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let (_tombstoned_keyring, different_file, _replacement) =
            seed_nonmatching_tombstone_reconciliation_cas_material(&store, &keyring, &key).await;

        let err = store
            .clear_if_current(&key, &different_file)
            .await
            .expect_err("conditional clear must fail closed over divergent unmarked file material after tombstone reconciliation");

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "matching tombstone may be finalized, but divergent file material must not become authoritative"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(different_file),
            "divergent file material must not be cleared by CAS"
        );
        assert_eq!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load"),
            None
        );
    }

    async fn seed_unmarked_auth_lease_file_during_keyring_outage(
        store: &AutoTokenStore,
        keyring: &Arc<MemoryKeyringBackend>,
        key: &TokenKey,
    ) -> (PersistedTokens, PersistedTokens) {
        let file_tokens = token("file").with_auth_lease_binding(key.clone(), 1);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 2);
        store
            .file
            .save_unlocked(key, &file_tokens)
            .await
            .expect("seed unmarked auth-lease file material");
        keyring.set_unavailable(true);
        (file_tokens, replacement)
    }

    #[tokio::test]
    async fn keyring_save_if_current_rejects_unmarked_auth_lease_file_during_keyring_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let (file_tokens, replacement) =
            seed_unmarked_auth_lease_file_during_keyring_outage(&store, &keyring, &key).await;

        let err = store
            .save_if_current(&key, &file_tokens, &replacement)
            .await
            .expect_err(
                "keyring-outage CAS must fail closed over unmarked auth-lease file material",
            );
        keyring.set_unavailable(false);

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_tokens)
        );
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_none(),
            "failed outage CAS must not mark untrusted file material as fallback truth"
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_optional_rejects_unmarked_auth_lease_file_during_keyring_outage()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let (file_tokens, replacement) =
            seed_unmarked_auth_lease_file_during_keyring_outage(&store, &keyring, &key).await;

        let err = store
            .save_if_current_optional(&key, Some(&file_tokens), &replacement)
            .await
            .expect_err("optional keyring-outage CAS must fail closed over unmarked auth-lease file material");
        keyring.set_unavailable(false);

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_tokens)
        );
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_none(),
            "failed optional outage CAS must not mark untrusted file material as fallback truth"
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_current_rejects_unmarked_auth_lease_file_during_keyring_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let (file_tokens, _replacement) =
            seed_unmarked_auth_lease_file_during_keyring_outage(&store, &keyring, &key).await;

        let err = store.clear_if_current(&key, &file_tokens).await.expect_err(
            "keyring-outage clear CAS must fail closed over unmarked auth-lease file material",
        );
        keyring.set_unavailable(false);

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_tokens)
        );
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_none(),
            "failed outage clear must not tombstone untrusted file material"
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_current_preserves_existing_different_tombstone_during_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let hidden_keyring = token("hidden-keyring").with_auth_lease_binding(key.clone(), 1);
        let pending_file = token("pending-file").with_auth_pending_owner_binding(key.clone(), 0);
        let hidden_tombstone =
            AutoClearTombstone::from_tokens(&hidden_keyring).expect("hidden lease tombstone");
        keyring.insert(&key, hidden_keyring.clone());
        store
            .file
            .save_unlocked(&key, &pending_file)
            .await
            .expect("seed pending file material");
        store
            .save_clear_tombstone(&key, &hidden_tombstone)
            .await
            .expect("seed hidden-keyring tombstone");
        keyring.set_unavailable(true);

        let err = store
            .clear_if_current(&key, &pending_file)
            .await
            .expect_err("outage clear must not overwrite an existing different tombstone");
        keyring.set_unavailable(false);

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert_eq!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load"),
            Some(hidden_tombstone),
            "original hidden-keyring tombstone must remain durable"
        );
        let load_after_keyring_returns = store.load(&key).await;
        assert!(
            matches!(
                load_after_keyring_returns,
                Err(TokenStoreError::Unavailable(_))
            ),
            "pending file material must not hide the tombstoned keyring clear"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "original tombstone must clear hidden keyring material when keyring returns"
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_rejects_unmarked_divergent_finalized_file_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let keyring_tokens = token("keyring").with_auth_lease_binding(key.clone(), 1);
        let file_tokens = token("file").with_auth_lease_binding(key.clone(), 2);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 3);
        keyring.insert(&key, keyring_tokens.clone());
        store
            .file
            .save_unlocked(&key, &file_tokens)
            .await
            .expect("seed unmarked divergent file material");

        let err = store
            .save_if_current(&key, &keyring_tokens, &replacement)
            .await
            .expect_err("CAS must fail closed over unmarked divergent finalized file material");

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(keyring_tokens)
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_tokens)
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_optional_rejects_unmarked_divergent_finalized_file_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let keyring_tokens = token("keyring").with_auth_lease_binding(key.clone(), 1);
        let file_tokens = token("file").with_auth_lease_binding(key.clone(), 2);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 3);
        keyring.insert(&key, keyring_tokens.clone());
        store
            .file
            .save_unlocked(&key, &file_tokens)
            .await
            .expect("seed unmarked divergent file material");

        let err = store
            .save_if_current_optional(&key, Some(&keyring_tokens), &replacement)
            .await
            .expect_err(
                "optional CAS must fail closed over unmarked divergent finalized file material",
            );

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(keyring_tokens)
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_tokens)
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_current_rejects_unmarked_divergent_finalized_file_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let keyring_tokens = token("keyring").with_auth_lease_binding(key.clone(), 1);
        let file_tokens = token("file").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, keyring_tokens.clone());
        store
            .file
            .save_unlocked(&key, &file_tokens)
            .await
            .expect("seed unmarked divergent file material");

        let err = store
            .clear_if_current(&key, &keyring_tokens)
            .await
            .expect_err("conditional clear must fail closed over unmarked divergent finalized file material");

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(keyring_tokens)
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_tokens)
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_rejects_stale_keyring_when_marked_file_fallback_is_current() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 3);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &stale_keyring)
            .await
            .expect("seed matching file shadow");
        store
            .save_file_fallback_marker(&key, &stale_keyring)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .save_if_current(&key, &stale_keyring, &file_fallback)
            .await
            .expect("file fallback save during keyring outage");
        keyring.set_unavailable(false);

        let saved = store
            .save_if_current(&key, &stale_keyring, &replacement)
            .await
            .expect("conditional save with stale keyring expected");
        let loaded = store.load(&key).await.expect("load effective current");

        assert!(!saved);
        assert_eq!(loaded, Some(file_fallback.clone()));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(file_fallback),
            "stale keyring material must not overwrite marked file fallback current material"
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_optional_rejects_stale_keyring_when_marked_file_fallback_is_current()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 3);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &stale_keyring)
            .await
            .expect("seed matching file shadow");
        store
            .save_file_fallback_marker(&key, &stale_keyring)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .save_if_current(&key, &stale_keyring, &file_fallback)
            .await
            .expect("file fallback save during keyring outage");
        keyring.set_unavailable(false);

        let saved = store
            .save_if_current_optional(&key, Some(&stale_keyring), &replacement)
            .await
            .expect("optional conditional save with stale keyring expected");
        let loaded = store.load(&key).await.expect("load effective current");

        assert!(!saved);
        assert_eq!(loaded, Some(file_fallback.clone()));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(file_fallback),
            "optional CAS must treat the marked file fallback as current material"
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_current_rejects_stale_keyring_when_marked_file_fallback_is_current() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &stale_keyring)
            .await
            .expect("seed matching file shadow");
        store
            .save_file_fallback_marker(&key, &stale_keyring)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .save_if_current(&key, &stale_keyring, &file_fallback)
            .await
            .expect("file fallback save during keyring outage");
        keyring.set_unavailable(false);

        let cleared = store
            .clear_if_current(&key, &stale_keyring)
            .await
            .expect("conditional clear with stale keyring expected");
        let loaded = store.load(&key).await.expect("load effective current");

        assert!(!cleared);
        assert_eq!(loaded, Some(file_fallback.clone()));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(file_fallback),
            "stale keyring material must not clear marked file fallback current material"
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_current_clears_marked_file_fallback_current_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &stale_keyring)
            .await
            .expect("seed matching file shadow");
        store
            .save_file_fallback_marker(&key, &stale_keyring)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .save_if_current(&key, &stale_keyring, &file_fallback)
            .await
            .expect("file fallback save during keyring outage");
        keyring.set_unavailable(false);

        let cleared = store
            .clear_if_current(&key, &file_fallback)
            .await
            .expect("conditional clear with file fallback expected");
        let loaded = store.load(&key).await.expect("load after clear");

        assert!(cleared);
        assert_eq!(loaded, None);
        assert_eq!(keyring.load_unlocked(&key).expect("keyring load"), None);
    }

    #[tokio::test]
    async fn keyring_clear_if_current_keeps_marked_fallback_when_keyring_clear_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &file_fallback)
            .await
            .expect("seed file fallback");
        store
            .save_file_fallback_marker(&key, &file_fallback)
            .await
            .expect("seed fallback marker");
        keyring.fail_clear_call(1);

        let err = store
            .clear_if_current(&key, &file_fallback)
            .await
            .expect_err("keyring clear failure must fail before fallback truth is removed");

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_some(),
            "fallback marker must remain when keyring clear does not commit"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_fallback.clone()),
            "file fallback material must remain authoritative after failed clear"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(stale_keyring),
            "stale keyring material must not become authoritative after failed clear"
        );

        let cleared = store
            .clear_if_current(&key, &file_fallback)
            .await
            .expect("retry conditional clear");

        assert!(cleared);
        assert_eq!(store.load(&key).await.expect("load after retry"), None);
        assert_eq!(keyring.load_unlocked(&key).expect("keyring load"), None);
    }

    #[tokio::test]
    async fn keyring_clear_keeps_marked_fallback_when_keyring_clear_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &file_fallback)
            .await
            .expect("seed file fallback");
        store
            .save_file_fallback_marker(&key, &file_fallback)
            .await
            .expect("seed fallback marker");
        keyring.fail_clear_call(1);

        let err = store
            .clear(&key)
            .await
            .expect_err("keyring clear failure must fail before fallback truth is removed");

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_some(),
            "fallback marker must remain when unconditional keyring clear does not commit"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_fallback.clone()),
            "file fallback material must remain authoritative after failed unconditional clear"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(stale_keyring),
            "stale keyring material must not become authoritative after failed unconditional clear"
        );

        store.clear(&key).await.expect("retry unconditional clear");

        assert_eq!(store.load(&key).await.expect("load after retry"), None);
        assert_eq!(keyring.load_unlocked(&key).expect("keyring load"), None);
    }

    #[tokio::test]
    async fn keyring_save_if_current_rejects_stale_keyring_when_clear_tombstone_is_pending() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .clear_if_current(&key, &expected)
            .await
            .expect("record clear tombstone during outage");
        keyring.set_unavailable(false);

        let saved = store
            .save_if_current(&key, &expected, &replacement)
            .await
            .expect("conditional save with stale keyring expected");
        let loaded = store.load(&key).await.expect("load effective current");

        assert!(!saved);
        assert_eq!(loaded, None);
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "pending clear tombstone must make stale keyring material non-current for CAS"
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_optional_rejects_stale_keyring_when_clear_tombstone_is_pending()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .clear_if_current(&key, &expected)
            .await
            .expect("record clear tombstone during outage");
        keyring.set_unavailable(false);

        let saved = store
            .save_if_current_optional(&key, Some(&expected), &replacement)
            .await
            .expect("optional conditional save with stale keyring expected");
        let loaded = store.load(&key).await.expect("load effective current");

        assert!(!saved);
        assert_eq!(loaded, None);
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "pending clear tombstone must make stale keyring material non-current for optional CAS"
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_optional_allows_none_after_pending_clear_tombstone() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .clear_if_current(&key, &expected)
            .await
            .expect("record clear tombstone during outage");
        keyring.set_unavailable(false);

        let saved = store
            .save_if_current_optional(&key, None, &replacement)
            .await
            .expect("optional initial save after pending clear tombstone");
        let loaded = store.load(&key).await.expect("load replacement");

        assert!(saved);
        assert_eq!(loaded, Some(replacement.clone()));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement),
            "new initial save should be allowed after the tombstone has made current material None"
        );
    }

    #[tokio::test]
    async fn keyring_save_keeps_pending_clear_tombstone_when_keyring_save_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .clear_if_current(&key, &expected)
            .await
            .expect("record clear tombstone during outage");
        keyring.set_unavailable(false);
        keyring.fail_save_call(1);

        let err = store
            .save(&key, &replacement)
            .await
            .expect_err("keyring save failure must not remove pending clear tombstone");

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_some(),
            "pending clear tombstone must remain when replacement keyring save fails"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(expected),
            "old keyring material remains physically present until tombstone reconciliation"
        );
        assert_eq!(
            store
                .load(&key)
                .await
                .expect("load after failed replacement"),
            None,
            "pending clear tombstone must still suppress old keyring material"
        );
        assert_eq!(keyring.load_unlocked(&key).expect("keyring load"), None);
    }

    #[tokio::test]
    async fn keyring_clear_keeps_pending_clear_tombstone_when_keyring_clear_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);
        store
            .clear_if_current(&key, &expected)
            .await
            .expect("record clear tombstone during outage");
        keyring.set_unavailable(false);
        keyring.fail_clear_call(1);

        let err = store
            .clear(&key)
            .await
            .expect_err("keyring clear failure must not remove pending clear tombstone");

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_some(),
            "pending clear tombstone must remain when keyring clear fails"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(expected),
            "old keyring material remains physically present until tombstone reconciliation"
        );
        assert_eq!(
            store.load(&key).await.expect("load after failed clear"),
            None,
            "pending clear tombstone must still suppress old keyring material"
        );
        assert_eq!(keyring.load_unlocked(&key).expect("keyring load"), None);
    }

    #[tokio::test]
    async fn keyring_clear_if_current_rejects_unleased_clear_during_keyring_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected_file = token("expected-file");
        let newer_keyring = token("newer-keyring");
        keyring.insert(&key, newer_keyring.clone());
        store
            .file
            .save_unlocked(&key, &expected_file)
            .await
            .expect("seed unleased file fallback");
        keyring.set_unavailable(true);

        let err = store
            .clear_if_current(&key, &expected_file)
            .await
            .expect_err("unleased conditional clear cannot write an Any tombstone");
        keyring.set_unavailable(false);

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_none(),
            "unleased conditional clear must not leave an Any tombstone"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(expected_file),
            "file material must remain because clear did not commit"
        );
        assert_eq!(
            store.load(&key).await.expect("load after failed clear"),
            Some(newer_keyring.clone()),
            "newer keyring material must not be deleted by a failed unleased file CAS clear"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(newer_keyring)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keyring_clear_if_current_tombstones_finalized_cleanup_over_pending_shadow() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let pending = token("fresh").with_auth_pending_owner_binding(key.clone(), 0);
        let finalized = token("fresh").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, pending.clone());
        store
            .file
            .save_unlocked(&key, &pending)
            .await
            .expect("seed pending file material");
        let shadow_path = file_shadow_path(temp.path(), &key);
        let shadow_dir = shadow_path.parent().expect("shadow dir").to_path_buf();
        let original_permissions = std::fs::metadata(&shadow_dir)
            .expect("shadow dir metadata")
            .permissions();
        let mut readonly_permissions = original_permissions.clone();
        readonly_permissions.set_mode(0o500);
        std::fs::set_permissions(&shadow_dir, readonly_permissions)
            .expect("make shadow dir unwritable");
        keyring.fail_save_call(2);

        let save_err = store
            .save_if_current(&key, &pending, &finalized)
            .await
            .expect_err("final keyring save with failed file shadow and rollback must fail");

        std::fs::set_permissions(&shadow_dir, original_permissions)
            .expect("restore shadow dir permissions");
        assert!(matches!(save_err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(finalized.clone()),
            "injected rollback failure leaves finalized material visible in keyring"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(pending.clone()),
            "pending file material remains readable after the failed finalization"
        );
        keyring.set_unavailable(true);

        let cleared = store
            .clear_if_current(&key, &finalized)
            .await
            .expect("finalization cleanup should record a lease tombstone during keyring outage");

        assert!(cleared);
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_some(),
            "cleanup must leave a durable tombstone for finalized keyring material"
        );
        keyring.set_unavailable(false);

        let loaded = store.load(&key).await;

        assert!(
            matches!(loaded, Err(TokenStoreError::Unavailable(_))),
            "unmarked pending material from the failed finalization must fail closed after keyring cleanup"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "lease tombstone must clear the visible finalized keyring material"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(pending)
        );
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_none(),
            "tombstone should be consumed once keyring reconciliation succeeds"
        );
    }

    #[tokio::test]
    async fn keyring_load_rejects_visible_keyring_replacement_when_shadow_and_rollback_failed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, expected.clone());
        keyring.fail_save_call(2);
        let shadow_path = file_shadow_path(temp.path(), &key);
        std::fs::create_dir_all(&shadow_path).expect("create unreplaceable shadow directory");

        let err = store
            .save_if_current(&key, &expected, &replacement)
            .await
            .expect_err("unsafe partial keyring save must fail");
        let load_after_partial_save = store.load(&key).await;

        assert!(matches!(err, TokenStoreError::Unavailable(_)));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement),
            "injected rollback failure leaves the replacement visible in keyring"
        );
        assert!(
            load_after_partial_save.is_err(),
            "AutoTokenStore load must not accept keyring material while a stale file fallback cannot be made safe"
        );
    }

    #[tokio::test]
    async fn keyring_load_honors_file_clear_recorded_during_transient_keyring_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        keyring.set_unavailable(true);

        let cleared = store
            .clear_if_current(&key, &expected)
            .await
            .expect("file fallback clear during keyring outage");
        keyring.set_unavailable(false);
        let loaded = store.load(&key).await.expect("load after keyring returns");

        assert!(cleared);
        assert_eq!(loaded, None);
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "keyring material must not reappear after a matching file fallback clear"
        );
    }

    #[tokio::test]
    async fn keyring_load_honors_unconditional_clear_recorded_during_transient_keyring_outage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        keyring.set_unavailable(true);

        store.clear(&key).await.expect("file fallback clear");
        keyring.set_unavailable(false);
        let loaded = store.load(&key).await.expect("load after keyring returns");

        assert_eq!(loaded, None);
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            None,
            "explicit file fallback clear must clear stale keyring material when keyring returns"
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_current_preserves_file_when_tombstone_write_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        std::fs::create_dir_all(store.clear_tombstone_path(&key))
            .expect("make tombstone write fail");
        keyring.set_unavailable(true);

        let err = store
            .clear_if_current(&key, &expected)
            .await
            .expect_err("tombstone write failure must fail clear");

        assert!(
            matches!(
                err,
                TokenStoreError::Io(_) | TokenStoreError::PermissionDenied(_)
            ),
            "unexpected tombstone failure: {err}"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(expected),
            "file material must remain when the durable clear tombstone cannot be written"
        );
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_some(),
            "fallback marker must remain when clear tombstone write fails"
        );
    }

    #[tokio::test]
    async fn keyring_clear_preserves_file_when_tombstone_write_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        store
            .save_file_fallback_marker(&key, &expected)
            .await
            .expect("seed fallback marker");
        std::fs::create_dir_all(store.clear_tombstone_path(&key))
            .expect("make tombstone write fail");
        keyring.set_unavailable(true);

        let err = store
            .clear(&key)
            .await
            .expect_err("tombstone write failure must fail clear");

        assert!(
            matches!(
                err,
                TokenStoreError::Io(_) | TokenStoreError::PermissionDenied(_)
            ),
            "unexpected tombstone failure: {err}"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(expected),
            "file material must remain when the durable clear tombstone cannot be written"
        );
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_some(),
            "fallback marker must remain when unconditional clear tombstone write fails"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keyring_save_preserves_tokens_when_sidecar_cleanup_fails_before_commit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file shadow");
        let sidecar_path = store.clear_tombstone_path(&key);
        let original_permissions = make_unclearable_sidecar_dir(&sidecar_path);

        let result = store.save(&key, &replacement).await;

        std::fs::set_permissions(&sidecar_path, original_permissions)
            .expect("restore sidecar dir permissions");
        let err = result.expect_err("sidecar cleanup failure must fail save before commit");
        assert!(
            matches!(
                err,
                TokenStoreError::Io(_) | TokenStoreError::PermissionDenied(_)
            ),
            "unexpected sidecar cleanup failure: {err}"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(expected.clone()),
            "keyring material must not change when stale sidecar cleanup fails"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(expected),
            "file shadow must not change when stale sidecar cleanup fails"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keyring_file_fallback_save_succeeds_when_stale_tombstone_cleanup_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 1);
        let sidecar_path = store.clear_tombstone_path(&key);
        let original_permissions = make_unclearable_sidecar_dir(&sidecar_path);
        keyring.set_unavailable(true);

        let saved = store
            .save_if_current_optional(&key, None, &replacement)
            .await
            .expect("file fallback save should not fail after material and marker commit");

        std::fs::set_permissions(&sidecar_path, original_permissions)
            .expect("restore sidecar dir permissions");
        assert!(saved);
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(replacement.clone())
        );
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_some(),
            "fallback marker must remain authoritative when old tombstone cleanup is deferred"
        );
        keyring.set_unavailable(false);
        assert_eq!(
            store.load(&key).await.expect("load marked file fallback"),
            Some(replacement)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keyring_load_keeps_fallback_marker_when_stale_tombstone_cleanup_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &file_fallback)
            .await
            .expect("seed file fallback");
        store
            .save_file_fallback_marker(&key, &file_fallback)
            .await
            .expect("seed fallback marker");
        let sidecar_path = store.clear_tombstone_path(&key);
        let original_permissions = make_unclearable_sidecar_dir(&sidecar_path);

        let result = store.load(&key).await;

        std::fs::set_permissions(&sidecar_path, original_permissions)
            .expect("restore sidecar dir permissions");
        let err = result.expect_err("stale tombstone cleanup failure must fail closed");
        assert!(
            matches!(
                err,
                TokenStoreError::Io(_) | TokenStoreError::PermissionDenied(_)
            ),
            "unexpected stale tombstone cleanup failure: {err}"
        );
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_some(),
            "fallback marker must remain until stale tombstone cleanup succeeds"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(stale_keyring),
            "stale keyring material must not be promoted when sidecar cleanup fails"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_fallback)
        );
    }

    #[tokio::test]
    async fn keyring_load_keeps_fallback_marker_when_promotion_keyring_save_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &file_fallback)
            .await
            .expect("seed file fallback");
        store
            .save_file_fallback_marker(&key, &file_fallback)
            .await
            .expect("seed fallback marker");
        keyring.fail_save_call(1);

        let err = store
            .load(&key)
            .await
            .expect_err("promotion keyring save failure must fail closed");

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_some(),
            "fallback marker must remain when promotion does not commit to keyring"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(stale_keyring),
            "stale keyring material must remain after failed promotion"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_fallback.clone()),
            "file fallback material must remain authoritative for retry"
        );

        let loaded = store.load(&key).await.expect("retry promotion");

        assert_eq!(loaded, Some(file_fallback.clone()));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(file_fallback)
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_keeps_previous_fallback_marker_when_keyring_save_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let stale_keyring = token("stale-keyring").with_auth_lease_binding(key.clone(), 1);
        let file_fallback = token("file-fallback").with_auth_lease_binding(key.clone(), 2);
        let replacement = token("replacement").with_auth_lease_binding(key.clone(), 3);
        keyring.insert(&key, stale_keyring.clone());
        store
            .file
            .save_unlocked(&key, &file_fallback)
            .await
            .expect("seed file fallback");
        store
            .save_file_fallback_marker(&key, &file_fallback)
            .await
            .expect("seed fallback marker");
        keyring.fail_save_call(1);

        let err = store
            .save_if_current(&key, &file_fallback, &replacement)
            .await
            .expect_err("replacement keyring save failure must fail before marker removal");

        assert!(matches!(err, TokenStoreError::KeyringUnavailable(_)));
        assert!(
            store
                .load_file_fallback_marker(&key)
                .await
                .expect("fallback marker load")
                .is_some(),
            "previous fallback marker must remain when replacement does not commit"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(file_fallback.clone()),
            "previous file fallback material must remain authoritative after failed replacement"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(stale_keyring),
            "stale keyring material must not replace the marked fallback after failed replacement"
        );

        let saved = store
            .save_if_current(&key, &file_fallback, &replacement)
            .await
            .expect("retry replacement");
        let loaded = store.load(&key).await.expect("load replacement");

        assert!(saved);
        assert_eq!(loaded, Some(replacement.clone()));
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn keyring_clear_succeeds_after_tombstone_when_immediate_file_cleanup_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        keyring.set_unavailable(true);
        let shadow_path = file_shadow_path(temp.path(), &key);
        std::fs::create_dir_all(&shadow_path).expect("make file cleanup fail");

        store
            .clear(&key)
            .await
            .expect("durable tombstone makes outage clear successful");

        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_some(),
            "clear tombstone must be durable when immediate file cleanup cannot finish"
        );
        assert!(
            shadow_path.is_dir(),
            "failed immediate cleanup may leave unreadable file path for later fail-closed reconciliation"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keyring_clear_if_current_preserves_file_when_marker_cleanup_fails_before_commit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        let sidecar_path = store.file_fallback_marker_path(&key);
        let original_permissions = make_unclearable_sidecar_dir(&sidecar_path);
        keyring.set_unavailable(true);

        let result = store.clear_if_current(&key, &expected).await;

        std::fs::set_permissions(&sidecar_path, original_permissions)
            .expect("restore sidecar dir permissions");
        let err = result.expect_err("sidecar cleanup failure must fail clear before commit");
        assert!(
            matches!(
                err,
                TokenStoreError::Io(_) | TokenStoreError::PermissionDenied(_)
            ),
            "unexpected sidecar cleanup failure: {err}"
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(expected),
            "file material must remain when stale marker cleanup fails"
        );
        assert!(
            store
                .load_clear_tombstone(&key)
                .await
                .expect("clear tombstone load")
                .is_none(),
            "clear tombstone must not be recorded until marker cleanup succeeds"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keyring_load_fails_closed_after_tombstone_cleanup_cannot_clear_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected").with_auth_lease_binding(key.clone(), 1);
        keyring.insert(&key, expected.clone());
        store
            .file
            .save_unlocked(&key, &expected)
            .await
            .expect("seed matching file fallback");
        let tombstone = AutoClearTombstone::from_tokens(&expected).expect("lease tombstone");
        store
            .save_clear_tombstone(&key, &tombstone)
            .await
            .expect("save clear tombstone");
        let shadow_path = file_shadow_path(temp.path(), &key);
        let shadow_dir = shadow_path.parent().expect("shadow dir").to_path_buf();
        let original_permissions = std::fs::metadata(&shadow_dir)
            .expect("shadow dir metadata")
            .permissions();
        let mut readonly_permissions = original_permissions.clone();
        readonly_permissions.set_mode(0o500);
        std::fs::set_permissions(&shadow_dir, readonly_permissions)
            .expect("make shadow dir read-only");

        let first_load = store.load(&key).await;
        let second_load = store.load(&key).await;

        std::fs::set_permissions(&shadow_dir, original_permissions)
            .expect("restore shadow dir permissions");
        assert!(
            first_load.is_err(),
            "load must fail when tombstone cleanup cannot clear file material"
        );
        assert!(
            second_load.is_err(),
            "tombstone must keep later loads fail-closed instead of returning stale file material"
        );
    }

    #[tokio::test]
    async fn keyring_save_if_current_rolls_back_when_file_shadow_cannot_be_made_safe() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected");
        let replacement = token("replacement");
        keyring
            .save_unlocked(&key, &expected)
            .expect("seed keyring");
        let shadow_path = file_shadow_path(temp.path(), &key);
        std::fs::create_dir_all(&shadow_path).expect("create unreplaceable shadow directory");

        let err = store
            .save_if_current(&key, &expected, &replacement)
            .await
            .expect_err("unsafe partial keyring save must fail");

        assert!(
            matches!(
                err,
                TokenStoreError::Io(_) | TokenStoreError::PermissionDenied(_)
            ),
            "unexpected shadow failure: {err}"
        );
        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(expected),
            "keyring replacement must be rolled back when stale file fallback cannot be made safe"
        );
        assert!(
            shadow_path.is_dir(),
            "failed shadow path remains unavailable for file fallback"
        );
    }

    #[tokio::test]
    async fn keyring_save_replaces_divergent_file_shadow() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let other = token("other-file-shadow");
        let replacement = token("replacement");
        store
            .file
            .save_unlocked(&key, &other)
            .await
            .expect("seed divergent file shadow");

        store.save(&key, &replacement).await.expect("raw save");

        assert_eq!(
            keyring.load_unlocked(&key).expect("keyring load"),
            Some(replacement.clone())
        );
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn keyring_clear_if_current_clears_divergent_file_shadow() {
        let temp = tempfile::tempdir().expect("tempdir");
        let keyring = Arc::new(MemoryKeyringBackend::default());
        let store =
            AutoTokenStore::new_with_keyring_backend(temp.path().to_path_buf(), keyring.clone());
        let key = key();
        let expected = token("expected");
        let other = token("other-file-shadow");
        keyring
            .save_unlocked(&key, &expected)
            .expect("seed keyring");
        store
            .file
            .save_unlocked(&key, &other)
            .await
            .expect("seed divergent file shadow");

        let cleared = store
            .clear_if_current(&key, &expected)
            .await
            .expect("conditional clear");

        assert!(cleared);
        assert_eq!(keyring.load_unlocked(&key).expect("keyring load"), None);
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            None
        );
    }
}
