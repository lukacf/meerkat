//! Config store abstraction.

use crate::config::{Config, ConfigDelta, ConfigError, ConfigWarning, PersistedConfigDocument};
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

    /// Like [`Self::get`], also returning the typed load warnings for legacy
    /// shapes the persisted document carried (see
    /// [`Config::from_persisted_toml`]). Default: no warnings.
    async fn get_with_warnings(&self) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        Ok((self.get().await?, Vec::new()))
    }

    /// The config [`Self::patch`] would write for `delta`, without persisting
    /// it, plus the warnings that write would raise. Surfaces validate this
    /// preview before committing. Default: the delta merged onto
    /// [`Self::get`] via [`apply_config_patch_preview`].
    async fn patch_preview(
        &self,
        delta: &ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        apply_config_patch_preview(&self.get().await?, delta.0.clone())
            .map(|config| (config, Vec::new()))
    }

    /// Like [`Self::patch`], also returning the typed warnings for legacy
    /// state the write normalized or dropped. Default: no warnings.
    async fn patch_with_warnings(
        &self,
        delta: ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        Ok((self.patch(delta).await?, Vec::new()))
    }
}

/// A [`ConfigWarning`] raised while loading one realm's own config document
/// during inheritance composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmConfigWarning {
    /// The realm whose document produced the warning.
    pub realm: crate::connection::RealmId,
    /// The typed warning.
    pub warning: ConfigWarning,
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

    /// Like [`Self::config_for_realm`], also returning the typed load
    /// warnings the document produced (see [`Config::from_persisted_toml`]).
    ///
    /// Default: no warnings. Sources that parse persisted documents override
    /// this so surfaces can report normalized legacy shapes.
    async fn config_for_realm_with_warnings(
        &self,
        realm: &crate::connection::RealmId,
    ) -> Result<Option<(Config, Vec<ConfigWarning>)>, ConfigError> {
        Ok(self
            .config_for_realm(realm)
            .await?
            .map(|config| (config, Vec::new())))
    }

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
        let (config, warnings) = self.effective_config_with_warnings(head).await?;
        report_realm_config_warnings_once(&warnings);
        Ok(config)
    }

    /// Like [`Self::effective_config`], also returning the typed load warnings
    /// raised by each composed realm document, in fetch order. Documents that
    /// are fetched but not on the resolved chain (the implicit `global` tail
    /// when no document declares `[realm.global]`) are not reported.
    pub async fn effective_config_with_warnings(
        &self,
        head: &crate::connection::RealmId,
    ) -> Result<(Config, Vec<RealmConfigWarning>), ConfigError> {
        use crate::connection::{MAX_REALM_CHAIN_DEPTH, RealmId};
        use std::collections::{BTreeMap, BTreeSet};

        let mut docs: BTreeMap<RealmId, Config> = BTreeMap::new();
        let mut raw_docs: BTreeMap<RealmId, toml::Value> = BTreeMap::new();
        let mut seen: BTreeSet<RealmId> = BTreeSet::new();
        let mut warnings: Vec<RealmConfigWarning> = Vec::new();
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
            if let Some((doc, doc_warnings)) =
                self.source.config_for_realm_with_warnings(&realm).await?
            {
                warnings.extend(doc_warnings.into_iter().map(|warning| RealmConfigWarning {
                    realm: realm.clone(),
                    warning,
                }));
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
            && let Some((doc, doc_warnings)) =
                self.source.config_for_realm_with_warnings(&global).await?
        {
            warnings.extend(doc_warnings.into_iter().map(|warning| RealmConfigWarning {
                realm: global.clone(),
                warning,
            }));
            if let Some(raw) = self.source.raw_config_for_realm(&global).await? {
                raw_docs.insert(global.clone(), raw);
            }
            docs.insert(global, doc);
        }

        Ok(compose_reporting_chain_warnings(
            &docs, &raw_docs, head, warnings,
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
        let (config, warnings) = self
            .effective_config_over_head_with_warnings(head, head_config)
            .await?;
        report_realm_config_warnings_once(&warnings);
        Ok(config)
    }

    /// Like [`Self::effective_config_over_head`], also returning the typed load
    /// warnings raised by the fetched ANCESTOR documents on the resolved chain.
    /// The caller owns the head config, so it reports the head's own warnings.
    pub async fn effective_config_over_head_with_warnings(
        &self,
        head: &crate::connection::RealmId,
        head_config: Config,
    ) -> Result<(Config, Vec<RealmConfigWarning>), ConfigError> {
        use crate::connection::{MAX_REALM_CHAIN_DEPTH, RealmId};
        use std::collections::{BTreeMap, BTreeSet};

        let mut warnings: Vec<RealmConfigWarning> = Vec::new();
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
            if let Some((doc, doc_warnings)) =
                self.source.config_for_realm_with_warnings(&realm).await?
            {
                warnings.extend(doc_warnings.into_iter().map(|warning| RealmConfigWarning {
                    realm: realm.clone(),
                    warning,
                }));
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
            && let Some((doc, doc_warnings)) =
                self.source.config_for_realm_with_warnings(&global).await?
        {
            warnings.extend(doc_warnings.into_iter().map(|warning| RealmConfigWarning {
                realm: global.clone(),
                warning,
            }));
            if let Some(raw) = self.source.raw_config_for_realm(&global).await? {
                raw_docs.insert(global.clone(), raw);
            }
            docs.insert(global, doc);
        }

        Ok(compose_reporting_chain_warnings(
            &docs, &raw_docs, head, warnings,
        )?)
    }
}

/// Log composed-document warnings once per realm per process for callers
/// that do not report them to an operator (server surfaces compose on every
/// request).
fn report_realm_config_warnings_once(warnings: &[RealmConfigWarning]) {
    static REPORTED: std::sync::Mutex<
        std::collections::BTreeSet<(crate::connection::RealmId, ConfigWarning)>,
    > = std::sync::Mutex::new(std::collections::BTreeSet::new());
    for RealmConfigWarning { realm, warning } in warnings {
        let first = REPORTED
            .lock()
            .map(|mut reported| reported.insert((realm.clone(), *warning)))
            .unwrap_or(true);
        if first {
            tracing::warn!(%realm, %warning, "normalized persisted realm config document");
        }
    }
}

/// Compose `head`'s effective config and keep only the warnings of documents
/// on the resolved chain: a fetched document the composition does not fold
/// has no effect, so its legacy shapes are not the operator's problem here.
fn compose_reporting_chain_warnings(
    docs: &std::collections::BTreeMap<crate::connection::RealmId, Config>,
    raw_docs: &std::collections::BTreeMap<crate::connection::RealmId, toml::Value>,
    head: &crate::connection::RealmId,
    mut warnings: Vec<RealmConfigWarning>,
) -> Result<(Config, Vec<RealmConfigWarning>), crate::connection::RealmChainError> {
    let (config, chain) = crate::config::compose_effective_config_with_chain(docs, raw_docs, head)?;
    warnings.retain(|warning| chain.realms().contains(&warning.realm));
    Ok((config, warnings))
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

    async fn get_with_warnings(&self) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        self.inner.get_with_warnings().await
    }

    async fn patch_preview(
        &self,
        delta: &ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        self.inner.patch_preview(delta).await
    }

    async fn patch_with_warnings(
        &self,
        delta: ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        self.inner.patch_with_warnings(delta).await
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

    /// Read the document as persisted (legacy keys removed, legacy value
    /// shapes kept). Loads normalize it; patches merge onto it.
    async fn read_persisted(&self) -> Result<PersistedConfigDocument, ConfigError> {
        if self.create_if_missing {
            self.ensure_exists().await?;
        }

        if !tokio::fs::try_exists(&self.path).await? {
            return Ok(PersistedConfigDocument::absent());
        }

        let bytes = tokio::fs::read(&self.path).await?;
        let content = String::from_utf8(bytes).map_err(ConfigError::Utf8)?;
        PersistedConfigDocument::parse(&content)
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
        let (config, warnings) = self.get_with_warnings().await?;
        report_config_load_warnings_once(&self.path, &warnings);
        Ok(config)
    }

    /// The file is never rewritten by a load; a later write persists the
    /// normalized value.
    async fn get_with_warnings(&self) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        Ok(self.read_persisted().await?.into_loaded())
    }

    /// The delta merges onto the document AS PERSISTED, so a patch that adds
    /// a chain to a pre-0.8.37 `enabled = true` document keeps fallback on.
    async fn patch_preview(
        &self,
        delta: &ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        self.read_persisted().await?.apply_patch(delta.0.clone())
    }

    async fn patch_with_warnings(
        &self,
        delta: ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        let (updated, warnings) = self.patch_preview(&delta).await?;
        updated.validate(self.catalog)?;
        self.set(updated.clone()).await?;
        Ok((updated, warnings))
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
        let (updated, warnings) = self.patch_with_warnings(delta).await?;
        report_config_load_warnings_once(&self.path, &warnings);
        Ok(updated)
    }
}

/// Log each load warning once per document per process. Server surfaces read
/// their documents on every request, so repeating the warning would flood the
/// log; surfaces that print to an operator use the `*_with_warnings` readers.
fn report_config_load_warnings_once(path: &Path, warnings: &[ConfigWarning]) {
    static REPORTED: std::sync::Mutex<std::collections::BTreeSet<(PathBuf, ConfigWarning)>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    for warning in warnings {
        let first = REPORTED
            .lock()
            .map(|mut reported| reported.insert((path.to_path_buf(), *warning)))
            .unwrap_or(true);
        if first {
            tracing::warn!(path = %path.display(), %warning, "normalized persisted config document");
        }
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

    const LEGACY_MODEL_FALLBACK_DOC: &str = "[model_fallback]\nenabled = true\n";

    /// Source double over per-realm files that routes through
    /// `FileConfigStore::get_with_warnings`, like the filesystem source.
    struct FileDocSource {
        root: PathBuf,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl RealmConfigSource for FileDocSource {
        async fn config_for_realm(
            &self,
            realm: &crate::connection::RealmId,
        ) -> Result<Option<Config>, ConfigError> {
            Ok(self
                .config_for_realm_with_warnings(realm)
                .await?
                .map(|(config, _)| config))
        }

        async fn config_for_realm_with_warnings(
            &self,
            realm: &crate::connection::RealmId,
        ) -> Result<Option<(Config, Vec<crate::config::ConfigWarning>)>, ConfigError> {
            let path = self.root.join(realm.as_str()).join("config.toml");
            if !tokio::fs::try_exists(&path).await? {
                return Ok(None);
            }
            FileConfigStore::new(path, *crate::model_profile::test_catalog::TEST_CATALOG)
                .get_with_warnings()
                .await
                .map(Some)
        }
    }

    #[tokio::test]
    async fn file_store_loads_legacy_model_fallback_default_disabled_without_rewriting()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, LEGACY_MODEL_FALLBACK_DOC).await?;
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );

        let (config, warnings) = store.get_with_warnings().await?;
        assert!(!config.model_fallback.is_enabled());
        assert_eq!(
            warnings,
            vec![crate::config::ConfigWarning::LegacyModelFallbackDefault]
        );
        assert!(!store.get().await?.model_fallback.is_enabled());
        assert_eq!(
            tokio::fs::read_to_string(&path).await?,
            LEGACY_MODEL_FALLBACK_DOC,
            "loading never rewrites the document"
        );
        Ok(())
    }

    /// Read-modify-write of a legacy document (for example `auth login`
    /// writing a binding into the pre-0.8.37 global doc) must not be bricked:
    /// the merged result still has the legacy shape, so the write persists
    /// fallback disabled and reports the warning.
    #[tokio::test]
    async fn file_store_patch_over_legacy_model_fallback_default_persists_disabled()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, LEGACY_MODEL_FALLBACK_DOC).await?;
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );

        let (preview, preview_warnings) = store
            .patch_preview(&ConfigDelta(serde_json::json!({ "max_tokens": 1234 })))
            .await?;
        assert_eq!(preview.model_fallback.enabled, Some(false));
        assert_eq!(
            preview_warnings,
            vec![crate::config::ConfigWarning::LegacyModelFallbackDefault]
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await?,
            LEGACY_MODEL_FALLBACK_DOC,
            "a preview never writes"
        );

        let (updated, patch_warnings) = store
            .patch_with_warnings(ConfigDelta(serde_json::json!({ "max_tokens": 1234 })))
            .await?;
        assert_eq!(updated.max_tokens, Some(1234));
        assert_eq!(updated.model_fallback.enabled, Some(false));
        assert_eq!(
            patch_warnings,
            vec![crate::config::ConfigWarning::LegacyModelFallbackDefault]
        );
        let persisted: Config = toml::from_str(&tokio::fs::read_to_string(&path).await?)?;
        assert_eq!(persisted.model_fallback.enabled, Some(false));

        let (reloaded, warnings) = store.get_with_warnings().await?;
        assert_eq!(reloaded.model_fallback.enabled, Some(false));
        assert!(
            warnings.is_empty(),
            "the rewritten document is no longer legacy"
        );
        Ok(())
    }

    /// Following the legacy warning's own advice by PATCH ("add a
    /// [[model_fallback.chain]] target") must keep fallback on: the delta is
    /// merged onto the RAW persisted document, and only a merged result that
    /// still has the legacy shape is normalized.
    #[tokio::test]
    async fn file_store_patch_adding_chain_to_legacy_default_keeps_fallback_enabled()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, LEGACY_MODEL_FALLBACK_DOC).await?;
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );

        let (updated, warnings) = store
            .patch_with_warnings(ConfigDelta(serde_json::json!({
                "model_fallback": {
                    "chain": [{ "model": "backup-openai", "provider": "openai" }]
                }
            })))
            .await?;
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(updated.model_fallback.enabled, Some(true));
        assert_eq!(updated.model_fallback.chain.len(), 1);

        let persisted: Config = toml::from_str(&tokio::fs::read_to_string(&path).await?)?;
        assert_eq!(persisted.model_fallback.enabled, Some(true));
        assert_eq!(persisted.model_fallback.chain.len(), 1);
        let (reloaded, warnings) = store.get_with_warnings().await?;
        assert!(reloaded.model_fallback.is_enabled());
        assert!(warnings.is_empty(), "no warning once the chain is present");
        Ok(())
    }

    /// A persisted `use_catalog_default_chain` (documented through 0.8.36)
    /// loads; the next write drops it; a write that INTRODUCES it is refused.
    #[tokio::test]
    async fn file_store_loads_legacy_catalog_chain_key_and_rejects_writes_introducing_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        let legacy = "max_tokens = 99\n\n[model_fallback]\nuse_catalog_default_chain = true\n";
        tokio::fs::write(&path, legacy).await?;
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );

        let loaded = store.get().await?;
        assert!(!loaded.model_fallback.is_enabled());
        assert_eq!(loaded.max_tokens, Some(99));
        let (_, warnings) = store.get_with_warnings().await?;
        assert_eq!(
            warnings,
            vec![crate::config::ConfigWarning::LegacyModelFallbackCatalogChain]
        );
        assert_eq!(tokio::fs::read_to_string(&path).await?, legacy);

        let error = store
            .patch(ConfigDelta(serde_json::json!({
                "model_fallback": { "use_catalog_default_chain": true }
            })))
            .await
            .expect_err("a write introducing the removed key is refused");
        assert!(
            error.to_string().contains("use_catalog_default_chain"),
            "{error}"
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await?,
            legacy,
            "a refused write leaves the document untouched"
        );

        let (updated, warnings) = store
            .patch_with_warnings(ConfigDelta(serde_json::json!({ "max_tokens": 1234 })))
            .await?;
        assert_eq!(updated.max_tokens, Some(1234));
        assert_eq!(
            warnings,
            vec![crate::config::ConfigWarning::LegacyModelFallbackCatalogChain],
            "the write that drops the ignored key reports it"
        );
        let rewritten = tokio::fs::read_to_string(&path).await?;
        assert!(
            !rewritten.contains("use_catalog_default_chain"),
            "the next write drops the ignored key: {rewritten}"
        );
        let _: Config = toml::from_str(&rewritten)?;
        Ok(())
    }

    /// The implicit `global` tail is fetched even when no document declares
    /// `[realm.global]`, but then it is not composed; its load warnings must
    /// not be reported for a doc that plays no part in the effective config.
    #[tokio::test]
    async fn effective_reader_skips_warnings_for_global_doc_outside_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::connection::RealmId;

        let temp = tempfile::tempdir()?;
        let global_dir = temp.path().join("global");
        tokio::fs::create_dir_all(&global_dir).await?;
        tokio::fs::write(global_dir.join("config.toml"), LEGACY_MODEL_FALLBACK_DOC).await?;
        let child_dir = temp.path().join("child");
        tokio::fs::create_dir_all(&child_dir).await?;
        tokio::fs::write(child_dir.join("config.toml"), "max_tokens = 77\n").await?;

        let reader = EffectiveConfigReader::new(Arc::new(FileDocSource {
            root: temp.path().to_path_buf(),
        }));
        let child = RealmId::parse("child")?;
        let (config, warnings) = reader.effective_config_with_warnings(&child).await?;
        assert_eq!(config.max_tokens, Some(77));
        assert!(
            warnings.is_empty(),
            "global is not on the chain, so its doc is not reported: {warnings:?}"
        );
        Ok(())
    }

    /// Writes stay strict: a write that INTRODUCES `enabled = true` with an
    /// empty chain is refused on every store.
    #[tokio::test]
    async fn stores_reject_writes_that_introduce_enabled_fallback_without_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        let legacy: Config = toml::from_str(LEGACY_MODEL_FALLBACK_DOC)?;
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        let file_store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let error = file_store
            .set(legacy.clone())
            .await
            .expect_err("file set must reject");
        assert!(
            error
                .to_string()
                .contains("model_fallback.enabled = true requires a nonempty explicit chain"),
            "{error}"
        );
        assert!(!tokio::fs::try_exists(&path).await?, "nothing was written");
        let error = file_store
            .patch(ConfigDelta(
                serde_json::json!({ "model_fallback": { "enabled": true } }),
            ))
            .await
            .expect_err("file patch must reject");
        assert!(
            error.to_string().contains("nonempty explicit chain"),
            "{error}"
        );

        let memory = MemoryConfigStore::new(
            Config::default(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        assert!(memory.set(legacy).await.is_err(), "memory set must reject");
        assert!(
            memory
                .patch(ConfigDelta(
                    serde_json::json!({ "model_fallback": { "enabled": true } }),
                ))
                .await
                .is_err(),
            "memory patch must reject"
        );
        Ok(())
    }

    #[tokio::test]
    async fn effective_reader_reports_legacy_model_fallback_warning_per_realm()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::connection::RealmId;

        let temp = tempfile::tempdir()?;
        let global_dir = temp.path().join("global");
        tokio::fs::create_dir_all(&global_dir).await?;
        tokio::fs::write(global_dir.join("config.toml"), LEGACY_MODEL_FALLBACK_DOC).await?;
        let child_dir = temp.path().join("child");
        tokio::fs::create_dir_all(&child_dir).await?;
        tokio::fs::write(
            child_dir.join("config.toml"),
            "[realm.child]\nparent = \"global\"\n",
        )
        .await?;

        let reader = EffectiveConfigReader::new(Arc::new(FileDocSource {
            root: temp.path().to_path_buf(),
        }));
        let child = RealmId::parse("child")?;
        let (config, warnings) = reader.effective_config_with_warnings(&child).await?;
        assert!(!config.model_fallback.is_enabled());
        config.validate(*crate::model_profile::test_catalog::TEST_CATALOG)?;
        assert_eq!(
            warnings,
            vec![RealmConfigWarning {
                realm: RealmId::global(),
                warning: crate::config::ConfigWarning::LegacyModelFallbackDefault,
            }]
        );
        assert!(
            !reader
                .effective_config(&child)
                .await?
                .model_fallback
                .is_enabled()
        );
        Ok(())
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
                server: None,
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
                credential_account: None,
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
