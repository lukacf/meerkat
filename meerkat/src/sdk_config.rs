//! SDK-facing configuration store helpers.

use std::path::PathBuf;
use std::sync::Arc;

use meerkat_core::config::ConfigError;
use meerkat_core::{Config, ConfigDelta, ConfigStore, FileConfigStore, MemoryConfigStore};

/// SDK config store wrapper.
///
/// Defaults to an in-memory store. Use `with_path` for explicit persistence.
#[derive(Clone)]
pub struct SdkConfigStore {
    store: Arc<dyn ConfigStore>,
}

impl SdkConfigStore {
    /// Create a default in-memory config store.
    pub fn new() -> Self {
        let mut config = Config::default();
        let _ = config.apply_env_overrides();
        Self {
            store: Arc::new(MemoryConfigStore::new(config)),
        }
    }

    /// Create an in-memory store with a specific config.
    pub fn with_config(config: Config) -> Self {
        let mut config = config;
        let _ = config.apply_env_overrides();
        Self {
            store: Arc::new(MemoryConfigStore::new(config)),
        }
    }

    /// Create a file-backed store for explicit persistence.
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            store: Arc::new(FileConfigStore::new(path.into())),
        }
    }

    /// Fetch the current config.
    pub fn get(&self) -> Result<Config, ConfigError> {
        let mut config = self.store.get()?;
        let _ = config.apply_env_overrides();
        Ok(config)
    }

    /// Persist the provided config.
    pub fn set(&self, config: Config) -> Result<(), ConfigError> {
        self.store.set(config)
    }

    /// Apply a config patch and return the updated config.
    pub fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        self.store.patch(delta)
    }

    /// Return the inner store (for advanced integration).
    pub fn inner(&self) -> Arc<dyn ConfigStore> {
        Arc::clone(&self.store)
    }
}

impl Default for SdkConfigStore {
    fn default() -> Self {
        Self::new()
    }
}
