//! Config store abstraction.

use crate::config::{Config, ConfigDelta, ConfigError};
use crate::model_profile::ModelCatalog;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

/// Resolved paths attached to a config store context.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigResolvedPaths {
    pub root: String,
    pub manifest_path: String,
    pub config_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions_sqlite_path: Option<String>,
    pub sessions_jsonl_dir: String,
}

/// Optional metadata for config endpoints.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigStoreMetadata {
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub resolved_paths: Option<ConfigResolvedPaths>,
}

/// Abstraction over config persistence backends.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ConfigStore: Send + Sync {
    /// Fetch the current config.
    async fn get(&self) -> Result<Config, ConfigError>;

    /// Persist the provided config.
    async fn set(&self, config: Config) -> Result<(), ConfigError>;

    /// Apply a config patch and return the updated config.
    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError>;

    /// Optional metadata to expose on config APIs.
    fn metadata(&self) -> Option<ConfigStoreMetadata> {
        None
    }
}

/// In-memory config store for ephemeral settings.
pub struct MemoryConfigStore {
    config: tokio::sync::RwLock<Config>,
    catalog: ModelCatalog,
}

impl MemoryConfigStore {
    /// Create a store validating against the injected model catalog
    /// (canonically `meerkat_models::canonical()`).
    pub fn new(config: Config, catalog: ModelCatalog) -> Self {
        Self {
            config: tokio::sync::RwLock::new(config),
            catalog,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ConfigStore for MemoryConfigStore {
    async fn get(&self) -> Result<Config, ConfigError> {
        Ok(self.config.read().await.clone())
    }

    async fn set(&self, config: Config) -> Result<(), ConfigError> {
        config.validate(self.catalog)?;
        *self.config.write().await = config;
        Ok(())
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let mut config = self.config.write().await;
        let mut value = serde_json::to_value(&*config).map_err(ConfigError::Json)?;
        merge_patch(&mut value, delta.0);
        let updated: Config = serde_json::from_value(value).map_err(ConfigError::Json)?;
        updated.validate(self.catalog)?;
        *config = updated.clone();
        Ok(updated)
    }
}

/// Metadata-tagged config store wrapper.
pub struct TaggedConfigStore {
    inner: Arc<dyn ConfigStore>,
    metadata: ConfigStoreMetadata,
}

impl TaggedConfigStore {
    pub fn new(inner: Arc<dyn ConfigStore>, metadata: ConfigStoreMetadata) -> Self {
        Self { inner, metadata }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ConfigStore for TaggedConfigStore {
    async fn get(&self) -> Result<Config, ConfigError> {
        self.inner.get().await
    }

    async fn set(&self, config: Config) -> Result<(), ConfigError> {
        self.inner.set(config).await
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        self.inner.patch(delta).await
    }

    fn metadata(&self) -> Option<ConfigStoreMetadata> {
        Some(self.metadata.clone())
    }
}

/// File-backed config store with optional bootstrap template.
pub struct FileConfigStore {
    path: PathBuf,
    create_if_missing: bool,
    catalog: ModelCatalog,
}

impl FileConfigStore {
    /// Create a new file-backed store for an explicit path, validating
    /// against the injected model catalog (canonically
    /// `meerkat_models::canonical()`).
    pub fn new(path: PathBuf, catalog: ModelCatalog) -> Self {
        Self {
            path,
            create_if_missing: false,
            catalog,
        }
    }

    /// Create a store that bootstraps a global config file if missing.
    pub async fn global(catalog: ModelCatalog) -> Result<Self, ConfigError> {
        let path = Config::global_config_path()
            .ok_or_else(|| ConfigError::MissingField("HOME".to_string()))?;
        let store = Self {
            path,
            create_if_missing: true,
            catalog,
        };
        store.ensure_exists().await?;
        Ok(store)
    }

    /// Create a store rooted at the provided project directory.
    pub fn project(project_root: impl Into<PathBuf>, catalog: ModelCatalog) -> Self {
        let root = project_root.into();
        Self::new(root.join(".rkat").join("config.toml"), catalog)
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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
        config.validate(self.catalog)?;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let content = toml::to_string_pretty(&config).map_err(ConfigError::TomlSerialize)?;
        let parent = self
            .path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let tmp_path = parent.join(format!(".config.tmp.{}", crate::time_compat::new_uuid_v7()));
        let mut tmp = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .await?;
        tmp.write_all(content.as_bytes()).await?;
        tmp.sync_all().await?;
        drop(tmp);
        tokio::fs::rename(&tmp_path, &self.path).await?;
        Ok(())
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let mut value = serde_json::to_value(self.get().await?).map_err(ConfigError::Json)?;
        merge_patch(&mut value, delta.0);
        let updated: Config = serde_json::from_value(value).map_err(ConfigError::Json)?;
        updated.validate(self.catalog)?;
        self.set(updated.clone()).await?;
        Ok(updated)
    }
}

/// Canonical RFC 7386 JSON merge-patch application.
///
/// This is the single owner of config patch acceptance/rejection semantics:
/// a `null` patch value removes the key, an object recurses, and any other
/// value replaces. All surfaces (RPC, REST, MCP) MUST route through this and
/// [`apply_config_patch_preview`] rather than re-deriving the merge rules.
pub fn merge_patch(base: &mut Value, patch: Value) {
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

/// Compute the [`Config`] that would result from applying `patch` to `config`,
/// without persisting it.
///
/// This is the canonical preview used by every surface's "config patch" entry
/// point. The patch is applied via [`merge_patch`] and re-deserialized into a
/// typed [`Config`]; a malformed patch (one that no longer deserializes) yields
/// a typed [`ConfigError::Json`] that surfaces map onto their own error type —
/// none re-implement the merge or the (de)serialization.
pub fn apply_config_patch_preview(config: &Config, patch: Value) -> Result<Config, ConfigError> {
    let mut value = serde_json::to_value(config).map_err(ConfigError::Json)?;
    merge_patch(&mut value, patch);
    serde_json::from_value(value).map_err(ConfigError::Json)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn merge_patch_removes_keys_on_null_and_merges_nested_objects() {
        let mut base = serde_json::json!({
            "keep": 1,
            "drop": "gone",
            "nested": { "a": 1, "b": 2 },
        });
        let patch = serde_json::json!({
            "drop": null,
            "nested": { "b": 20, "c": 3 },
            "added": true,
        });
        merge_patch(&mut base, patch);
        assert_eq!(
            base,
            serde_json::json!({
                "keep": 1,
                "nested": { "a": 1, "b": 20, "c": 3 },
                "added": true,
            }),
            "null removes a key, nested objects merge recursively, scalars replace"
        );
    }

    #[test]
    fn apply_config_patch_preview_applies_patch_without_mutating_input() {
        let config = Config::default();
        let original_max_tokens = config.max_tokens;
        let bumped = original_max_tokens.saturating_add(1);
        let previewed =
            apply_config_patch_preview(&config, serde_json::json!({ "max_tokens": bumped }))
                .expect("scalar patch should preview cleanly");
        assert_eq!(previewed.max_tokens, bumped, "preview reflects the patch");
        assert_eq!(
            config.max_tokens, original_max_tokens,
            "input config is not mutated by preview"
        );
    }

    #[tokio::test]
    async fn file_config_store_set_skips_null_backend_options()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join(".rkat").join("config.toml");
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let mut config = Config::default();
        let mut section = crate::RealmConfigSection::default();
        section.backend.insert(
            "openai_chatgpt".to_string(),
            crate::BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: crate::provider_matrix::OpenAiBackendKind::ChatGptBackend
                    .as_str()
                    .to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_oauth".to_string(),
            crate::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: crate::provider_matrix::OpenAiAuthMethod::ManagedChatGptOauth
                    .as_str()
                    .to_string(),
                source: crate::CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "openai_oauth".to_string(),
            crate::ProviderBindingConfig {
                backend_profile: "openai_chatgpt".to_string(),
                auth_profile: "openai_oauth".to_string(),
                default_model: Some("test-openai-default".to_string()),
                policy: Default::default(),
                provider_default: false,
            },
        );
        config.realm.insert("dev".to_string(), section);

        store.set(config).await?;
        let rendered = tokio::fs::read_to_string(&path).await?;
        assert!(
            !rendered.contains("options"),
            "null backend options should be omitted from TOML, not rendered"
        );
        let loaded = store.get().await?;
        assert!(
            loaded
                .realm
                .get("dev")
                .and_then(|section| section.backend.get("openai_chatgpt"))
                .is_some(),
            "backend profile should survive round trip"
        );
        Ok(())
    }
}
