//! Config store abstraction.

use crate::config::{Config, ConfigDelta, ConfigError};
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Abstraction over config persistence backends.
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Fetch the current config.
    async fn get(&self) -> Result<Config, ConfigError>;

    /// Persist the provided config.
    async fn set(&self, config: Config) -> Result<(), ConfigError>;

    /// Apply a config patch and return the updated config.
    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError>;
}

/// In-memory config store for ephemeral settings.
pub struct MemoryConfigStore {
    config: tokio::sync::RwLock<Config>,
}

impl MemoryConfigStore {
    pub fn new(config: Config) -> Self {
        Self {
            config: tokio::sync::RwLock::new(config),
        }
    }
}

#[async_trait]
impl ConfigStore for MemoryConfigStore {
    async fn get(&self) -> Result<Config, ConfigError> {
        Ok(self.config.read().await.clone())
    }

    async fn set(&self, config: Config) -> Result<(), ConfigError> {
        *self.config.write().await = config;
        Ok(())
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let mut config = self.config.write().await;
        let mut value = serde_json::to_value(&*config).map_err(ConfigError::Json)?;
        merge_patch(&mut value, delta.0);
        let updated: Config = serde_json::from_value(value).map_err(ConfigError::Json)?;
        *config = updated.clone();
        Ok(updated)
    }
}

/// File-backed config store with optional bootstrap template.
pub struct FileConfigStore {
    path: PathBuf,
    create_if_missing: bool,
}

impl FileConfigStore {
    /// Create a new file-backed store for an explicit path.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            create_if_missing: false,
        }
    }

    /// Create a store that bootstraps a global config file if missing.
    pub async fn global() -> Result<Self, ConfigError> {
        let path = Config::global_config_path()
            .ok_or_else(|| ConfigError::MissingField("HOME".to_string()))?;
        let store = Self {
            path,
            create_if_missing: true,
        };
        store.ensure_exists().await?;
        Ok(store)
    }

    /// Create a store rooted at the provided project directory.
    pub fn project(project_root: impl Into<PathBuf>) -> Self {
        let root = project_root.into();
        Self::new(root.join(".rkat").join("config.toml"))
    }

    /// Return the config file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn ensure_exists(&self) -> Result<(), ConfigError> {
        if tokio::fs::try_exists(&self.path).await? {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let content = Config::template_toml();
        tokio::fs::write(&self.path, content).await?;
        Ok(())
    }
}

#[async_trait]
impl ConfigStore for FileConfigStore {
    async fn get(&self) -> Result<Config, ConfigError> {
        if self.create_if_missing {
            self.ensure_exists().await?;
        }

        if !tokio::fs::try_exists(&self.path).await? {
            return Ok(Config::default());
        }

        let bytes = tokio::fs::read(&self.path).await?;
        let content = String::from_utf8(bytes).map_err(ConfigError::Utf8)?;
        toml::from_str(&content).map_err(ConfigError::Parse)
    }

    async fn set(&self, config: Config) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let content = toml::to_string_pretty(&config).map_err(ConfigError::TomlSerialize)?;
        tokio::fs::write(&self.path, content).await?;
        Ok(())
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let mut value = serde_json::to_value(self.get().await?).map_err(ConfigError::Json)?;
        merge_patch(&mut value, delta.0);
        let updated: Config = serde_json::from_value(value).map_err(ConfigError::Json)?;
        self.set(updated.clone()).await?;
        Ok(updated)
    }
}

fn merge_patch(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (k, v) in patch_map {
                if v.is_null() {
                    base_map.remove(&k);
                } else {
                    merge_patch(base_map.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        (base_val, patch_val) => {
            *base_val = patch_val;
        }
    }
}
