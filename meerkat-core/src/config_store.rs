//! Config store abstraction.

use crate::config::{Config, ConfigDelta, ConfigError};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Abstraction over config persistence backends.
pub trait ConfigStore: Send + Sync {
    /// Fetch the current config.
    fn get(&self) -> Result<Config, ConfigError>;

    /// Persist the provided config.
    fn set(&self, config: Config) -> Result<(), ConfigError>;

    /// Apply a config patch and return the updated config.
    fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError>;
}

/// In-memory config store for ephemeral settings.
pub struct MemoryConfigStore {
    config: Mutex<Config>,
}

impl MemoryConfigStore {
    pub fn new(config: Config) -> Self {
        Self {
            config: Mutex::new(config),
        }
    }
}

impl ConfigStore for MemoryConfigStore {
    fn get(&self) -> Result<Config, ConfigError> {
        Ok(self.config.lock().unwrap().clone())
    }

    fn set(&self, config: Config) -> Result<(), ConfigError> {
        *self.config.lock().unwrap() = config;
        Ok(())
    }

    fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let mut config = self.config.lock().unwrap();
        let mut value =
            serde_json::to_value(&*config).map_err(|e| ConfigError::ParseError(e.to_string()))?;
        merge_patch(&mut value, delta.0);
        let updated: Config =
            serde_json::from_value(value).map_err(|e| ConfigError::ParseError(e.to_string()))?;
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
    pub fn global() -> Result<Self, ConfigError> {
        let path = Config::global_config_path()
            .ok_or_else(|| ConfigError::MissingField("HOME".to_string()))?;
        let store = Self {
            path,
            create_if_missing: true,
        };
        store.ensure_exists()?;
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

    fn ensure_exists(&self) -> Result<(), ConfigError> {
        if self.path.exists() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::IoError(e.to_string()))?;
        }
        let content = Config::template_toml();
        std::fs::write(&self.path, content).map_err(|e| ConfigError::IoError(e.to_string()))?;
        Ok(())
    }
}

impl ConfigStore for FileConfigStore {
    fn get(&self) -> Result<Config, ConfigError> {
        if self.create_if_missing {
            self.ensure_exists()?;
        }

        if !self.path.exists() {
            return Ok(Config::default());
        }

        let content =
            std::fs::read_to_string(&self.path).map_err(|e| ConfigError::IoError(e.to_string()))?;
        toml::from_str(&content).map_err(|e| ConfigError::ParseError(e.to_string()))
    }

    fn set(&self, config: Config) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::IoError(e.to_string()))?;
        }
        let content =
            toml::to_string_pretty(&config).map_err(|e| ConfigError::ParseError(e.to_string()))?;
        std::fs::write(&self.path, content).map_err(|e| ConfigError::IoError(e.to_string()))?;
        Ok(())
    }

    fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let mut value = serde_json::to_value(self.get()?)
            .map_err(|e| ConfigError::ParseError(e.to_string()))?;
        merge_patch(&mut value, delta.0);
        let updated: Config =
            serde_json::from_value(value).map_err(|e| ConfigError::ParseError(e.to_string()))?;
        self.set(updated.clone())?;
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
