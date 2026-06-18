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

/// Source of per-realm config documents for inheritance composition.
///
/// Abstraction-level seam (filesystem-free): a surface injects an implementation
/// that knows how to fetch a realm's OWN config document (the CLI reads
/// `<state_root>/<realm>/config.toml` for workspace realms and a single
/// home-rooted doc for the `global` realm; the WASM runtime returns its single
/// synthesized doc). Returning `None` means the document is ABSENT — it must NOT
/// be coerced to `Config::default()`, or an absent ancestor would clobber
/// inherited fields via the merge fold.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RealmConfigSource: Send + Sync {
    /// Fetch the OWN config document for `realm`, or `None` if absent.
    async fn config_for_realm(
        &self,
        realm: &crate::connection::RealmId,
    ) -> Result<Option<Config>, ConfigError>;

    /// Fetch the OWN raw TOML document for `realm` (presence-preserving), or
    /// `None`.
    ///
    /// Default: `None`. Composition then falls back to a value-merge that uses a
    /// `!= default` heuristic and so cannot honor a child realm overriding a
    /// scalar (e.g. a `tools.*_enabled` toggle) back to its struct default. The
    /// filesystem source overrides this with the parsed file so scalar/toggle
    /// inheritance is presence-exact (`child-wins-scalar`).
    async fn raw_config_for_realm(
        &self,
        _realm: &crate::connection::RealmId,
    ) -> Result<Option<toml::Value>, ConfigError> {
        Ok(None)
    }
}

/// Read-only reader that composes a realm's parent chain into the effective
/// flat [`Config`] the agent and resolvers consume.
///
/// This is deliberately NOT a [`ConfigStore`]: composition happens only on read.
/// Writes must go to the raw head [`ConfigStore`] (the unfolded head document),
/// never round-trip the composed view — otherwise a read-modify-write would
/// durably flatten every inherited entry into the child doc (and a child could
/// then never shed an inherited mcp/hook entry). Keeping the composing reader
/// and the head writer as distinct types makes that mistake unrepresentable.
pub struct EffectiveConfigReader {
    source: Arc<dyn RealmConfigSource>,
}

impl EffectiveConfigReader {
    pub fn new(source: Arc<dyn RealmConfigSource>) -> Self {
        Self { source }
    }

    /// Compose the effective config for `head` by walking + folding its chain.
    ///
    /// Discovery fetches the head document, follows its `parent` edge, and
    /// always attempts the reserved `global` tail. The fetch loop is bounded
    /// (a `seen` set + a depth guard) purely so a malformed cyclic config
    /// terminates the fetch; [`crate::config::compose_effective_config`] then
    /// re-resolves via the chain authority, which reports the cycle as a typed
    /// error rather than silently truncating.
    pub async fn effective_config(
        &self,
        head: &crate::connection::RealmId,
    ) -> Result<Config, ConfigError> {
        use crate::connection::{MAX_REALM_CHAIN_DEPTH, RealmId};
        use std::collections::{BTreeMap, BTreeSet};

        let mut docs: BTreeMap<RealmId, Config> = BTreeMap::new();
        let mut raw_docs: BTreeMap<RealmId, toml::Value> = BTreeMap::new();
        let mut seen: BTreeSet<RealmId> = BTreeSet::new();
        let mut frontier = vec![head.clone()];
        let mut guard = 0usize;

        while let Some(realm) = frontier.pop() {
            guard += 1;
            if guard > MAX_REALM_CHAIN_DEPTH + 4 {
                break; // belt-and-suspenders; the authority re-validates depth
            }
            if !seen.insert(realm.clone()) {
                continue;
            }
            if let Some(doc) = self.source.config_for_realm(&realm).await? {
                if let Some(parent) = doc
                    .realm
                    .get(realm.as_str())
                    .and_then(|section| section.parent.clone())
                {
                    frontier.push(parent);
                }
                if let Some(raw) = self.source.raw_config_for_realm(&realm).await? {
                    raw_docs.insert(realm.clone(), raw);
                }
                docs.insert(realm, doc);
            }
        }

        // Always attempt the implicit `global` tail document.
        let global = RealmId::global();
        if seen.insert(global.clone())
            && let Some(doc) = self.source.config_for_realm(&global).await?
        {
            if let Some(raw) = self.source.raw_config_for_realm(&global).await? {
                raw_docs.insert(global.clone(), raw);
            }
            docs.insert(global, doc);
        }

        Ok(crate::config::compose_effective_config(
            &docs, &raw_docs, head,
        )?)
    }

    /// Like [`Self::effective_config`], but the HEAD realm's document is supplied
    /// by the caller (the surface's authoritative head config — e.g. from an
    /// in-memory store or a `ConfigRuntime` snapshot) instead of being fetched
    /// from the source. Ancestors (the parent chain + the implicit `global`
    /// tail) are still fetched from the source. Network surfaces use this so the
    /// head config keeps coming from their existing config store/runtime while
    /// inheritance only ADDS ancestor docs — composing purely from a filesystem
    /// source would drop a head config that lives in memory.
    pub async fn effective_config_over_head(
        &self,
        head: &crate::connection::RealmId,
        head_config: Config,
    ) -> Result<Config, ConfigError> {
        use crate::connection::{MAX_REALM_CHAIN_DEPTH, RealmId};
        use std::collections::{BTreeMap, BTreeSet};

        let mut docs: BTreeMap<RealmId, Config> = BTreeMap::new();
        // The head's VALUES come from the caller's in-memory config (authoritative
        // — it may post-date the on-disk doc, e.g. a ConfigRuntime snapshot). Its
        // PRESENCE (which keys it explicitly sets) is read from the head's durable
        // doc in the source, so a network surface's head realm can also override a
        // scalar/toggle back to its struct default (config get/set write that same
        // doc, so its key set is current). Ancestors carry presence the same way.
        let mut raw_docs: BTreeMap<RealmId, toml::Value> = BTreeMap::new();
        let mut seen: BTreeSet<RealmId> = BTreeSet::new();
        seen.insert(head.clone());
        let mut frontier = Vec::new();
        if let Some(parent) = head_config
            .realm
            .get(head.as_str())
            .and_then(|section| section.parent.clone())
        {
            frontier.push(parent);
        }
        if let Some(raw) = self.source.raw_config_for_realm(head).await? {
            raw_docs.insert(head.clone(), raw);
        }
        docs.insert(head.clone(), head_config);

        let mut guard = 0usize;
        while let Some(realm) = frontier.pop() {
            guard += 1;
            if guard > MAX_REALM_CHAIN_DEPTH + 4 {
                break;
            }
            if !seen.insert(realm.clone()) {
                continue;
            }
            if let Some(doc) = self.source.config_for_realm(&realm).await? {
                if let Some(parent) = doc
                    .realm
                    .get(realm.as_str())
                    .and_then(|section| section.parent.clone())
                {
                    frontier.push(parent);
                }
                if let Some(raw) = self.source.raw_config_for_realm(&realm).await? {
                    raw_docs.insert(realm.clone(), raw);
                }
                docs.insert(realm, doc);
            }
        }

        let global = RealmId::global();
        if seen.insert(global.clone())
            && let Some(doc) = self.source.config_for_realm(&global).await?
        {
            if let Some(raw) = self.source.raw_config_for_realm(&global).await? {
                raw_docs.insert(global.clone(), raw);
            }
            docs.insert(global, doc);
        }

        Ok(crate::config::compose_effective_config(
            &docs, &raw_docs, head,
        )?)
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

    // RCT-16/30: EffectiveConfigReader composes a realm's chain across separate
    // per-realm docs (read-only — it is not a ConfigStore, so a write cannot
    // round-trip the composed view back into the head doc).
    struct MapSource {
        docs: std::collections::BTreeMap<String, Config>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl RealmConfigSource for MapSource {
        async fn config_for_realm(
            &self,
            realm: &crate::connection::RealmId,
        ) -> Result<Option<Config>, ConfigError> {
            Ok(self.docs.get(realm.as_str()).cloned())
        }
    }

    #[tokio::test]
    async fn effective_reader_composes_chain_across_docs() {
        use crate::connection::{RealmConfigSection, RealmId};

        let mut global = Config::default();
        global.models.openai = "g-openai".to_string();
        global
            .realm
            .insert("global".to_string(), RealmConfigSection::default());

        let mut child = Config::default();
        child.models.anthropic = "c-anthropic".to_string();
        child.realm.insert(
            "child".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::global()),
                ..Default::default()
            },
        );

        let mut docs = std::collections::BTreeMap::new();
        docs.insert("child".to_string(), child);
        docs.insert("global".to_string(), global);

        let reader = EffectiveConfigReader::new(Arc::new(MapSource { docs }));
        let eff = reader
            .effective_config(&RealmId::parse("child").unwrap())
            .await
            .expect("compose");
        assert_eq!(eff.models.openai, "g-openai", "inherited from global");
        assert_eq!(eff.models.anthropic, "c-anthropic", "child override");
        assert!(eff.realm.contains_key("global") && eff.realm.contains_key("child"));
    }

    #[tokio::test]
    async fn effective_reader_absent_ancestor_does_not_clobber() {
        use crate::connection::{RealmConfigSection, RealmId};

        // child references global via parent, but NO global doc exists.
        let mut child = Config::default();
        child.models.anthropic = "c-anthropic".to_string();
        child.realm.insert(
            "child".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::global()),
                ..Default::default()
            },
        );
        let mut docs = std::collections::BTreeMap::new();
        docs.insert("child".to_string(), child);

        let reader = EffectiveConfigReader::new(Arc::new(MapSource { docs }));
        let eff = reader
            .effective_config(&RealmId::parse("child").unwrap())
            .await
            .expect("compose");
        assert_eq!(
            eff.models.anthropic, "c-anthropic",
            "absent global ancestor must not clobber the child's own fields"
        );
    }

    // RealmConfigSource double that also serves raw TOML, so the presence-aware
    // composition path (the one every network/runtime surface exercises via
    // `effective_config_over_head`) can be tested — the plain `MapSource` uses the
    // default `raw_config_for_realm` (None) and so only covers the value-merge.
    struct RawMapSource {
        docs: std::collections::BTreeMap<String, Config>,
        raw: std::collections::BTreeMap<String, toml::Value>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl RealmConfigSource for RawMapSource {
        async fn config_for_realm(
            &self,
            realm: &crate::connection::RealmId,
        ) -> Result<Option<Config>, ConfigError> {
            Ok(self.docs.get(realm.as_str()).cloned())
        }
        async fn raw_config_for_realm(
            &self,
            realm: &crate::connection::RealmId,
        ) -> Result<Option<toml::Value>, ConfigError> {
            Ok(self.raw.get(realm.as_str()).cloned())
        }
    }

    // Regression (over the in-memory head path used by every network surface): a
    // child realm must be able to re-enable a skills toggle its parent disabled,
    // even though the child's value (`true`) equals the struct default — only the
    // presence-aware merge can carry it. Before `merge_skills_from_toml_presence`
    // existed, the `!= default` value-merge silently kept the parent's `false`.
    #[tokio::test]
    async fn over_head_child_re_enables_inherited_disabled_skills() {
        use crate::connection::{RealmConfigSection, RealmId};

        let mut global = Config::default();
        global.skills.enabled = false;
        global
            .realm
            .insert("global".to_string(), RealmConfigSection::default());

        let mut child = Config::default();
        child.skills.enabled = true; // == struct default; only presence can carry it
        child.realm.insert(
            "child".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::global()),
                ..Default::default()
            },
        );

        let mut docs = std::collections::BTreeMap::new();
        docs.insert("global".to_string(), global);
        let mut raw = std::collections::BTreeMap::new();
        raw.insert(
            "global".to_string(),
            toml::from_str("[skills]\nenabled = false\n").expect("parse global toml"),
        );
        raw.insert(
            "child".to_string(),
            toml::from_str("[skills]\nenabled = true\n").expect("parse child toml"),
        );

        let reader = EffectiveConfigReader::new(Arc::new(RawMapSource { docs, raw }));
        let eff = reader
            .effective_config_over_head(&RealmId::parse("child").unwrap(), child)
            .await
            .expect("compose over head");
        assert!(
            eff.skills.enabled,
            "child must re-enable an inherited-disabled skills toggle via presence-aware merge"
        );
    }

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
        let config = Config {
            max_tokens: Some(8192),
            ..Config::default()
        };
        let original_max_tokens = config.max_tokens;
        let bumped = original_max_tokens
            .expect("max_tokens set above")
            .saturating_add(1);
        let previewed =
            apply_config_patch_preview(&config, serde_json::json!({ "max_tokens": bumped }))
                .expect("scalar patch should preview cleanly");
        assert_eq!(
            previewed.max_tokens,
            Some(bumped),
            "preview reflects the patch"
        );
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
