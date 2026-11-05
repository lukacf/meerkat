//! The config the factory reads: the realm's own `config.toml`, through its
//! owner ([`FileConfigStore`]), with what `rkat` applies on top of it at
//! load time (environment overrides) and the TUI's user-global `SYSTEM.md`
//! ([`super::instructions::apply_system_md`]).
//!
//! Every read goes to the document, so what the TUI writes into it (the
//! endpoint connections, `[models.<id>]` profiles; see
//! [`crate::endpoints`]) and what another surface writes is what the next
//! session build sees. Writes go to the document unchanged: the overrides
//! are never persisted.

use std::path::Path;

use async_trait::async_trait;
use meerkat_core::config::{ConfigError, ConfigWarning};
use meerkat_core::{Config, ConfigDelta, ConfigStore, FileConfigStore};

use super::dirs::GlobalDirs;

/// The realm document plus the load-time overrides (see the module docs).
pub struct RealmConfigStore {
    document: FileConfigStore,
    global: Option<GlobalDirs>,
}

impl RealmConfigStore {
    /// The store of the realm document at `path`; `global` supplies the
    /// user-global `SYSTEM.md`.
    #[must_use]
    pub fn new(path: &Path, global: Option<GlobalDirs>) -> Self {
        Self {
            document: FileConfigStore::new(path.to_path_buf(), meerkat_models::canonical()),
            global,
        }
    }

    /// The document's owner, for writes of individual entries.
    #[must_use]
    pub fn document(&self) -> &FileConfigStore {
        &self.document
    }

    /// The document as persisted with the load-time overrides applied.
    fn effective(&self, mut config: Config) -> Result<Config, ConfigError> {
        config.apply_env_overrides()?;
        super::instructions::apply_system_md(&mut config, self.global.as_ref());
        Ok(config)
    }
}

impl std::fmt::Debug for RealmConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealmConfigStore")
            .field("path", &self.document.path())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ConfigStore for RealmConfigStore {
    async fn get(&self) -> Result<Config, ConfigError> {
        self.effective(self.document.get().await?)
    }

    async fn get_with_warnings(&self) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        let (config, warnings) = self.document.get_with_warnings().await?;
        Ok((self.effective(config)?, warnings))
    }

    async fn set(&self, config: Config) -> Result<(), ConfigError> {
        self.document.set(config).await
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let patched = self.document.patch(delta).await?;
        self.effective(patched)
    }

    async fn patch_preview(
        &self,
        delta: &ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        let (config, warnings) = self.document.patch_preview(delta).await?;
        Ok((self.effective(config)?, warnings))
    }

    async fn patch_with_warnings(
        &self,
        delta: ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        let (config, warnings) = self.document.patch_with_warnings(delta).await?;
        Ok((self.effective(config)?, warnings))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_follow_the_document_and_writes_reach_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let store = RealmConfigStore::new(&path, None);
        assert!(store.get().await.unwrap().models.custom.is_empty());
        // Another writer (rkat) changes the document: the next read sees it.
        std::fs::write(
            &path,
            "[models.\"m-1\"]\nprovider = \"openai\"\ncontext_window = 1000\n",
        )
        .unwrap();
        let config = store.get().await.unwrap();
        assert_eq!(config.models.custom["m-1"].context_window, Some(1000));
        // A write through the store lands in the document.
        store
            .patch(ConfigDelta(serde_json::json!({
                "models": { "m-2": { "provider": "anthropic" } }
            })))
            .await
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("m-2"), "{text}");
    }
}
