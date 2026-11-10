//! Config store abstraction.

use crate::config::{Config, ConfigDelta, ConfigError, ConfigWarning, PersistedConfigDocument};
use crate::model_profile::ModelCatalog;
use crate::realm_admission::{AdmissionFence, AdmissionGuard};
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

    /// Persist the provided config as a full replace.
    ///
    /// A document-backed store writes every field explicitly, so in a realm
    /// document each one then overrides the parent realm instead of
    /// inheriting it. Change individual keys with [`Self::patch`].
    async fn set(&self, config: Config) -> Result<(), ConfigError>;

    /// Apply an RFC 7396 JSON merge patch and return the updated config.
    ///
    /// A document-backed store writes only the keys the patch names; the
    /// others keep their presence, so a realm document keeps inheriting them.
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

    /// The admission fence of this store's document
    /// ([`crate::realm_admission`]): every write of the document holds it
    /// EXCLUSIVE. `None` for a store with no document (in memory), whose
    /// writes need no fence.
    fn admission_fence(&self) -> Option<AdmissionFence> {
        None
    }

    /// [`Self::set`] as the raw write of a caller that already holds `guard`
    /// over the document's admission fence and the document's config custody
    /// (a [`crate::ConfigRuntime`]): it takes no lock of its own. A
    /// document-backed store refuses a guard that does not hold its fence
    /// ([`ConfigError::AdmissionFenceUnavailable`]) with nothing written; a
    /// store with no document accepts any guard. Only the config runtime
    /// calls it.
    async fn set_held(&self, guard: &AdmissionGuard, config: Config) -> Result<(), ConfigError>;

    /// [`Self::patch`] as the raw write under a held guard and custody, as
    /// [`Self::set_held`].
    async fn patch_held(
        &self,
        guard: &AdmissionGuard,
        delta: ConfigDelta,
    ) -> Result<Config, ConfigError>;

    /// [`Self::patch_with_warnings`] as the raw write under a held guard and
    /// custody, as [`Self::set_held`]. Default: [`Self::patch_held`] with no
    /// warnings.
    async fn patch_held_with_warnings(
        &self,
        guard: &AdmissionGuard,
        delta: ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        Ok((self.patch_held(guard, delta).await?, Vec::new()))
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

    /// The file a store that guards its writes against these documents
    /// ([`FileConfigStore::with_realm_chain`]) holds an exclusive advisory
    /// lock on from before it reads its own document, through its
    /// composition check, to after its write. Writers of documents one host
    /// composes then take turns, in one process and across processes, so no
    /// check reads a document another writer is about to replace.
    ///
    /// Default: `None`, and such writes stay unserialized. The filesystem
    /// source returns one file under its state root, which every guarded
    /// store over that root locks (the user document's included).
    fn composition_lock_path(&self) -> Option<PathBuf> {
        None
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
        let ChainDocs {
            docs,
            raw_docs,
            warnings,
        } = self.chain_docs(head).await?;
        compose_reporting_chain_warnings(&docs, &raw_docs, head, warnings)
    }

    /// The realm whose OWN document defines the MCP server `name` in
    /// `head`'s effective configuration, with that definition: the nearest
    /// chain member, head first, whose document lists the name. Servers fold
    /// by name with the child winning, so this is the definition the
    /// composition yields for the name. The documents and the chain are
    /// discovered and resolved exactly as [`Self::effective_config`] does.
    /// `None` when no document on the chain defines the name.
    pub async fn mcp_server_owner(
        &self,
        head: &crate::connection::RealmId,
        name: &str,
    ) -> Result<
        Option<(
            crate::connection::RealmId,
            crate::mcp_config::McpServerConfig,
        )>,
        ConfigError,
    > {
        let ChainDocs { docs, .. } = self.chain_docs(head).await?;
        Ok(crate::config::mcp_server_definition_owner(
            &docs, head, name,
        )?)
    }

    /// The documents [`Self::effective_config`] composes for `head`: the head
    /// document, its `parent` edges and the implicit `global` tail.
    async fn chain_docs(
        &self,
        head: &crate::connection::RealmId,
    ) -> Result<ChainDocs, ConfigError> {
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

        Ok(ChainDocs {
            docs,
            raw_docs,
            warnings,
        })
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
        let ChainDocs {
            docs,
            raw_docs,
            warnings,
        } = self.chain_docs_over_head(head, head_config).await?;
        compose_reporting_chain_warnings(&docs, &raw_docs, head, warnings)
    }

    /// [`Self::mcp_server_owner`] with the HEAD realm's own document
    /// supplied by the caller, as [`Self::effective_config_over_head`]
    /// composes it; ancestors are fetched from the source.
    pub async fn mcp_server_owner_over_head(
        &self,
        head: &crate::connection::RealmId,
        head_config: Config,
        name: &str,
    ) -> Result<
        Option<(
            crate::connection::RealmId,
            crate::mcp_config::McpServerConfig,
        )>,
        ConfigError,
    > {
        let ChainDocs { docs, .. } = self.chain_docs_over_head(head, head_config).await?;
        Ok(crate::config::mcp_server_definition_owner(
            &docs, head, name,
        )?)
    }

    /// The `[confinement]` and `[governance]` `head` composes when its own
    /// document states `realms` as its realm sections (only each section's
    /// presence and parent edge decide a composition) and `confinement` and
    /// `governance` as its own sections. The documents and the chain are
    /// discovered and resolved, and the sections composed, exactly as
    /// [`Self::effective_config_over_head`] does for that head document. The
    /// layer check is not applied.
    async fn administered_with_realms(
        &self,
        head: &crate::connection::RealmId,
        realms: &RealmEdges,
        confinement: &crate::ConfinementConfig,
        governance: &Option<crate::GovernanceConfig>,
    ) -> Result<(crate::ConfinementConfig, Option<crate::GovernanceConfig>), ConfigError> {
        let head_config = administration_document(realms, confinement, governance);
        let ChainDocs { docs, raw_docs, .. } = self.chain_docs_over_head(head, head_config).await?;
        let composed = crate::config::compose_effective_config(&docs, &raw_docs, head)?;
        Ok((composed.confinement, composed.governance))
    }

    /// The `[confinement]` and `[governance]` `head` composes, from the
    /// documents and the chain [`Self::effective_config`] discovers and
    /// resolves (`head` may have no document of its own). The layer check
    /// is not applied.
    async fn administered(
        &self,
        head: &crate::connection::RealmId,
    ) -> Result<(crate::ConfinementConfig, Option<crate::GovernanceConfig>), ConfigError> {
        let ChainDocs { docs, raw_docs, .. } = self.chain_docs(head).await?;
        let composed = crate::config::compose_effective_config(&docs, &raw_docs, head)?;
        Ok((composed.confinement, composed.governance))
    }

    /// The documents [`Self::effective_config_over_head`] composes for
    /// `head`: the caller's head document, its `parent` edges and the
    /// implicit `global` tail.
    async fn chain_docs_over_head(
        &self,
        head: &crate::connection::RealmId,
        head_config: Config,
    ) -> Result<ChainDocs, ConfigError> {
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

        Ok(ChainDocs {
            docs,
            raw_docs,
            warnings,
        })
    }
}

/// The per-realm documents fetched for one composition, with their raw TOML
/// (presence) and load warnings.
struct ChainDocs {
    docs: std::collections::BTreeMap<crate::connection::RealmId, Config>,
    raw_docs: std::collections::BTreeMap<crate::connection::RealmId, toml::Value>,
    warnings: Vec<RealmConfigWarning>,
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
) -> Result<(Config, Vec<RealmConfigWarning>), ConfigError> {
    let (config, chain) = crate::config::compose_effective_config_with_chain(docs, raw_docs, head)?;
    crate::config::check_composed_confinement(docs, &chain)?;
    crate::config::check_composed_command_servers(docs, &chain, &config)?;
    warnings.retain(|warning| chain.realms().contains(&warning.realm));
    Ok((config, warnings))
}

/// In-memory config store for ephemeral settings. Its writes keep the
/// config's own `[confinement]` and `[governance]` sections; it composes no
/// realm chain, so it does not guard the realm sections that compose an
/// ancestor's ones ([`FileConfigStore::with_realm_chain`] does).
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
        let mut current = self.config.write().await;
        keep_governance(&current.governance, &config.governance)?;
        keep_confinement(&current.confinement, &config.confinement)?;
        keep_credential_commands(&credential_commands(&current), &config)?;
        config.validate_for_write(self.catalog, &current.tools.mcp_servers)?;
        *current = config;
        Ok(())
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let mut config = self.config.write().await;
        let mut value = serde_json::to_value(&*config).map_err(ConfigError::Json)?;
        merge_patch(&mut value, delta.0);
        let updated: Config = serde_json::from_value(value).map_err(ConfigError::Json)?;
        keep_governance(&config.governance, &updated.governance)?;
        keep_confinement(&config.confinement, &updated.confinement)?;
        keep_credential_commands(&credential_commands(&config), &updated)?;
        updated.validate_for_write(self.catalog, &config.tools.mcp_servers)?;
        *config = updated.clone();
        Ok(updated)
    }

    /// No document, so no fence: any guard is accepted.
    async fn set_held(&self, _guard: &AdmissionGuard, config: Config) -> Result<(), ConfigError> {
        self.set(config).await
    }

    /// No document, so no fence: any guard is accepted.
    async fn patch_held(
        &self,
        _guard: &AdmissionGuard,
        delta: ConfigDelta,
    ) -> Result<Config, ConfigError> {
        self.patch(delta).await
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

    fn admission_fence(&self) -> Option<AdmissionFence> {
        self.inner.admission_fence()
    }

    async fn set_held(&self, guard: &AdmissionGuard, config: Config) -> Result<(), ConfigError> {
        self.inner.set_held(guard, config).await
    }

    async fn patch_held(
        &self,
        guard: &AdmissionGuard,
        delta: ConfigDelta,
    ) -> Result<Config, ConfigError> {
        self.inner.patch_held(guard, delta).await
    }

    async fn patch_held_with_warnings(
        &self,
        guard: &AdmissionGuard,
        delta: ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        self.inner.patch_held_with_warnings(guard, delta).await
    }
}

/// File-backed config store with optional bootstrap template.
///
/// Every write is fenced ([`crate::realm_admission`]). Its public writes
/// ([`ConfigStore::set`], [`ConfigStore::patch`],
/// [`ConfigStore::patch_with_warnings`]) run the config runtime's sequence
/// for the document itself: its admission fence, its config custody, the
/// write and the generation sidecar beside it, settled together on an owned
/// task, so they move the generation every [`crate::ConfigRuntime`] whose
/// sidecar lies beside the same document reads. A host's runtime of the
/// `global` realm keeps its own sidecar in that realm's directory, which
/// these writes do not move. A held fence refuses them
/// ([`ConfigError::AdmissionInProgress`]) with nothing written. Its raw
/// writes ([`ConfigStore::set_held`] and the patch forms) take the guard of
/// a caller that holds the fence and the custody.
pub struct FileConfigStore {
    path: PathBuf,
    create_if_missing: bool,
    catalog: ModelCatalog,
    // The realm chain's documents are a shared source, which a panic cannot
    // leave half-changed, so the store stays `UnwindSafe` and
    // `RefUnwindSafe`.
    realm_chain: Option<std::panic::AssertUnwindSafe<RealmChainGuard>>,
}

impl Clone for FileConfigStore {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            create_if_missing: self.create_if_missing,
            catalog: self.catalog,
            realm_chain: self
                .realm_chain
                .as_ref()
                .map(|chain| std::panic::AssertUnwindSafe(chain.0.clone())),
        }
    }
}

impl FileConfigStore {
    /// Create a new file-backed store for an explicit path, validating
    /// against the injected model catalog (canonically
    /// `meerkat_models::canonical()`).
    ///
    /// Its writes keep the document's own `[confinement]` and
    /// `[governance]` sections, but a store for a realm's document does not
    /// guard the realm sections that compose its ancestors' ones until it
    /// is declared with [`Self::with_realm_chain`], as the stock hosts'
    /// stores are.
    pub fn new(path: PathBuf, catalog: ModelCatalog) -> Self {
        Self {
            path,
            create_if_missing: false,
            catalog,
            realm_chain: None,
        }
    }

    /// Create a store that bootstraps a global config file if missing.
    ///
    /// The document is created from its template only when its admission
    /// fence and config custody are free now: it never waits, and while
    /// either is held it creates nothing and the store reads the missing
    /// document as empty.
    pub async fn global(catalog: ModelCatalog) -> Result<Self, ConfigError> {
        let path = Config::global_config_path()
            .ok_or_else(|| ConfigError::MissingField("HOME".to_string()))?;
        let store = Self {
            path,
            create_if_missing: true,
            catalog,
            realm_chain: None,
        };
        store.create_if_unheld().await?;
        Ok(store)
    }

    /// Create a store rooted at the provided project directory.
    pub fn project(project_root: impl Into<PathBuf>, catalog: ModelCatalog) -> Self {
        let root = project_root.into();
        Self::new(root.join(".rkat").join("config.toml"), catalog)
    }

    /// Declare this document `head`'s own realm document, composed with the
    /// other documents `documents` supplies (its ancestors and the user
    /// document), as a host does at start. A write that changes the realm
    /// sections the document states (the realm's own parent edge, or a
    /// section it states for another realm, such as `[realm.global]`) is
    /// then refused unless the realm composes exactly the same
    /// `[confinement]` and the same `[governance]` after it
    /// ([`ConfigError::CompositionAdministration`]): ordinary config write
    /// permission cannot cut the realm from an ancestor's requirement or
    /// governance, nor add either. A store for the user document (`head` is
    /// `global`) also refuses a write after which another realm would
    /// compose either differently: a realm whose own document states no
    /// parent and no `[realm.global]` (the user document's `[realm.global]`
    /// decides whether `global` is on its chain), and each realm the user
    /// document states a section for, both from its own document in
    /// `documents` and as if it had none (on another state root that shares
    /// the user document, it takes the user document's section). Nothing is
    /// written.
    ///
    /// Only a store declared this way guards the realm's composition. A
    /// store built with [`Self::new`] alone, and a [`MemoryConfigStore`],
    /// keep the document's own `[confinement]` and `[governance]` sections
    /// but not the realm sections that compose the ancestors' ones.
    #[must_use]
    pub fn with_realm_chain(
        mut self,
        head: crate::connection::RealmId,
        documents: Arc<dyn RealmConfigSource>,
    ) -> Self {
        self.realm_chain = Some(std::panic::AssertUnwindSafe(RealmChainGuard {
            head,
            documents,
        }));
        self
    }

    /// Return the config file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The composition lock a store declared with a realm chain holds
    /// across a write ([`RealmConfigSource::composition_lock_path`]), or
    /// `None` when there is no chain or its documents name no lock. Lock
    /// order: a caller's own lock first (such as `ConfigRuntime`'s), then
    /// this one; nothing takes them the other way round.
    async fn composition_lock(&self) -> Result<Option<CompositionLock>, ConfigError> {
        match self
            .realm_chain
            .as_ref()
            .and_then(|chain| chain.documents.composition_lock_path())
        {
            Some(path) => CompositionLock::acquire(path).await.map(Some),
            None => Ok(None),
        }
    }

    /// The document text as persisted; empty when the file does not exist.
    /// A store that bootstraps its document creates it first, only when its
    /// fence and custody are free now ([`Self::create_if_unheld`]).
    async fn read_content(&self) -> Result<String, ConfigError> {
        if self.create_if_missing {
            self.create_if_unheld().await?;
        }
        self.read_persisted_text().await
    }

    /// The document text as a held write reads it: a store that bootstraps
    /// its document creates it from the template first, under the caller's
    /// guard and custody.
    async fn read_content_held(&self, guard: &AdmissionGuard) -> Result<String, ConfigError> {
        if self.create_if_missing && !tokio::fs::try_exists(&self.path).await? {
            self.write_content(guard, Config::template_toml()).await?;
        }
        self.read_persisted_text().await
    }

    async fn read_persisted_text(&self) -> Result<String, ConfigError> {
        if !tokio::fs::try_exists(&self.path).await? {
            return Ok(String::new());
        }

        let bytes = tokio::fs::read(&self.path).await?;
        String::from_utf8(bytes).map_err(ConfigError::Utf8)
    }

    /// The document's own MCP servers, for a write's literal-server check.
    /// A missing or unreadable document has none, so no entry of the write
    /// is exempted; a full replace never depends on parsing the old
    /// document.
    async fn persisted_mcp_servers_for_write(&self) -> Vec<crate::mcp_config::McpServerConfig> {
        let content = match tokio::fs::read(&self.path).await {
            Ok(bytes) => String::from_utf8(bytes).ok(),
            Err(_) => None,
        };
        content
            .and_then(|content| PersistedConfigDocument::parse(&content).ok())
            .map(|document| document.into_loaded().0.tools.mcp_servers)
            .unwrap_or_default()
    }

    /// Read the document as persisted (legacy keys removed, legacy value
    /// shapes kept). Loads normalize it.
    async fn read_persisted(&self) -> Result<PersistedConfigDocument, ConfigError> {
        PersistedConfigDocument::parse(&self.read_content().await?)
    }

    /// Replace the document atomically under `guard`, which must hold the
    /// document's admission fence (a guard that does not is refused with
    /// nothing written): write a sibling temp file, sync it, rename it over
    /// the document.
    async fn write_content(
        &self,
        guard: &AdmissionGuard,
        content: &str,
    ) -> Result<(), ConfigError> {
        guard.covers(&AdmissionFence::beside(&self.path))?;
        let parent = parent_directory(&self.path);
        tokio::fs::create_dir_all(&parent).await?;
        let tmp_path = parent.join(format!(".config.tmp.{}", crate::time_compat::new_uuid_v7()));
        let mut tmp = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .await?;
        tmp.write_all(content.as_bytes()).await?;
        // The write's own result. The sync below waits for the write but
        // does not report its failure, so without this a write the
        // filesystem refused would publish an empty document as saved.
        tmp.flush().await?;
        tmp.sync_all().await?;
        drop(tmp);
        tokio::fs::rename(&tmp_path, &self.path).await?;
        Ok(())
    }

    /// Create the missing document from its template, as the config
    /// runtime writes (its fence, its custody, temp file, sync, rename, and
    /// the generation beside it), but only when the document's admission
    /// fence and config custody are both free now. It never waits: while
    /// either is held (by an admission, another writer, or a reader such as
    /// [`crate::ConfigRuntime::get`]), or the fence is unavailable, it creates
    /// nothing, and a read then sees the missing document as empty.
    async fn create_if_unheld(&self) -> Result<(), ConfigError> {
        if tokio::fs::try_exists(&self.path).await? {
            return Ok(());
        }
        let fences = self.admission_fence().into_iter().collect();
        let guard = match AdmissionGuard::try_acquire(fences) {
            Ok(guard) => guard,
            Err(refusal) => {
                tracing::debug!(path = %self.path.display(), %refusal, "config document not created on read");
                return Ok(());
            }
        };
        let custody = match crate::config_runtime::ConfigWriterLock::try_hold(&self.custody_path())
        {
            Ok(Some(custody)) => custody,
            Ok(None) => return Ok(()),
            Err(error) => {
                tracing::debug!(path = %self.path.display(), %error, "config document not created on read");
                return Ok(());
            }
        };
        let store = self.clone();
        let generation_path = self.generation_path();
        let held = Arc::clone(&guard);
        crate::realm_admission::settle_entered(guard, async move {
            let _custody = custody;
            if tokio::fs::try_exists(&store.path).await? {
                return Ok(());
            }
            store.write_content(&held, Config::template_toml()).await?;
            let current = crate::config_runtime::read_generation_at(&generation_path).await?;
            crate::config_runtime::write_generation_at(
                &held,
                &generation_path,
                current.saturating_add(1),
            )
            .await?;
            Ok::<_, ConfigError>(())
        })
        .await
        .map_err(|lost| ConfigError::Io(std::io::Error::other(lost.to_string())))?
    }

    /// The generation sidecar of this document's own writes, beside it: the
    /// one a [`crate::ConfigRuntime`] the surfaces build for the document's
    /// directory reads.
    fn generation_path(&self) -> PathBuf {
        parent_directory(&self.path).join(GENERATION_FILE)
    }

    /// The config custody lock of [`Self::generation_path`], as a
    /// [`crate::ConfigRuntime`] over that sidecar takes it.
    fn custody_path(&self) -> PathBuf {
        self.generation_path().with_extension("lock")
    }

    /// The config runtime of this document's own fenced writes.
    fn own_runtime(&self) -> crate::ConfigRuntime {
        crate::ConfigRuntime::new(Arc::new(self.clone()), self.generation_path())
    }
}

/// The generation sidecar's file name beside a document.
const GENERATION_FILE: &str = "config_state.json";

fn parent_directory(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
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

    /// The delta edits the document AS PERSISTED (see
    /// `PersistedConfigDocument::patch_content`): keys it does not name keep
    /// their presence, so a realm document keeps inheriting them, and a patch
    /// that adds a chain to a pre-0.8.37 `enabled = true` document keeps
    /// fallback on.
    async fn patch_preview(
        &self,
        delta: &ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        let patched =
            PersistedConfigDocument::patch_content(&self.read_content().await?, delta.0.clone())?;
        Ok((patched.config, patched.warnings))
    }

    async fn patch_with_warnings(
        &self,
        delta: ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        let (snapshot, warnings) = self.own_runtime().patch_with_warnings(delta, None).await?;
        Ok((snapshot.config, warnings))
    }

    /// A full replace writes every field of `config` explicitly. In a realm
    /// document each one then overrides the parent realm, so callers that
    /// change individual keys of a realm document patch it instead.
    async fn set(&self, config: Config) -> Result<(), ConfigError> {
        self.own_runtime().set(config, None).await?;
        Ok(())
    }

    async fn patch(&self, delta: ConfigDelta) -> Result<Config, ConfigError> {
        let (updated, warnings) = self.patch_with_warnings(delta).await?;
        report_config_load_warnings_once(&self.path, &warnings);
        Ok(updated)
    }

    /// The fence of the document's directory, named by its kind when this
    /// store is a realm's own document in the realm layout: the user
    /// document fence for the `global` realm's document, the realm's fence
    /// for another realm's, and otherwise the fence beside the document. The
    /// kind matters only in a set of several fences, which must be named by
    /// kind; every name of it is the same fence file.
    fn admission_fence(&self) -> Option<AdmissionFence> {
        let beside = AdmissionFence::beside(&self.path);
        let by_kind = self.realm_chain.as_ref().and_then(|chain| {
            if chain.head.is_global() {
                Some(AdmissionFence::user_document(beside.directory()))
            } else {
                let state_root = beside.directory().parent()?;
                AdmissionFence::realm(state_root, &chain.head).ok()
            }
        });
        Some(
            by_kind
                .filter(|fence| fence.path() == beside.path())
                .unwrap_or(beside),
        )
    }

    async fn patch_held_with_warnings(
        &self,
        guard: &AdmissionGuard,
        delta: ConfigDelta,
    ) -> Result<(Config, Vec<ConfigWarning>), ConfigError> {
        guard.covers(&AdmissionFence::beside(&self.path))?;
        // Held from before the read to after the write.
        let _composition = self.composition_lock().await?;
        let content = self.read_content_held(guard).await?;
        let patched = PersistedConfigDocument::patch_content(&content, delta.0)?;
        // The patch already parsed this content strictly, so this parse
        // succeeds; the comparisons are against the same bytes the patch
        // edits.
        let persisted =
            PersistedConfigDocument::parse(&content).map(|document| document.into_loaded().0)?;
        keep_governance(&persisted.governance, &patched.config.governance)?;
        keep_confinement(&persisted.confinement, &patched.config.confinement)?;
        keep_credential_commands(&credential_commands(&persisted), &patched.config)?;
        if let Some(chain) = &self.realm_chain {
            chain
                .keep_composed_administration(
                    realm_edges(&persisted),
                    &patched.config,
                    &persisted.confinement,
                    &persisted.governance,
                )
                .await?;
        }
        patched
            .config
            .validate_for_write(self.catalog, &persisted.tools.mcp_servers)?;
        self.write_content(guard, &patched.content).await?;
        Ok((patched.config, patched.warnings))
    }

    async fn set_held(&self, guard: &AdmissionGuard, config: Config) -> Result<(), ConfigError> {
        guard.covers(&AdmissionFence::beside(&self.path))?;
        // Held from before the read to after the write.
        let _composition = self.composition_lock().await?;
        let content = self.read_content_held(guard).await?;
        let governance = persisted_governance(&content)?;
        keep_governance(&governance, &config.governance)?;
        let confinement = persisted_confinement(&content)?;
        keep_confinement(&confinement, &config.confinement)?;
        keep_credential_commands(&persisted_credential_commands(&content), &config)?;
        if let Some(chain) = &self.realm_chain {
            chain
                .keep_composed_administration(
                    persisted_realm_edges(&content)?,
                    &config,
                    &confinement,
                    &governance,
                )
                .await?;
        }
        config.validate_for_write(self.catalog, &self.persisted_mcp_servers_for_write().await)?;
        let content = toml::to_string_pretty(&config).map_err(ConfigError::TomlSerialize)?;
        self.write_content(guard, &content).await
    }

    async fn patch_held(
        &self,
        guard: &AdmissionGuard,
        delta: ConfigDelta,
    ) -> Result<Config, ConfigError> {
        let (updated, warnings) = self.patch_held_with_warnings(guard, delta).await?;
        report_config_load_warnings_once(&self.path, &warnings);
        Ok(updated)
    }
}

/// An exclusive advisory lock on a realm config root's composition lock
/// file ([`RealmConfigSource::composition_lock_path`]), released when
/// dropped. The lock is taken on the blocking pool and waits for the
/// writer that holds it.
struct CompositionLock {
    #[cfg(not(target_arch = "wasm32"))]
    file: std::fs::File,
}

impl CompositionLock {
    #[cfg(not(target_arch = "wasm32"))]
    async fn acquire(path: PathBuf) -> Result<Self, ConfigError> {
        tokio::task::spawn_blocking(move || -> Result<Self, ConfigError> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)?;
            fs4::fs_std::FileExt::lock_exclusive(&file)?;
            Ok(Self { file })
        })
        .await
        .map_err(|error| {
            ConfigError::InternalError(format!(
                "realm config composition lock task failed: {error}"
            ))
        })?
    }

    /// wasm32 has no advisory file lock, and no source there names a lock
    /// file.
    #[cfg(target_arch = "wasm32")]
    async fn acquire(_path: PathBuf) -> Result<Self, ConfigError> {
        Ok(Self {})
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for CompositionLock {
    fn drop(&mut self) {
        // An explicit unlock, so a copy of the descriptor a concurrently
        // spawned process holds until its exec cannot keep the lock.
        let _ = fs4::fs_std::FileExt::unlock(&self.file);
    }
}

/// Refuse a write that would not keep `committed`, the document's governance
/// section, exactly ([`ConfigError::GovernanceAdministration`]): ordinary
/// config write permission is not governance administration. The section is
/// administered by the realm's trusted host in the document itself, so a
/// write can neither change nor remove it (an explicitly empty section
/// included), and neither state one in a document that states none, which
/// would shadow every later change of the parent's.
///
/// The check runs inside each write; it does not exclude a concurrent edit
/// of the document by another writer outside the config owner (such as the
/// trusted host editing the file by hand at the same moment).
fn keep_governance(
    committed: &Option<crate::GovernanceConfig>,
    written: &Option<crate::GovernanceConfig>,
) -> Result<(), ConfigError> {
    if committed == written {
        Ok(())
    } else {
        Err(ConfigError::GovernanceAdministration)
    }
}

/// The governance section a persisted document states (`None` when it states
/// none), for a full replace that does not otherwise parse the old
/// document's sections. The section is found by parsed key identity, so a
/// quoted or escaped key names the same section. A document that is not
/// TOML could state one, so no write may replace it.
fn persisted_governance(content: &str) -> Result<Option<crate::GovernanceConfig>, ConfigError> {
    let document: toml::Table = content
        .parse()
        .map_err(|_| ConfigError::GovernanceAdministration)?;
    match document.get("governance") {
        None => Ok(None),
        Some(section) => section
            .clone()
            .try_into()
            .map(Some)
            .map_err(|_| ConfigError::GovernanceAdministration),
    }
}

/// Refuse a write that would not keep `committed`, the document's
/// `[confinement]` section, exactly ([`ConfigError::ConfinementAdministration`]):
/// ordinary config write permission is not confinement administration. The
/// host administers the section in the document itself, so a write can
/// neither add, change nor remove it; it may restate it unchanged.
fn keep_confinement(
    committed: &crate::ConfinementConfig,
    written: &crate::ConfinementConfig,
) -> Result<(), ConfigError> {
    if committed == written {
        Ok(())
    } else {
        Err(ConfigError::ConfinementAdministration)
    }
}

/// The `[confinement]` section a persisted document states (empty when it
/// states none), for a full replace that does not otherwise parse the old
/// document's sections. A document that is not TOML could state one, so no
/// write may replace it.
fn persisted_confinement(content: &str) -> Result<crate::ConfinementConfig, ConfigError> {
    let document: toml::Table = content
        .parse()
        .map_err(|_| ConfigError::ConfinementAdministration)?;
    match document.get("confinement") {
        None => Ok(crate::ConfinementConfig::default()),
        Some(section) => section
            .clone()
            .try_into()
            .map_err(|_| ConfigError::ConfinementAdministration),
    }
}

/// The credential commands a document states, by realm section and auth
/// profile.
type CredentialCommands = std::collections::BTreeMap<(String, String), CredentialCommand>;

/// A credential command as its document states it: what it runs (its
/// program, arguments, environment and working directory; its timeout and
/// caching are ordinary config), the provider and auth method of its auth
/// profile, and where its token goes: each binding of the same realm section
/// that names the profile as its auth profile, by id
/// ([`CredentialCommandRoute`]). A binding resolves its backend and auth
/// profile within its own section, and a composed section is one document's
/// section whole, so these are all of the document's routes for the token.
#[derive(PartialEq)]
struct CredentialCommand {
    runs: (
        PathBuf,
        Vec<String>,
        std::collections::BTreeMap<String, String>,
        Option<PathBuf>,
    ),
    provider: String,
    auth_method: String,
    routes: std::collections::BTreeMap<String, CredentialCommandRoute>,
}

/// Where a binding sends a credential command's token: the id of its
/// backend profile, that backend as the section states it (`None` when it
/// states none by that id), the `[self_hosted.servers]` entry the backend
/// names as the document states it (`None` when it names none, or the
/// document states none by that id; a composition takes the entry only from
/// the command's document, and refuses a self-hosted backend that names
/// none, see [`crate::config::check_composed_command_servers`]), and the
/// binding's default model, which decides the model, and so the server, of
/// a request through the binding that names none.
#[derive(PartialEq)]
struct CredentialCommandRoute {
    backend_profile: String,
    backend: Option<crate::connection::BackendProfileConfig>,
    server: Option<crate::config::SelfHostedServerConfig>,
    default_model: Option<String>,
}

impl CredentialCommand {
    /// Whether `written` keeps this command: it runs the same thing for the
    /// same provider and auth method, and each binding that uses it is one
    /// that already did, sending the token where it did. Removing a binding
    /// is ordinary config.
    fn kept_by(&self, written: &Self) -> bool {
        self.runs == written.runs
            && self.provider == written.provider
            && self.auth_method == written.auth_method
            && written
                .routes
                .iter()
                .all(|(binding, route)| self.routes.get(binding) == Some(route))
    }
}

/// The credential commands `config` states.
fn credential_commands(config: &Config) -> CredentialCommands {
    config
        .realm
        .iter()
        .flat_map(|(realm, section)| {
            section.auth.iter().filter_map(move |(profile, auth)| {
                let crate::connection::CredentialSourceSpec::Command {
                    program,
                    args,
                    cwd,
                    env,
                    ..
                } = &auth.source
                else {
                    return None;
                };
                let routes = section
                    .binding
                    .iter()
                    .filter(|(_, binding)| &binding.auth_profile == profile)
                    .map(|(id, binding)| {
                        let backend = section.backend.get(&binding.backend_profile).cloned();
                        let server = backend
                            .as_ref()
                            .and_then(|backend| backend.server.as_ref())
                            .and_then(|server| config.self_hosted.servers.get(server))
                            .cloned();
                        (
                            id.clone(),
                            CredentialCommandRoute {
                                backend_profile: binding.backend_profile.clone(),
                                backend,
                                server,
                                default_model: binding.default_model.clone(),
                            },
                        )
                    })
                    .collect();
                Some((
                    (realm.clone(), profile.clone()),
                    CredentialCommand {
                        runs: (program.clone(), args.clone(), env.clone(), cwd.clone()),
                        provider: auth.provider.clone(),
                        auth_method: auth.auth_method.clone(),
                        routes,
                    },
                ))
            })
        })
        .collect()
}

/// The credential commands a persisted document states, for a full replace
/// that does not otherwise parse the old document. A document that does not
/// load states none here, so no write adds one over it.
fn persisted_credential_commands(content: &str) -> CredentialCommands {
    PersistedConfigDocument::parse(content)
        .map(|document| credential_commands(&document.into_loaded().0))
        .unwrap_or_default()
}

/// Refuse a write that adds a credential command, changes what one runs, or
/// changes where its token goes ([`ConfigError::CredentialCommandAdministration`],
/// naming the auth profile): the host runs it as itself, outside its OS
/// process confinement, so ordinary config write permission is not its
/// administration. Where the token goes is the profile's provider and auth
/// method and each binding of the section that uses the profile: such a
/// binding added, re-pointed at the profile, moved to another backend or
/// given another default model, and a change to the backend it names or to
/// the `[self_hosted.servers]` entry that backend names, are all refused. The host adds or changes any
/// of them by editing the document itself. A write that keeps each command
/// and its routes, changes only a command's timeout or caching, or removes a
/// command or a binding is ordinary config.
fn keep_credential_commands(
    committed: &CredentialCommands,
    written: &Config,
) -> Result<(), ConfigError> {
    match credential_commands(written)
        .into_iter()
        .find(|(key, command)| committed.get(key).is_none_or(|kept| !kept.kept_by(command)))
    {
        Some(((realm, profile), _)) => {
            Err(ConfigError::CredentialCommandAdministration { realm, profile })
        }
        None => Ok(()),
    }
}

/// The realm sections a document states, by realm, with the parent edge
/// each names: what the document contributes to the chains it composes.
type RealmEdges = std::collections::BTreeMap<String, Option<crate::connection::RealmId>>;

/// A document stating only `realms` as its realm sections (each section's
/// presence and parent edge) and `confinement` and `governance` as its own
/// sections: all of a document the composition of those sections reads.
fn administration_document(
    realms: &RealmEdges,
    confinement: &crate::ConfinementConfig,
    governance: &Option<crate::GovernanceConfig>,
) -> Config {
    Config {
        confinement: confinement.clone(),
        governance: governance.clone(),
        realm: realms
            .iter()
            .map(|(realm, parent)| {
                (
                    realm.clone(),
                    crate::connection::RealmConfigSection {
                        parent: parent.clone(),
                        ..Default::default()
                    },
                )
            })
            .collect(),
        ..Config::default()
    }
}

/// The realm sections a write changes, as an operator reads them: for
/// example `removing [realm.global]` or `changing the parent of
/// [realm.team]`.
fn realm_edge_change(persisted: &RealmEdges, written: &RealmEdges) -> String {
    let realms: std::collections::BTreeSet<&String> =
        persisted.keys().chain(written.keys()).collect();
    let changes: Vec<String> = realms
        .into_iter()
        .filter_map(|realm| {
            let realm_name = realm.escape_debug();
            match (persisted.get(realm), written.get(realm)) {
                (Some(_), None) => Some(format!("removing [realm.{realm_name}]")),
                (None, Some(_)) => Some(format!("adding [realm.{realm_name}]")),
                (Some(before), Some(after)) if before != after => {
                    Some(format!("changing the parent of [realm.{realm_name}]"))
                }
                _ => None,
            }
        })
        .collect();
    changes.join(", ")
}

/// What a realm composes of the administered sections: its `[confinement]`
/// and its `[governance]`.
type Administered = (crate::ConfinementConfig, Option<crate::GovernanceConfig>);

/// Refuse a write `change` after which `composer` composes a different
/// `[confinement]` or `[governance]` than `before`
/// ([`ConfigError::CompositionAdministration`]).
fn keep_composition(
    change: &str,
    composer: &str,
    before: &Administered,
    after: &Administered,
) -> Result<(), ConfigError> {
    let administered = if !composes_the_same(&before.0, &after.0) {
        crate::config::AdministeredSection::Confinement
    } else if before.1 != after.1 {
        crate::config::AdministeredSection::Governance
    } else {
        return Ok(());
    };
    Err(ConfigError::CompositionAdministration {
        administered,
        change: change.to_owned(),
        composer: composer.to_owned(),
    })
}

/// `documents` with the user document (the `global` realm's own) replaced
/// by `user`: the documents a realm composes from after a write to the user
/// document. With `without`, that realm's own document is left out too: the
/// documents it composes from on a state root that holds no document of its
/// own for it, where it takes the section the user document states for it.
struct WithUserDocument {
    documents: Arc<dyn RealmConfigSource>,
    user: Config,
    without: Option<crate::connection::RealmId>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RealmConfigSource for WithUserDocument {
    async fn config_for_realm(
        &self,
        realm: &crate::connection::RealmId,
    ) -> Result<Option<Config>, ConfigError> {
        if realm.is_global() {
            return Ok(Some(self.user.clone()));
        }
        if self.without.as_ref() == Some(realm) {
            return Ok(None);
        }
        self.documents.config_for_realm(realm).await
    }

    async fn config_for_realm_with_warnings(
        &self,
        realm: &crate::connection::RealmId,
    ) -> Result<Option<(Config, Vec<ConfigWarning>)>, ConfigError> {
        if realm.is_global() {
            return Ok(Some((self.user.clone(), Vec::new())));
        }
        if self.without.as_ref() == Some(realm) {
            return Ok(None);
        }
        self.documents.config_for_realm_with_warnings(realm).await
    }

    async fn raw_config_for_realm(
        &self,
        realm: &crate::connection::RealmId,
    ) -> Result<Option<toml::Value>, ConfigError> {
        if realm.is_global() || self.without.as_ref() == Some(realm) {
            return Ok(None);
        }
        self.documents.raw_config_for_realm(realm).await
    }

    fn composition_lock_path(&self) -> Option<PathBuf> {
        self.documents.composition_lock_path()
    }
}

/// The realm sections `config` states, with their parent edges.
fn realm_edges(config: &Config) -> RealmEdges {
    config
        .realm
        .iter()
        .map(|(realm, section)| (realm.clone(), section.parent.clone()))
        .collect()
}

/// The realm sections a persisted document states, with their parent edges,
/// for a full replace that does not otherwise parse the old document's
/// sections. A document whose sections cannot be read could state any, so
/// no write may change them.
fn persisted_realm_edges(content: &str) -> Result<RealmEdges, ConfigError> {
    let document: toml::Table = content
        .parse()
        .map_err(|_| ConfigError::ConfinementAdministration)?;
    let Some(realms) = document.get("realm") else {
        return Ok(RealmEdges::new());
    };
    realms
        .as_table()
        .ok_or(ConfigError::ConfinementAdministration)?
        .iter()
        .map(|(realm, section)| {
            let section = section
                .as_table()
                .ok_or(ConfigError::ConfinementAdministration)?;
            let parent = match section.get("parent") {
                None => None,
                Some(parent) => Some(
                    parent
                        .as_str()
                        .and_then(|parent| crate::connection::RealmId::parse(parent).ok())
                        .ok_or(ConfigError::ConfinementAdministration)?,
                ),
            };
            Ok((realm.clone(), parent))
        })
        .collect()
}

/// The realm chain a head realm's own document is composed into
/// ([`FileConfigStore::with_realm_chain`]).
#[derive(Clone)]
struct RealmChainGuard {
    head: crate::connection::RealmId,
    documents: Arc<dyn RealmConfigSource>,
}

impl RealmChainGuard {
    /// Refuse a write whose realm sections would change the `[confinement]`
    /// or the `[governance]` a realm composes
    /// ([`ConfigError::CompositionAdministration`], naming the change and
    /// the realm): ordinary config write permission is not their
    /// administration, and a realm composes its ancestors' sections through
    /// its own parent edge and the `global` root a `[realm.global]` section
    /// puts on its chain. A write that keeps the document's realm sections
    /// never composes differently (its own `confinement` and `governance`
    /// are already kept exactly), so only a changed one is composed: from
    /// the persisted sections and from the written ones, over the documents
    /// the host composes at start. The head realm is composed, and for the
    /// user document the other realms it decides for
    /// ([`Self::keep_compositions_over_user_document`]). A persisted chain
    /// that does not compose refuses the write with its error; written
    /// sections whose chain does not compose are invalid.
    async fn keep_composed_administration(
        &self,
        persisted: RealmEdges,
        written: &Config,
        confinement: &crate::ConfinementConfig,
        governance: &Option<crate::GovernanceConfig>,
    ) -> Result<(), ConfigError> {
        let written = realm_edges(written);
        if persisted == written {
            return Ok(());
        }
        let change = realm_edge_change(&persisted, &written);
        let reader = EffectiveConfigReader::new(Arc::clone(&self.documents));
        let before = reader
            .administered_with_realms(&self.head, &persisted, confinement, governance)
            .await?;
        let after = reader
            .administered_with_realms(&self.head, &written, confinement, governance)
            .await
            .map_err(|error| {
                ConfigError::Validation(format!(
                    "the realm's parent edge does not compose: {error}"
                ))
            })?;
        keep_composition(
            &change,
            &format!("realm '{}'", self.head.as_str()),
            &before,
            &after,
        )?;
        if self.head.is_global() {
            self.keep_compositions_over_user_document(
                &change,
                &persisted,
                &written,
                confinement,
                governance,
            )
            .await?;
        }
        Ok(())
    }

    /// For a write to the user document (the `global` realm's own), refuse
    /// one after which another realm would compose a different
    /// `[confinement]` or `[governance]`. Any document's `[realm.global]`
    /// puts `global` on a chain, so the user document's decides it for each
    /// realm whose own documents state none, and a realm without a document
    /// of its own takes the section the user document states for it.
    /// Composed over the persisted and over the written user document: a
    /// realm with no document (for every realm whose own document states no
    /// parent and no `[realm.global]`), and each realm the user document
    /// states a section for, from its own document as the host has it and
    /// as if it had none. Several state roots can share one user document,
    /// and one that holds no document of its own for a stated realm composes
    /// that realm from the user document's section; a stated realm whose
    /// chain resolves on no such state root, before or after the write,
    /// composes nothing there either way.
    async fn keep_compositions_over_user_document(
        &self,
        change: &str,
        persisted: &RealmEdges,
        written: &RealmEdges,
        confinement: &crate::ConfinementConfig,
        governance: &Option<crate::GovernanceConfig>,
    ) -> Result<(), ConfigError> {
        use crate::connection::RealmId;
        let probe = RealmId::parse(format!(
            "composition-probe.{}",
            crate::time_compat::new_uuid_v7()
        ))
        .map_err(|error| ConfigError::InternalError(format!("composition probe realm: {error}")))?;
        let mut composers = vec![(
            probe,
            "a realm whose own document states no parent and no [realm.global]".to_owned(),
            false,
        )];
        let stated: std::collections::BTreeSet<&String> =
            persisted.keys().chain(written.keys()).collect();
        for realm in stated {
            // A section that names no valid realm configures none.
            let Ok(realm) = RealmId::parse(realm.as_str()) else {
                continue;
            };
            if !realm.is_global() {
                let composer = format!("realm '{}'", realm.as_str());
                let without_document = format!(
                    "realm '{}' (on a state root without its own document)",
                    realm.as_str()
                );
                composers.push((realm.clone(), composer, false));
                composers.push((realm, without_document, true));
            }
        }
        let over = |realms: &RealmEdges, without: Option<&RealmId>| {
            EffectiveConfigReader::new(Arc::new(WithUserDocument {
                documents: Arc::clone(&self.documents),
                user: administration_document(realms, confinement, governance),
                without: without.cloned(),
            }))
        };
        for (realm, composer, without_document) in composers {
            let without = without_document.then_some(&realm);
            let before = over(persisted, without).administered(&realm).await;
            let after = over(written, without).administered(&realm).await;
            match (before, after) {
                (Ok(before), Ok(after)) => keep_composition(change, &composer, &before, &after)?,
                // A stated realm whose chain resolves on no state root
                // without its own document, before or after, composes
                // nothing there: chain discovery follows no user-document
                // edge for a realm without its own document.
                (Err(ConfigError::RealmChain(_)), Err(ConfigError::RealmChain(_)))
                    if without_document => {}
                // The user document states a section for the realm that
                // does not compose where the realm has no document, and the
                // write would make it compose: what it would compose there
                // cannot be checked against what it composed before.
                (Err(error), _) if without_document => {
                    return Err(ConfigError::Validation(format!(
                        "the realm sections do not compose for {composer} before this write ({error}), so {change} cannot be checked against what it composes there; the host edits the user document instead"
                    )));
                }
                (Err(error), _) => return Err(error),
                (Ok(_), Err(error)) => {
                    return Err(ConfigError::Validation(format!(
                        "the realm sections do not compose for {composer}: {error}"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Whether two composed `[confinement]` sections state the same thing: the
/// same statement for every launch kind, and the same `all`, which also
/// covers launch kinds a later release adds. A fold fills each kind's own
/// statement once a less trusted document follows the one that decided it,
/// so the same requirement composed over a different number of documents is
/// not field-for-field equal.
fn composes_the_same(a: &crate::ConfinementConfig, b: &crate::ConfinementConfig) -> bool {
    a.all == b.all
        && crate::ConfinementLaunchKind::ALL
            .into_iter()
            .all(|kind| a.statement(kind) == b.statement(kind))
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

/// Canonical RFC 7396 JSON merge-patch application.
///
/// This is the single owner of config patch acceptance/rejection semantics:
/// a `null` patch value removes the key, an object recurses, and any other
/// value replaces. All surfaces (RPC, REST, MCP) MUST route through this and
/// [`apply_config_patch_preview`] (or a [`ConfigStore`]) rather than
/// re-deriving the merge rules. [`FileConfigStore`] applies the same rules to
/// the persisted document itself, so a patch never rewrites keys it does not
/// name.
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

    fn workspace_confinement() -> crate::ConfinementConfig {
        crate::ConfinementConfig::for_all(crate::LaunchConfinement::Workspace(
            crate::WorkspaceConfinement::default(),
        ))
    }

    fn confinement_document(text: &str) -> Config {
        Config::from_persisted_toml(text).unwrap().0
    }

    async fn compose_head(docs: Vec<(&str, Config)>) -> Result<Config, ConfigError> {
        let docs = docs
            .into_iter()
            .map(|(realm, doc)| (realm.to_string(), doc))
            .collect();
        EffectiveConfigReader::new(Arc::new(MapSource { docs }))
            .effective_config(&crate::connection::RealmId::parse("head").unwrap())
            .await
    }

    /// Across the documents a composition folds, a confinement requirement
    /// only holds or is added. The user document decides it even when no
    /// `[realm.global]` section puts it on the chain; a less trusted realm
    /// that restates it or says nothing composes; one that states it
    /// differently, `trusted_host` included, is refused naming both realms.
    #[tokio::test]
    async fn effective_reader_never_weakens_confinement_across_layers() {
        const WORKSPACE: &str = "[confinement.all]\nmode = \"workspace\"\n";
        const TRUSTED_HOST: &str = "[confinement.all]\nmode = \"trusted_host\"\n";
        const WEAKER_MCP: &str = "[confinement.mcp_stdio]\nmode = \"trusted_host\"\n";

        let required = compose_head(vec![
            ("global", confinement_document(WORKSPACE)),
            ("head", Config::default()),
        ])
        .await
        .unwrap();
        for kind in crate::ConfinementLaunchKind::ALL {
            assert_eq!(
                required.confinement.statement(kind),
                workspace_confinement().statement(kind),
                "the off-chain user document decides {kind}"
            );
        }
        let restated = compose_head(vec![
            ("global", confinement_document(WORKSPACE)),
            ("head", confinement_document(WORKSPACE)),
        ])
        .await
        .unwrap();
        assert_eq!(
            restated
                .confinement
                .statement(crate::ConfinementLaunchKind::Shell),
            workspace_confinement().statement(crate::ConfinementLaunchKind::Shell)
        );
        for weaker in [TRUSTED_HOST, WEAKER_MCP] {
            let refused = compose_head(vec![
                ("global", confinement_document(WORKSPACE)),
                ("head", confinement_document(weaker)),
            ])
            .await
            .expect_err("a less trusted document cannot weaken the requirement");
            assert!(
                matches!(
                    &refused,
                    ConfigError::ConfinementLayerConflict(conflict)
                        if conflict.decided_by == "global" && conflict.conflicting == "head"
                ),
                "{weaker}: {refused:?}"
            );
        }
        // A less trusted document can add confinement the user did not set.
        for user in [None, Some(TRUSTED_HOST)] {
            let mut docs = vec![("head", confinement_document(WORKSPACE))];
            if let Some(user) = user {
                docs.push(("global", confinement_document(user)));
            }
            let tightened = compose_head(docs).await.unwrap();
            assert_eq!(
                tightened
                    .confinement
                    .statement(crate::ConfinementLaunchKind::McpStdio),
                workspace_confinement().statement(crate::ConfinementLaunchKind::McpStdio),
                "{user:?}"
            );
        }
    }

    /// An ancestor realm decides confinement for its descendants: a child
    /// realm that states it differently is refused.
    #[tokio::test]
    async fn effective_reader_refuses_a_child_realm_that_weakens_its_parent() {
        use crate::connection::{RealmConfigSection, RealmId};

        let mut org = confinement_document("[confinement.shell]\nmode = \"workspace\"\n");
        org.realm
            .insert("org".to_string(), RealmConfigSection::default());
        let mut head = confinement_document(
            "[confinement.shell]\nmode = \"workspace\"\nnetwork = \"unrestricted\"\n",
        );
        head.realm.insert(
            "head".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::parse("org").unwrap()),
                ..Default::default()
            },
        );
        let refused = compose_head(vec![("org", org.clone()), ("head", head)])
            .await
            .expect_err("a child cannot replace its parent's requirement");
        assert!(
            matches!(
                &refused,
                ConfigError::ConfinementLayerConflict(conflict)
                    if conflict.kind == crate::ConfinementLaunchKind::Shell
                        && conflict.decided_by == "org"
                        && conflict.conflicting == "head"
            ),
            "{refused:?}"
        );

        // The supplied-head path of the network surfaces refuses it too.
        let mut silent_head = Config::default();
        silent_head.realm.insert(
            "head".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::parse("org").unwrap()),
                ..Default::default()
            },
        );
        let mut weaker = silent_head.clone();
        weaker.confinement =
            crate::ConfinementConfig::for_all(crate::LaunchConfinement::TrustedHost);
        let docs = std::collections::BTreeMap::from([("org".to_string(), org)]);
        let reader = EffectiveConfigReader::new(Arc::new(MapSource { docs }));
        let head_id = RealmId::parse("head").unwrap();
        assert!(matches!(
            reader.effective_config_over_head(&head_id, weaker).await,
            Err(ConfigError::ConfinementLayerConflict(_))
        ));
        let inherited = reader
            .effective_config_over_head(&head_id, silent_head)
            .await
            .unwrap();
        assert!(
            inherited
                .confinement
                .statement(crate::ConfinementLaunchKind::Shell)
                .is_some_and(crate::LaunchConfinement::requires_confinement)
        );
    }

    /// A realm's own document decides its own parent edge. An ancestor's
    /// document that states a section for the realm (an ordinary config
    /// write to the ancestor's own document can add one) never cuts the
    /// realm from that ancestor: the realm still composes the ancestor's
    /// `[confinement]`, whichever document sorts last.
    #[tokio::test]
    async fn another_documents_section_never_cuts_a_realm_from_its_ancestors() {
        use crate::connection::{RealmConfigSection, RealmId};

        // `org` requires confinement and states an unparented section for
        // `alpha`, which names `org` as its parent in its own document.
        let mut org = confinement_document("[confinement.all]\nmode = \"workspace\"\n");
        org.realm
            .insert("org".to_string(), RealmConfigSection::default());
        org.realm
            .insert("alpha".to_string(), RealmConfigSection::default());
        let mut alpha = Config::default();
        alpha.realm.insert(
            "alpha".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::parse("org").unwrap()),
                ..Default::default()
            },
        );
        let docs = std::collections::BTreeMap::from([
            ("org".to_string(), org),
            ("alpha".to_string(), alpha.clone()),
        ]);
        let reader = EffectiveConfigReader::new(Arc::new(MapSource { docs }));
        let alpha_id = RealmId::parse("alpha").unwrap();
        let fetched = reader.effective_config(&alpha_id).await.unwrap();
        let supplied = reader
            .effective_config_over_head(&alpha_id, alpha)
            .await
            .unwrap();
        for composed in [fetched, supplied] {
            for kind in crate::ConfinementLaunchKind::ALL {
                assert_eq!(
                    composed.confinement.statement(kind),
                    workspace_confinement().statement(kind),
                    "{kind}"
                );
            }
        }
    }

    /// `[confinement]` is administered by the host in the document itself:
    /// an ordinary write can neither add, change nor remove it, and a write
    /// that keeps it exactly commits.
    #[tokio::test]
    async fn config_writes_cannot_administer_confinement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let refused = |result: Result<(), ConfigError>| {
            assert!(
                matches!(result, Err(ConfigError::ConfinementAdministration)),
                "{result:?}"
            );
        };

        refused(
            store
                .patch(ConfigDelta(
                    serde_json::json!({"confinement": {"all": {"mode": "workspace"}}}),
                ))
                .await
                .map(|_| ()),
        );
        let added = Config {
            confinement: workspace_confinement(),
            ..Config::default()
        };
        refused(store.set(added).await);

        let stated = "[confinement.all]\nmode = \"workspace\"\n";
        tokio::fs::write(&path, stated).await.unwrap();
        for patch in [
            serde_json::json!({"confinement": null}),
            serde_json::json!({"confinement": {"all": {"mode": "trusted_host"}}}),
            serde_json::json!({"confinement": {"shell": {"mode": "trusted_host"}}}),
        ] {
            refused(store.patch(ConfigDelta(patch)).await.map(|_| ()));
        }
        refused(store.set(Config::default()).await);
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), stated);

        let patched = store
            .patch(ConfigDelta(serde_json::json!({"max_tokens": 2048})))
            .await
            .unwrap();
        assert_eq!(patched.confinement, workspace_confinement());
        let mut restated = patched;
        restated.max_tokens = Some(4096);
        store.set(restated).await.unwrap();
        let reloaded = store.get().await.unwrap();
        assert_eq!(reloaded.confinement, workspace_confinement());
        assert_eq!(reloaded.max_tokens, Some(4096));

        let memory = MemoryConfigStore::new(
            reloaded.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        refused(memory.set(Config::default()).await);
        refused(
            memory
                .patch(ConfigDelta(serde_json::json!({"confinement": null})))
                .await
                .map(|_| ()),
        );
        memory.set(reloaded).await.unwrap();
    }

    /// A store for realm `team`'s own document at `path`, composed into its
    /// realm chain over the other documents `ancestors` supplies.
    fn chained_store(path: &Path, ancestors: Vec<(&str, &str)>) -> FileConfigStore {
        let docs = ancestors
            .into_iter()
            .map(|(realm, text)| (realm.to_string(), confinement_document(text)))
            .collect();
        FileConfigStore::new(
            path.to_path_buf(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .with_realm_chain(
            crate::connection::RealmId::parse("team").unwrap(),
            Arc::new(MapSource { docs }),
        )
    }

    /// An ordinary write cannot cut a realm from the ancestors whose
    /// `[confinement]` it composes: a patch or a full replace that removes or
    /// changes the realm's own parent edge is refused when the realm would
    /// compose a different section, in either direction, and nothing is
    /// written. A changed edge that composes the same section, an edge that
    /// stays, and a document that states the requirement itself commit; an
    /// edge whose chain does not resolve is invalid.
    #[tokio::test]
    async fn a_write_cannot_change_the_confinement_a_realm_composes_through_its_parent() {
        const ORG: &str = "[realm.org]\n\n[confinement.shell]\nmode = \"workspace\"\n";
        const TWIN: &str = "[realm.twin]\n\n[confinement.shell]\nmode = \"workspace\"\n";
        const LOOSE: &str = "[realm.loose]\n";
        const HEAD: &str = "[realm.team]\nparent = \"org\"\n";
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, HEAD).await.unwrap();
        let store = chained_store(&path, vec![("org", ORG), ("twin", TWIN), ("loose", LOOSE)]);
        let refused = |result: Result<(), ConfigError>| {
            assert!(
                matches!(
                    result,
                    Err(ConfigError::CompositionAdministration {
                        administered: crate::config::AdministeredSection::Confinement,
                        ..
                    })
                ),
                "{result:?}"
            );
        };

        for patch in [
            serde_json::json!({"realm": {"team": {"parent": null}}}),
            serde_json::json!({"realm": {"team": null}}),
            serde_json::json!({"realm": null}),
            serde_json::json!({"realm": {"team": {"parent": "loose"}}}),
            serde_json::json!({"realm": {"team": {"parent": "global"}}}),
        ] {
            refused(store.patch(ConfigDelta(patch)).await.map(|_| ()));
        }
        refused(store.set(Config::default()).await);
        let mut loose = store.get().await.unwrap();
        loose.realm.get_mut("team").unwrap().parent =
            Some(crate::connection::RealmId::parse("loose").unwrap());
        refused(store.set(loose).await);
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), HEAD);

        // A missing parent does not compose: the written edge is invalid.
        assert!(matches!(
            store
                .patch(ConfigDelta(
                    serde_json::json!({"realm": {"team": {"parent": "absent"}}})
                ))
                .await,
            Err(ConfigError::Validation(_))
        ));
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), HEAD);

        // The same requirement through another parent, and a write that
        // keeps the edge, commit.
        store
            .patch(ConfigDelta(
                serde_json::json!({"realm": {"team": {"parent": "twin"}}}),
            ))
            .await
            .expect("a parent composing the same section");
        let mut kept = store.get().await.unwrap();
        kept.max_tokens = Some(2048);
        store
            .set(kept)
            .await
            .expect("a replace that keeps the edge");
        let reloaded = store.get().await.unwrap();
        assert_eq!(
            reloaded.realm["team"]
                .parent
                .as_ref()
                .map(|parent| parent.as_str()),
            Some("twin")
        );
        assert_eq!(reloaded.max_tokens, Some(2048));

        // Adding confinement through a parent edge is administration too.
        tokio::fs::write(&path, "[realm.team]\nparent = \"loose\"\n")
            .await
            .unwrap();
        refused(
            store
                .patch(ConfigDelta(
                    serde_json::json!({"realm": {"team": {"parent": "org"}}}),
                ))
                .await
                .map(|_| ()),
        );
        // Between parents that require nothing, the edge is ordinary config.
        store
            .patch(ConfigDelta(
                serde_json::json!({"realm": {"team": {"parent": null}}}),
            ))
            .await
            .expect("no confinement is composed either way");

        // A document that states the requirement itself keeps it without the
        // parent edge.
        let restated =
            "[realm.team]\nparent = \"org\"\n\n[confinement.shell]\nmode = \"workspace\"\n";
        tokio::fs::write(&path, restated).await.unwrap();
        store
            .patch(ConfigDelta(
                serde_json::json!({"realm": {"team": {"parent": null}}}),
            ))
            .await
            .expect("the realm still composes the same requirement");
    }

    /// A persisted chain that does not compose refuses an edge change with
    /// its own error instead of guessing what the realm composed, and a
    /// document whose edge cannot be read refuses a full replace.
    #[tokio::test]
    async fn an_edge_change_from_a_chain_that_does_not_compose_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, "[realm.team]\nparent = \"absent\"\n")
            .await
            .unwrap();
        let store = chained_store(&path, vec![("loose", "[realm.loose]\n")]);
        assert!(matches!(
            store
                .patch(ConfigDelta(
                    serde_json::json!({"realm": {"team": {"parent": "loose"}}}),
                ))
                .await,
            Err(ConfigError::RealmChain(_))
        ));
        tokio::fs::write(&path, "[realm.team]\nparent = 7\n")
            .await
            .unwrap();
        assert!(matches!(
            store.set(Config::default()).await,
            Err(ConfigError::ConfinementAdministration)
        ));
    }

    /// The review rule the governance tests compose.
    const REVIEW_RULE: &str = "[[governance.review.rules]]\n\
                               target = { kind = \"mcp_server\", server = \"mail\" }\n\
                               tier = \"r2\"\n";

    /// A realm composes its ancestors' `[governance]` through its own parent
    /// edge too: a patch or a full replace that removes or changes the edge
    /// is refused as governance administration when the realm would compose
    /// a different section, in either direction, and nothing is written. A
    /// changed edge that composes the same section, and an edge change in a
    /// document that states its own section, commit.
    #[tokio::test]
    async fn a_write_cannot_change_the_governance_a_realm_composes_through_its_parent() {
        const HEAD: &str = "[realm.team]\nparent = \"org\"\n";
        let org = format!("[realm.org]\n\n{REVIEW_RULE}");
        let twin = format!("[realm.twin]\n\n{REVIEW_RULE}");
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, HEAD).await.unwrap();
        let store = chained_store(
            &path,
            vec![
                ("org", org.as_str()),
                ("twin", twin.as_str()),
                ("loose", "[realm.loose]\n"),
            ],
        );
        let refused = |result: Result<(), ConfigError>| {
            assert!(
                matches!(
                    result,
                    Err(ConfigError::CompositionAdministration {
                        administered: crate::config::AdministeredSection::Governance,
                        ..
                    })
                ),
                "{result:?}"
            );
        };

        for patch in [
            serde_json::json!({"realm": {"team": {"parent": null}}}),
            serde_json::json!({"realm": {"team": null}}),
            serde_json::json!({"realm": null}),
            serde_json::json!({"realm": {"team": {"parent": "loose"}}}),
        ] {
            refused(store.patch(ConfigDelta(patch)).await.map(|_| ()));
        }
        refused(store.set(Config::default()).await);
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), HEAD);

        store
            .patch(ConfigDelta(
                serde_json::json!({"realm": {"team": {"parent": "twin"}}}),
            ))
            .await
            .expect("a parent composing the same section");

        // Adding governance through a parent edge is administration too.
        tokio::fs::write(&path, "[realm.team]\nparent = \"loose\"\n")
            .await
            .unwrap();
        refused(
            store
                .patch(ConfigDelta(
                    serde_json::json!({"realm": {"team": {"parent": "org"}}}),
                ))
                .await
                .map(|_| ()),
        );

        // A document that states its own section composes it whatever its
        // parent.
        let own = format!("[realm.team]\nparent = \"org\"\n\n{REVIEW_RULE}");
        tokio::fs::write(&path, &own).await.unwrap();
        store
            .patch(ConfigDelta(
                serde_json::json!({"realm": {"team": {"parent": null}}}),
            ))
            .await
            .expect("the realm states its own governance");
    }

    /// The user document's `[governance]` reaches a realm while the `global`
    /// realm is on its chain, which a `[realm.global]` section in any
    /// composed document puts it. A write that removes that section from
    /// the realm's own document, the only one stating it, would drop the
    /// governance without touching the realm's parent edge: it is refused,
    /// and nothing is written.
    #[tokio::test]
    async fn a_write_cannot_take_the_global_realm_off_a_chain_that_composes_its_governance() {
        const HEAD: &str = "[realm.team]\n\n[realm.global]\n";
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, HEAD).await.unwrap();
        let store = chained_store(&path, vec![("global", REVIEW_RULE)]);
        for patch in [
            serde_json::json!({"realm": {"global": null}}),
            serde_json::json!({"realm": null}),
        ] {
            let result = store.patch(ConfigDelta(patch)).await.map(|_| ());
            assert!(
                matches!(
                    result,
                    Err(ConfigError::CompositionAdministration {
                        administered: crate::config::AdministeredSection::Governance,
                        ..
                    })
                ),
                "{result:?}"
            );
        }
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), HEAD);
    }

    /// A user document that states `[governance]` but no `[realm.global]`
    /// governs no other realm; a write adding `[realm.global]` (as a sign-in
    /// that provisions credentials under it does) would put that governance
    /// on every realm's chain, so it is refused, and the refusal says to add
    /// the section by hand. The bootstrap template states the section, so a
    /// user document created from it never meets this.
    #[tokio::test]
    async fn adding_the_global_realm_section_is_refused_with_its_remedy() {
        let user = REVIEW_RULE.to_string();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, &user).await.unwrap();
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .with_realm_chain(
            crate::connection::RealmId::global(),
            Arc::new(MapSource {
                docs: std::collections::BTreeMap::new(),
            }),
        );
        let refused = store
            .patch(ConfigDelta(serde_json::json!({"realm": {"global": {}}})))
            .await
            .expect_err("adding [realm.global] would spread the user's governance");
        assert!(
            refused
                .to_string()
                .contains("add [realm.global] to the user document by hand"),
            "{refused}"
        );
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), user);
        assert!(
            Config::template()
                .unwrap()
                .realm
                .contains_key(crate::connection::RealmId::global().as_str()),
            "the bootstrap template states [realm.global]"
        );
    }

    /// The user document's `[realm.global]` section puts the `global` realm,
    /// and with it the user document's `[governance]`, on the chain of every
    /// realm that states no parent of its own. A write to the user document
    /// (through the global realm's own store) that removes it would drop
    /// that governance from all of them: it is refused, naming the section,
    /// and nothing is written. An unrelated edit commits.
    #[tokio::test]
    async fn a_user_document_write_cannot_take_global_off_the_chains_it_governs() {
        let user = format!("[realm.global]\n\n{REVIEW_RULE}");
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, &user).await.unwrap();
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .with_realm_chain(
            crate::connection::RealmId::global(),
            Arc::new(MapSource {
                docs: std::collections::BTreeMap::new(),
            }),
        );
        for patch in [
            serde_json::json!({"realm": {"global": null}}),
            serde_json::json!({"realm": null}),
        ] {
            let refused = store
                .patch(ConfigDelta(patch.clone()))
                .await
                .expect_err("the write would drop the governance other realms compose");
            assert!(
                refused.to_string().contains("[realm.global]"),
                "{patch}: {refused}"
            );
        }
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), user);
        store
            .patch(ConfigDelta(serde_json::json!({"max_tokens": 2048})))
            .await
            .expect("an unrelated edit of the user document commits");
    }

    /// Several state roots can share one user document. On a host whose
    /// state root holds no document of its own for a realm the user
    /// document states a section for, that realm takes the user document's
    /// section, so a write to the user document is refused when such a
    /// realm would compose differently, even on a host whose state root
    /// holds the realm's own document, and nothing is written. A write
    /// after which such a realm composes the same commits, and so does one
    /// beside a stated realm that composes on no such host before or after
    /// the write.
    #[tokio::test]
    async fn a_user_document_write_keeps_what_a_stated_realm_without_its_own_document_composes() {
        use crate::connection::RealmId;

        // On this host `team` has its own document, which states
        // `[realm.global]`: it composes the user document's governance
        // whatever the user document says about `team`. On a host whose
        // state root holds no document for `team`, `team` takes the user
        // document's `[realm.team]`, whose edge alone puts `global`, and
        // with it the governance, on its chain.
        let team_document = confinement_document("[realm.team]\n\n[realm.global]\n");
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .with_realm_chain(
            RealmId::global(),
            Arc::new(MapSource {
                docs: std::iter::once(("team".to_string(), team_document)).collect(),
            }),
        );

        let user = format!("[realm.team]\nparent = \"global\"\n\n{REVIEW_RULE}");
        tokio::fs::write(&path, &user).await.unwrap();
        let refused = store
            .patch(ConfigDelta(serde_json::json!({"realm": {"team": null}})))
            .await
            .expect_err("team would lose the governance where it has no document of its own");
        assert!(
            matches!(
                refused,
                ConfigError::CompositionAdministration {
                    administered: crate::config::AdministeredSection::Governance,
                    ..
                }
            ) && refused.to_string().contains("realm 'team'"),
            "{refused}"
        );
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), user);

        // With `[realm.global]` in the user document, `team` composes
        // `global` with or without its section there.
        let user = format!("[realm.global]\n\n[realm.team]\nparent = \"global\"\n\n{REVIEW_RULE}");
        tokio::fs::write(&path, &user).await.unwrap();
        store
            .patch(ConfigDelta(serde_json::json!({"realm": {"team": null}})))
            .await
            .expect("team composes the same governance without its section");

        // A stated realm whose edge resolves on no host where it has no
        // document of its own composes nothing there, before or after, so it
        // does not refuse a write that changes other realm sections.
        let user = format!("[realm.global]\n\n[realm.team]\nparent = \"org\"\n\n{REVIEW_RULE}");
        tokio::fs::write(&path, &user).await.unwrap();
        store
            .patch(ConfigDelta(serde_json::json!({"realm": {"other": {}}})))
            .await
            .expect("no realm composes differently");
    }

    /// A user document section for a realm that does not compose where the
    /// realm has no document of its own (its edge names a realm with no
    /// document) cannot be checked against what a write would make it
    /// compose there: removing it is refused, and the refusal names the
    /// composition it could not check (the realm on a state root without
    /// its own document, not this host's) with the chain error. Nothing is
    /// written; the host edits the user document instead.
    #[tokio::test]
    async fn removing_a_user_section_that_composes_nowhere_names_the_state_root_it_is_about() {
        use crate::connection::RealmId;

        let team_document = confinement_document("[realm.team]\n\n[realm.global]\n");
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .with_realm_chain(
            RealmId::global(),
            Arc::new(MapSource {
                docs: std::iter::once(("team".to_string(), team_document)).collect(),
            }),
        );

        let user = format!("[realm.team]\nparent = \"org\"\n\n{REVIEW_RULE}");
        tokio::fs::write(&path, &user).await.unwrap();
        let refused = store
            .patch(ConfigDelta(serde_json::json!({"realm": {"team": null}})))
            .await
            .expect_err("what team would compose without its document cannot be checked");
        let message = refused.to_string();
        assert!(
            matches!(refused, ConfigError::Validation(_))
                && message.contains("removing [realm.team]")
                && message.contains("realm 'team' (on a state root without its own document)")
                && message.contains("org"),
            "{message}"
        );
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), user);
    }

    /// The view a user document write checks a stated realm in (as if the
    /// realm had no document of its own) is what a host on another state
    /// root that shares the user document, and holds no document for the
    /// realm, composes at run time. Chain discovery follows no user document
    /// edge for a realm without its own document, so an edge to a realm
    /// that has a document on that root does not compose there either: the
    /// guard may skip a stated realm whose chain resolves neither before nor
    /// after a write.
    #[tokio::test]
    async fn the_as_if_view_is_what_another_state_root_composes() {
        use crate::connection::RealmId;

        /// A state root's documents, as the filesystem source maps them:
        /// `global` to the user document, every other realm to
        /// `<root>/<realm>/config.toml`.
        struct StateRoot {
            root: PathBuf,
            user: PathBuf,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl RealmConfigSource for StateRoot {
            async fn config_for_realm(
                &self,
                realm: &crate::connection::RealmId,
            ) -> Result<Option<Config>, ConfigError> {
                let path = if realm.is_global() {
                    self.user.clone()
                } else {
                    self.root.join(realm.as_str()).join("config.toml")
                };
                if !tokio::fs::try_exists(&path).await? {
                    return Ok(None);
                }
                FileConfigStore::new(path, *crate::model_profile::test_catalog::TEST_CATALOG)
                    .get()
                    .await
                    .map(Some)
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let user = temp.path().join("user.toml");
        // Root A, where a user document write is checked, holds `team`'s
        // document; root B holds none for `team`, and one for `org` that
        // states governance.
        let write = |relative: &str, text: String| {
            let path = temp.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "a/team/config.toml",
            "[realm.team]\n\n[realm.global]\n".to_owned(),
        );
        write("b/org/config.toml", format!("[realm.org]\n\n{REVIEW_RULE}"));
        let root_a = Arc::new(StateRoot {
            root: temp.path().join("a"),
            user: user.clone(),
        });
        let root_b = Arc::new(StateRoot {
            root: temp.path().join("b"),
            user: user.clone(),
        });
        let team = RealmId::parse("team").unwrap();

        for (stated, composes) in [
            (
                format!("[realm.team]\nparent = \"global\"\n\n{REVIEW_RULE}"),
                true,
            ),
            ("[realm.team]\nparent = \"org\"\n".to_owned(), false),
        ] {
            std::fs::write(&user, &stated).unwrap();
            let run_time =
                EffectiveConfigReader::new(Arc::clone(&root_b) as Arc<dyn RealmConfigSource>)
                    .administered(&team)
                    .await;
            let user_config = FileConfigStore::new(
                user.clone(),
                *crate::model_profile::test_catalog::TEST_CATALOG,
            )
            .get()
            .await
            .unwrap();
            let as_if = EffectiveConfigReader::new(Arc::new(WithUserDocument {
                documents: Arc::clone(&root_a) as Arc<dyn RealmConfigSource>,
                user: administration_document(
                    &realm_edges(&user_config),
                    &user_config.confinement,
                    &user_config.governance,
                ),
                without: Some(team.clone()),
            }))
            .administered(&team)
            .await;
            match (&run_time, &as_if) {
                (Ok(run_time), Ok(as_if)) => assert!(
                    composes
                        && run_time.1 == as_if.1
                        && run_time.1.is_some()
                        && composes_the_same(&run_time.0, &as_if.0),
                    "{stated}"
                ),
                (Err(ConfigError::RealmChain(_)), Err(ConfigError::RealmChain(_))) => {
                    assert!(!composes, "{stated}");
                }
                other => panic!(
                    "{stated}: root B at run time and root A's as-if view differ: {:?}",
                    (
                        other.0.as_ref().map(|composed| composed.1.is_some()),
                        other.1.as_ref().map(|composed| composed.1.is_some())
                    )
                ),
            }
        }
    }

    /// A store keeps the auto traits it had before it could declare a realm
    /// chain.
    #[test]
    fn the_file_store_stays_unwind_safe() {
        fn unwind_safe<T: std::panic::UnwindSafe + std::panic::RefUnwindSafe>() {}
        unwind_safe::<FileConfigStore>();
    }

    /// A credential command runs as the host, outside its confinement, so it
    /// is administered by the host in the document itself: an ordinary
    /// write can neither add one nor change its program, arguments,
    /// environment or working directory (refused as confinement
    /// administration, naming the auth profile), and nothing is written. A
    /// write that keeps it, changes only its timeout, or removes it commits.
    #[tokio::test]
    async fn a_config_write_cannot_add_or_change_a_credential_command() {
        const STATED: &str = "[realm.global.auth.helper]\n\
                              provider = \"openai\"\n\
                              auth_method = \"api_key\"\n\
                              source = { kind = \"command\", program = \"/usr/bin/helper\", args = [\"--token\"] }\n";
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, "max_tokens = 4096\n")
            .await
            .unwrap();
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let refused = |result: Result<Config, ConfigError>| {
            let refused = result.expect_err("a credential command is host administration");
            assert!(
                refused.administered_section()
                    == Some(crate::config::AdministeredSection::Confinement)
                    && refused.to_string().contains("helper"),
                "{refused}"
            );
        };
        refused(store.patch(ConfigDelta(serde_json::json!({"realm": {"global": {"auth": {"helper": {
                "provider": "openai",
                "auth_method": "api_key",
                "source": {"kind": "command", "program": "/bin/sh", "args": ["-c", "printf sk-x"]}
            }}}}}))).await);
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "max_tokens = 4096\n"
        );

        tokio::fs::write(&path, STATED).await.unwrap();
        for patch in [
            serde_json::json!({"realm": {"global": {"auth": {"helper": {"source": {"args": ["--other"]}}}}}}),
            serde_json::json!({"realm": {"global": {"auth": {"helper": {"source": {"program": "/bin/sh"}}}}}}),
            serde_json::json!({"realm": {"global": {"auth": {"helper": {"source": {"env": {"PATH": "/tmp"}}}}}}}),
            serde_json::json!({"realm": {"global": {"auth": {"helper": {"source": {"cwd": "/tmp"}}}}}}),
        ] {
            refused(store.patch(ConfigDelta(patch)).await);
            assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), STATED);
        }
        store
            .patch(ConfigDelta(serde_json::json!({"realm": {"global": {"auth": {"helper": {"source": {"timeout_ms": 9000}}}}}})))
            .await
            .expect("a timeout is ordinary config");
        store
            .patch(ConfigDelta(
                serde_json::json!({"realm": {"global": {"auth": {"helper": null}}}}),
            ))
            .await
            .expect("removing a credential command is ordinary config");

        let memory = MemoryConfigStore::new(
            Config::default(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        refused(memory.patch(ConfigDelta(serde_json::json!({"realm": {"global": {"auth": {"helper": {
                "provider": "openai",
                "auth_method": "api_key",
                "source": {"kind": "command", "program": "/bin/sh", "args": ["-c", "printf sk-x"]}
            }}}}}))).await);
    }

    /// A chain member other than the head and `global` composes only from
    /// its own document: a section another document states for it neither
    /// stands in for the missing document nor gives it a parent edge. A
    /// realm whose ancestor's own document is gone does not compose, rather
    /// than composing as if that ancestor and the realms above it required
    /// nothing.
    #[tokio::test]
    async fn a_missing_ancestor_document_does_not_compose_as_requiring_nothing() {
        use crate::connection::RealmId;

        // team -> org -> corp, where org requires confinement; org's own
        // document is gone. team's own document and the user document state
        // an empty section for org, which an ordinary write can add.
        let team = confinement_document("[realm.team]\nparent = \"org\"\n\n[realm.org]\n");
        let corp =
            confinement_document("[realm.corp]\n\n[confinement.shell]\nmode = \"workspace\"\n");
        let global = confinement_document("[realm.org]\n");
        let docs = std::collections::BTreeMap::from([
            ("team".to_string(), team.clone()),
            ("corp".to_string(), corp),
            ("global".to_string(), global),
        ]);
        let reader = EffectiveConfigReader::new(Arc::new(MapSource { docs }));
        let team_id = RealmId::parse("team").unwrap();
        for composed in [
            reader.effective_config(&team_id).await,
            reader.effective_config_over_head(&team_id, team).await,
        ] {
            assert!(
                matches!(composed, Err(ConfigError::RealmChain(_))),
                "{composed:?}"
            );
        }
    }

    /// A realm document as 0.9.0 wrote it, with no `[confinement]`, loads,
    /// composes and is written back without one: every launch stays the
    /// trusted host, exactly as before.
    #[tokio::test]
    async fn a_document_written_by_090_is_unchanged_and_unconfined() {
        const DOCUMENT_090: &str = r#"max_tokens = 4096

[agent]
model = "test-anthropic-configured"

[tools]
shell_enabled = true
builtins_enabled = true

[limits]
budget = 200000
max_turn_duration = "30m"

[shell]
security_mode = "allow_list"
security_patterns = ["git *", "cargo test*"]

[[tools.mcp_servers]]
name = "search"
command = "/usr/local/bin/search-mcp"

[realm.ws-090]
parent = "global"
"#;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, DOCUMENT_090).await.unwrap();
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );

        let (loaded, warnings) = store.get_with_warnings().await.unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(loaded.confinement.is_empty());
        let composed = compose_head(vec![("head", loaded.clone())]).await.unwrap();
        for kind in crate::ConfinementLaunchKind::ALL {
            assert_eq!(composed.confinement.statement(kind), None, "{kind}");
        }
        assert!(
            !toml::to_string_pretty(&loaded)
                .unwrap()
                .contains("confinement"),
            "a typed write adds no section"
        );

        store
            .patch(ConfigDelta(serde_json::json!({"max_tokens": 8192})))
            .await
            .unwrap();
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(!written.contains("confinement"), "{written}");
        assert_eq!(
            written.replace("max_tokens = 8192", "max_tokens = 4096"),
            DOCUMENT_090,
            "a patch rewrites only the key it names"
        );
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

    #[tokio::test]
    async fn mcp_server_owner_is_the_nearest_own_document_on_the_composed_chain() {
        use crate::connection::{RealmConfigSection, RealmId};
        use crate::mcp_config::McpServerConfig;

        let server = |name: &str, command: &str| {
            McpServerConfig::stdio(name, command, Vec::new(), std::collections::HashMap::new())
        };
        let mut global = Config::default();
        global
            .realm
            .insert("global".to_string(), RealmConfigSection::default());
        // `twice` is listed twice: the composition makes the later entry
        // effective.
        global.tools.mcp_servers = vec![
            server("search", "global-search"),
            server("shared", "same"),
            server("twice", "shadowed"),
            server("twice", "effective"),
        ];
        let mut child = Config::default();
        child.realm.insert(
            "child".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::global()),
                ..Default::default()
            },
        );
        // The child repeats `shared` exactly; it still defines it itself.
        child.tools.mcp_servers = vec![server("shared", "same")];
        let mut docs = std::collections::BTreeMap::new();
        docs.insert("child".to_string(), child.clone());
        docs.insert("global".to_string(), global);
        let reader = EffectiveConfigReader::new(Arc::new(MapSource { docs }));
        let child_id = RealmId::parse("child").unwrap();

        assert_eq!(
            reader.mcp_server_owner(&child_id, "search").await.unwrap(),
            Some((RealmId::global(), server("search", "global-search"))),
            "an inherited server is owned by the realm whose document defines it"
        );
        assert_eq!(
            reader
                .mcp_server_owner(&child_id, "shared")
                .await
                .unwrap()
                .map(|(owner, _)| owner),
            Some(child_id.clone()),
            "an identical child definition is the child's own, never its parent's"
        );
        assert_eq!(
            reader.mcp_server_owner(&child_id, "missing").await.unwrap(),
            None
        );
        assert_eq!(
            reader.mcp_server_owner(&child_id, "twice").await.unwrap(),
            Some((RealmId::global(), server("twice", "effective"))),
            "a twice-listed name is owned with its later, effective entry"
        );
        // The owner's definition is the one the composition yields.
        let effective = reader.effective_config(&child_id).await.unwrap();
        assert!(
            !effective
                .tools
                .mcp_servers
                .contains(&server("twice", "shadowed"))
        );
        for name in ["search", "shared", "twice"] {
            let (_, defined) = reader
                .mcp_server_owner(&child_id, name)
                .await
                .unwrap()
                .unwrap();
            assert!(effective.tools.mcp_servers.contains(&defined), "{name}");
        }
        // Over a caller-supplied head, the head's own servers come from it.
        let mut head = child;
        head.tools.mcp_servers.clear();
        assert_eq!(
            reader
                .mcp_server_owner_over_head(&child_id, head, "shared")
                .await
                .unwrap()
                .map(|(owner, _)| owner),
            Some(RealmId::global())
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

    /// A binding whose auth profile runs a credential command sends the token
    /// to the `[self_hosted.servers]` entry its backend names. Server entries
    /// compose field by field across the documents on a chain, so the
    /// composition refuses one that another document states or changes: a
    /// realm document cannot point the user document's command at its own
    /// endpoint. The same document may change an entry that only a binding
    /// without a command uses, and a chain whose entry is the command
    /// document's own composes.
    #[tokio::test]
    async fn a_composition_takes_a_command_bindings_server_only_from_its_document() {
        use crate::connection::RealmId;

        const USER: &str = r#"[realm.global]

[self_hosted.servers.box]
base_url = "http://127.0.0.1:9/v1"

[self_hosted.servers.open]
base_url = "http://127.0.0.1:10/v1"

[realm.team.backend.boxed]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "box"

[realm.team.backend.opened]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "open"

[realm.team.auth.helper]
provider = "self_hosted"
auth_method = "api_key"
source = { kind = "command", program = "/usr/bin/helper" }

[realm.team.auth.anonymous]
provider = "self_hosted"
auth_method = "none"
source = { kind = "platform_default" }

[realm.team.binding.main]
backend_profile = "boxed"
auth_profile = "helper"

[realm.team.binding.public]
backend_profile = "opened"
auth_profile = "anonymous"
"#;
        let team = RealmId::parse("team").unwrap();
        let compose = |team_document: &'static str| {
            let source = RawMapSource {
                docs: [("global", USER), ("team", team_document)]
                    .into_iter()
                    .map(|(realm, text)| {
                        (
                            realm.to_owned(),
                            Config::from_persisted_toml(text).unwrap().0,
                        )
                    })
                    .collect(),
                raw: [("global", USER), ("team", team_document)]
                    .into_iter()
                    .map(|(realm, text)| (realm.to_owned(), toml::from_str(text).unwrap()))
                    .collect(),
            };
            let team = team.clone();
            async move {
                EffectiveConfigReader::new(Arc::new(source))
                    .effective_config(&team)
                    .await
            }
        };

        for redirect in [
            "[self_hosted.servers.box]\nbase_url = \"https://attacker.example/v1\"\n",
            "[self_hosted.servers.box]\napi_style = \"responses\"\n",
        ] {
            match compose(redirect).await {
                Err(ConfigError::CredentialCommandServer {
                    realm,
                    binding,
                    profile,
                    server,
                }) => assert_eq!(
                    (
                        realm.as_str(),
                        binding.as_str(),
                        profile.as_str(),
                        server.as_str()
                    ),
                    ("team", "main", "helper", "box"),
                    "{redirect}"
                ),
                other => panic!("{redirect} is refused: {:?}", other.map(|_| ())),
            }
        }

        let composed = compose("").await.unwrap();
        assert_eq!(
            composed.self_hosted.servers["box"].base_url,
            "http://127.0.0.1:9/v1"
        );
        let moved =
            compose("[self_hosted.servers.open]\nbase_url = \"https://moved.example/v1\"\n")
                .await
                .unwrap();
        assert_eq!(
            moved.self_hosted.servers["open"].base_url,
            "https://moved.example/v1"
        );
    }

    /// A binding whose auth profile runs a credential command on a
    /// self-hosted backend must declare that backend's server: without one,
    /// the binding would authenticate whatever server a request's model is
    /// served by (or the one its default model names), so the composition is
    /// refused, naming the binding and the remedy. A declared one composes.
    #[tokio::test]
    async fn a_command_binding_on_a_self_hosted_backend_must_declare_its_server() {
        use crate::connection::RealmId;

        let document = |server_line: &str| {
            format!(
                "[realm.global]\n\n\
                 [self_hosted.servers.box]\nbase_url = \"http://127.0.0.1:9/v1\"\n\n\
                 [realm.team.backend.boxed]\nprovider = \"self_hosted\"\nbackend_kind = \"self_hosted\"\n{server_line}\n\
                 [realm.team.auth.helper]\nprovider = \"self_hosted\"\nauth_method = \"api_key\"\n\
                 source = {{ kind = \"command\", program = \"/usr/bin/helper\" }}\n\n\
                 [realm.team.binding.main]\nbackend_profile = \"boxed\"\nauth_profile = \"helper\"\n"
            )
        };
        let compose = |user_document: String| {
            let documents = [("global", user_document), ("team", String::new())];
            let source = RawMapSource {
                docs: documents
                    .iter()
                    .map(|(realm, text)| {
                        (
                            (*realm).to_owned(),
                            Config::from_persisted_toml(text).unwrap().0,
                        )
                    })
                    .collect(),
                raw: documents
                    .iter()
                    .map(|(realm, text)| ((*realm).to_owned(), toml::from_str(text).unwrap()))
                    .collect(),
            };
            async move {
                EffectiveConfigReader::new(Arc::new(source))
                    .effective_config(&RealmId::parse("team").unwrap())
                    .await
            }
        };

        let refused = compose(document(""))
            .await
            .expect_err("a command binding with no declared server does not compose");
        let message = refused.to_string();
        assert!(
            message.contains("binding 'main'")
                && message.contains("credential command")
                && message.contains("[realm.team.backend.boxed]")
                && message.contains("declares no server")
                && message.contains("server = \""),
            "{message}"
        );
        compose(document("server = \"box\""))
            .await
            .expect("a command binding that declares its server composes");
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
    /// the merged result still has the legacy shape, so the write persists the
    /// normalized "no fallback policy" form (the `enabled` key dropped, so the
    /// document keeps inheriting) and reports the warning.
    #[tokio::test]
    async fn file_store_patch_over_legacy_model_fallback_default_persists_no_policy()
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
        assert_eq!(preview.model_fallback.enabled, None);
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
        assert_eq!(updated.model_fallback.enabled, None);
        assert_eq!(
            patch_warnings,
            vec![crate::config::ConfigWarning::LegacyModelFallbackDefault]
        );
        let persisted_text = tokio::fs::read_to_string(&path).await?;
        let persisted_raw: toml::Table = toml::from_str(&persisted_text)?;
        assert!(
            persisted_raw
                .get("model_fallback")
                .and_then(|table| table.get("enabled"))
                .is_none(),
            "the persisted doc drops the legacy `enabled` key: {persisted_text}"
        );
        let persisted: Config = toml::from_str(&persisted_text)?;
        assert_eq!(
            persisted.model_fallback,
            crate::config::ModelFallbackConfig::default()
        );

        let (reloaded, warnings) = store.get_with_warnings().await?;
        assert_eq!(reloaded.model_fallback.enabled, None);
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

    /// A 0.8.36 workspace realm doc under a global doc with a reviewed
    /// explicit chain: the legacy child table is no policy, so the child keeps
    /// the parent's chain (as in 0.8.36) and the child's doc is reported.
    #[tokio::test]
    async fn effective_reader_legacy_child_inherits_parent_explicit_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::connection::RealmId;

        let temp = tempfile::tempdir()?;
        let global_dir = temp.path().join("global");
        tokio::fs::create_dir_all(&global_dir).await?;
        tokio::fs::write(
            global_dir.join("config.toml"),
            "[realm.global]\n\n[model_fallback]\nenabled = true\n\n[[model_fallback.chain]]\nmodel = \"backup-openai\"\nprovider = \"openai\"\n",
        )
        .await?;
        let reader = EffectiveConfigReader::new(Arc::new(FileDocSource {
            root: temp.path().to_path_buf(),
        }));

        for (realm, legacy_doc, warning) in [
            (
                "child",
                "[realm.child]\nparent = \"global\"\n\n[model_fallback]\nenabled = true\n",
                crate::config::ConfigWarning::LegacyModelFallbackDefault,
            ),
            (
                "catalog-child",
                "[realm.catalog-child]\nparent = \"global\"\n\n[model_fallback]\nuse_catalog_default_chain = true\n",
                crate::config::ConfigWarning::LegacyModelFallbackCatalogChain,
            ),
        ] {
            let child_dir = temp.path().join(realm);
            tokio::fs::create_dir_all(&child_dir).await?;
            tokio::fs::write(child_dir.join("config.toml"), legacy_doc).await?;
            let child = RealmId::parse(realm)?;

            let (config, warnings) = reader.effective_config_with_warnings(&child).await?;
            assert!(config.model_fallback.is_enabled(), "{realm}");
            assert_eq!(config.model_fallback.chain.len(), 1, "{realm}");
            assert_eq!(config.model_fallback.chain[0].model, "backup-openai");
            config.validate(*crate::model_profile::test_catalog::TEST_CATALOG)?;
            assert_eq!(
                warnings,
                vec![RealmConfigWarning {
                    realm: child.clone(),
                    warning,
                }],
                "{realm}"
            );
        }
        Ok(())
    }

    /// A top-level legacy doc with no parent has no explicit policy to
    /// inherit: fallback is off, with the typed warning.
    #[tokio::test]
    async fn effective_reader_top_level_legacy_doc_is_off_with_warning()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::connection::RealmId;

        let temp = tempfile::tempdir()?;
        let global_dir = temp.path().join("global");
        tokio::fs::create_dir_all(&global_dir).await?;
        tokio::fs::write(
            global_dir.join("config.toml"),
            "[realm.global]\n\n[model_fallback]\nenabled = true\n",
        )
        .await?;
        let reader = EffectiveConfigReader::new(Arc::new(FileDocSource {
            root: temp.path().to_path_buf(),
        }));

        let (config, warnings) = reader
            .effective_config_with_warnings(&RealmId::global())
            .await?;
        assert!(!config.model_fallback.is_enabled());
        assert!(config.model_fallback.chain.is_empty());
        config.validate(*crate::model_profile::test_catalog::TEST_CATALOG)?;
        assert_eq!(
            warnings,
            vec![RealmConfigWarning {
                realm: RealmId::global(),
                warning: crate::config::ConfigWarning::LegacyModelFallbackDefault,
            }]
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

    /// Realm documents at `<root>/<realm>/config.toml`, served both typed and
    /// as raw TOML like the filesystem source every surface composes with, so
    /// composition reads each document's key presence (`child-wins-scalar`).
    struct PresenceDocSource {
        root: PathBuf,
    }

    impl PresenceDocSource {
        fn doc_path(&self, realm: &crate::connection::RealmId) -> PathBuf {
            self.root.join(realm.as_str()).join("config.toml")
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl RealmConfigSource for PresenceDocSource {
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
            let path = self.doc_path(realm);
            if !tokio::fs::try_exists(&path).await? {
                return Ok(None);
            }
            FileConfigStore::new(path, *crate::model_profile::test_catalog::TEST_CATALOG)
                .get_with_warnings()
                .await
                .map(Some)
        }

        async fn raw_config_for_realm(
            &self,
            realm: &crate::connection::RealmId,
        ) -> Result<Option<toml::Value>, ConfigError> {
            let path = self.doc_path(realm);
            if !tokio::fs::try_exists(&path).await? {
                return Ok(None);
            }
            Ok(Some(toml::from_str(
                &tokio::fs::read_to_string(&path).await?,
            )?))
        }
    }

    /// A parent realm that restricts tool policy and DISABLES capabilities
    /// whose defaults are enabled (scheduling, provider web search).
    const POLICY_PARENT_DOC: &str = "[realm.parent]\n\n\
        [tools]\nshell_enabled = true\nmax_concurrent = 3\nschedule_enabled = false\n\n\
        [retry]\nmax_retries = 7\n\n\
        [provider_tools.anthropic]\nweb_search = false\n\n\
        [provider_tools.openai]\nweb_search = false\n\n\
        [provider_tools.gemini]\ngoogle_search = false\n";

    /// A child that inherits all of that, with one explicit key that equals
    /// its struct default.
    const POLICY_CHILD_DOC: &str = "# Child realm: inherits tool policy from its parent.\n\
        [realm.child]\nparent = \"parent\" # the chain edge\n\n\
        [skills]\nenabled = true # explicit, equal to the default\n";

    async fn write_realm_doc(
        root: &Path,
        realm: &str,
        content: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let dir = root.join(realm);
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("config.toml");
        tokio::fs::write(&path, content).await?;
        Ok(path)
    }

    fn child_runtime(child_path: &Path) -> crate::ConfigRuntime {
        let state_path = child_path.with_file_name("config_state.json");
        crate::ConfigRuntime::new(
            Arc::new(FileConfigStore::new(
                child_path.to_path_buf(),
                *crate::model_profile::test_catalog::TEST_CATALOG,
            )),
            state_path,
        )
    }

    fn table_keys(table: &toml::Table) -> Vec<&str> {
        table.keys().map(String::as_str).collect()
    }

    /// Regression: a patch naming one key must write only that key. The
    /// typed round trip materialized every default into the child document,
    /// and composition read them as explicit child overrides: inherited
    /// restrictions were dropped and capabilities the parent DISABLED
    /// (scheduling, provider web search) were silently re-enabled.
    #[tokio::test]
    async fn file_store_patch_keeps_child_presence_and_inherited_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::connection::RealmId;

        let temp = tempfile::tempdir()?;
        write_realm_doc(temp.path(), "parent", POLICY_PARENT_DOC).await?;
        let child_path = write_realm_doc(temp.path(), "child", POLICY_CHILD_DOC).await?;
        let runtime = child_runtime(&child_path);
        let generation = runtime.get().await?.generation;

        let snapshot = runtime
            .patch(
                ConfigDelta(serde_json::json!({
                    "tools": { "mcp_servers": [{
                        "name": "sentinel",
                        "command": "sentinel-mcp",
                        "args": ["--read-only"],
                    }]}
                })),
                Some(generation),
            )
            .await?;
        assert_eq!(snapshot.generation, generation + 1);

        let written = tokio::fs::read_to_string(&child_path).await?;
        let raw: toml::Table = toml::from_str(&written)?;
        assert_eq!(
            table_keys(&raw),
            vec!["realm", "skills", "tools"],
            "only the patched section is added: {written}"
        );
        let tools = raw
            .get("tools")
            .and_then(toml::Value::as_table)
            .ok_or("tools table")?;
        assert_eq!(table_keys(tools), vec!["mcp_servers"], "{written}");
        let skills = raw
            .get("skills")
            .and_then(toml::Value::as_table)
            .ok_or("skills table")?;
        assert_eq!(
            skills.get("enabled").and_then(toml::Value::as_bool),
            Some(true),
            "the explicit default-valued key keeps its presence: {written}"
        );
        assert_eq!(table_keys(skills), vec!["enabled"], "{written}");

        let reader = EffectiveConfigReader::new(Arc::new(PresenceDocSource {
            root: temp.path().to_path_buf(),
        }));
        let effective = reader.effective_config(&RealmId::parse("child")?).await?;
        assert!(effective.tools.shell_enabled, "inherited shell_enabled");
        assert_eq!(
            effective.tools.max_concurrent, 3,
            "inherited max_concurrent"
        );
        assert!(
            !effective.tools.schedule_enabled,
            "the parent's disabled scheduling must stay disabled"
        );
        assert_eq!(effective.retry.max_retries, 7, "inherited retry policy");
        assert!(!effective.provider_tools.anthropic.web_search);
        assert!(
            !effective.provider_tools.openai.web_search,
            "the parent's disabled web search must stay disabled"
        );
        assert!(!effective.provider_tools.gemini.google_search);
        assert!(effective.skills.enabled);
        assert_eq!(
            effective
                .tools
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["sentinel"]
        );
        Ok(())
    }

    /// A stale `expected_generation` is refused before the document is read
    /// or written.
    #[tokio::test]
    async fn file_store_patch_with_stale_generation_leaves_document_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        write_realm_doc(temp.path(), "parent", POLICY_PARENT_DOC).await?;
        let child_path = write_realm_doc(temp.path(), "child", POLICY_CHILD_DOC).await?;
        let runtime = child_runtime(&child_path);
        let stale = runtime.get().await?.generation;
        runtime
            .patch(
                ConfigDelta(serde_json::json!({ "max_tokens": 1234 })),
                Some(stale),
            )
            .await?;
        let before = tokio::fs::read(&child_path).await?;

        let error = runtime
            .patch(
                ConfigDelta(serde_json::json!({ "tools": { "shell_enabled": false } })),
                Some(stale),
            )
            .await
            .expect_err("a stale generation must be refused");
        assert!(
            matches!(
                error,
                crate::ConfigRuntimeError::GenerationConflict { expected, current }
                    if expected == stale && current == stale + 1
            ),
            "{error}"
        );
        assert_eq!(
            tokio::fs::read(&child_path).await?,
            before,
            "a refused write leaves the bytes untouched"
        );
        Ok(())
    }

    /// `null` removes the child's own key, so the field inherits again.
    #[tokio::test]
    async fn file_store_patch_null_removes_key_and_restores_inheritance()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::connection::RealmId;

        let temp = tempfile::tempdir()?;
        write_realm_doc(temp.path(), "parent", POLICY_PARENT_DOC).await?;
        let child_path = write_realm_doc(
            temp.path(),
            "child",
            "[realm.child]\nparent = \"parent\"\n\n[tools]\nmax_concurrent = 5\n",
        )
        .await?;
        let reader = EffectiveConfigReader::new(Arc::new(PresenceDocSource {
            root: temp.path().to_path_buf(),
        }));
        let child = RealmId::parse("child")?;
        assert_eq!(
            reader.effective_config(&child).await?.tools.max_concurrent,
            5
        );

        child_runtime(&child_path)
            .patch(
                ConfigDelta(serde_json::json!({ "tools": { "max_concurrent": null } })),
                None,
            )
            .await?;

        let written = tokio::fs::read_to_string(&child_path).await?;
        let raw: toml::Table = toml::from_str(&written)?;
        assert!(
            raw.get("tools")
                .and_then(toml::Value::as_table)
                .is_none_or(|tools| !tools.contains_key("max_concurrent")),
            "the key is removed, not reset to its default: {written}"
        );
        let effective = reader.effective_config(&child).await?;
        assert_eq!(
            effective.tools.max_concurrent, 3,
            "the parent's value applies again"
        );
        assert!(!effective.tools.schedule_enabled);
        Ok(())
    }

    /// Realm MCP servers are literal, and a write is judged entry by entry
    /// against the document's own persisted servers: a new or changed entry
    /// holding an environment reference is refused and writes nothing, while
    /// an unchanged legacy entry does not block an unrelated write, a repair
    /// or a removal (resolving the MCP set still refuses it).
    #[tokio::test]
    async fn file_store_writes_refuse_new_or_changed_mcp_server_env_references()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        let original = "[agent]\nmodel = \"test-anthropic-third\"\n";
        tokio::fs::write(&path, original).await?;
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let referencing = |name: &str| {
            serde_json::json!({
                "name": name,
                "command": "tool",
                "env": { "TOKEN": "${HOST_SECRET}" },
            })
        };
        let refused_entry = |result: &Result<Config, ConfigError>, server: &str| {
            matches!(result, Err(ConfigError::RealmMcpServerEnvReference(reference))
                if reference.server == server && reference.field == "env")
        };

        // New entries are refused, on patch and on set.
        let patched = store
            .patch(ConfigDelta(serde_json::json!({
                "tools": { "mcp_servers": [referencing("exfil")] }
            })))
            .await;
        assert!(refused_entry(&patched, "exfil"), "{patched:?}");
        assert!(
            !format!("{patched:?}").contains("HOST_SECRET"),
            "{patched:?}"
        );
        let mut config: Config = toml::from_str(original)?;
        config.tools.mcp_servers = vec![serde_json::from_value(referencing("exfil"))?];
        let set = store.set(config).await;
        assert!(
            matches!(&set, Err(ConfigError::RealmMcpServerEnvReference(_))),
            "{set:?}"
        );
        assert_eq!(tokio::fs::read_to_string(&path).await?, original);

        // A document that already holds two legacy entries.
        let existing = "[[tools.mcp_servers]]\nname = \"legacy-a\"\ncommand = \"tool\"\n\
            env = { TOKEN = \"${HOST_SECRET}\" }\n\n\
            [[tools.mcp_servers]]\nname = \"legacy-b\"\ncommand = \"tool\"\n\
            env = { TOKEN = \"${HOST_SECRET}\" }\n";
        tokio::fs::write(&path, existing).await?;
        assert_eq!(store.get().await?.tools.mcp_servers.len(), 2);

        // An unrelated patch succeeds and keeps the legacy entries, which
        // resolving the MCP set still refuses.
        let unrelated = store
            .patch(ConfigDelta(serde_json::json!({ "max_tokens": 200 })))
            .await?;
        assert_eq!(unrelated.max_tokens, Some(200));
        assert!(matches!(
            crate::mcp_config::compose_effective_mcp_servers(&unrelated.tools.mcp_servers, vec![]),
            Err(crate::mcp_config::EffectiveMcpServersError::RealmEnvReference(_))
        ));

        // Changing a legacy entry, even in a field other than the reference,
        // or copying it under a new name, is a new or changed entry.
        let before = tokio::fs::read_to_string(&path).await?;
        let mut changed = referencing("legacy-a");
        changed["connect_timeout_secs"] = serde_json::json!(30);
        for (servers, server) in [
            (vec![changed, referencing("legacy-b")], "legacy-a"),
            (
                vec![
                    referencing("legacy-a"),
                    referencing("legacy-b"),
                    referencing("legacy-c"),
                ],
                "legacy-c",
            ),
        ] {
            let result = store
                .patch(ConfigDelta(
                    serde_json::json!({ "tools": { "mcp_servers": servers } }),
                ))
                .await;
            assert!(refused_entry(&result, server), "{server}: {result:?}");
            assert_eq!(tokio::fs::read_to_string(&path).await?, before);
        }

        // Repairing one legacy entry beside another unchanged one, adding a
        // valid server, removing a legacy entry, and a get-then-set round
        // trip all succeed.
        let repaired = serde_json::json!({ "name": "legacy-a", "command": "tool" });
        let added = serde_json::json!({ "name": "docs", "command": "docs-tool" });
        store
            .patch(ConfigDelta(serde_json::json!({ "tools": { "mcp_servers": [
                repaired, referencing("legacy-b"), added.clone()
            ]}})))
            .await?;
        let round_trip = store.get().await?;
        store.set(round_trip).await?;
        store
            .patch(ConfigDelta(serde_json::json!({ "tools": { "mcp_servers": [
                serde_json::json!({ "name": "legacy-a", "command": "tool" }), added
            ]}})))
            .await?;
        let names: Vec<String> = store
            .get()
            .await?
            .tools
            .mcp_servers
            .into_iter()
            .map(|server| server.name)
            .collect();
        assert_eq!(names, vec!["legacy-a", "docs"]);

        // A full replace never depends on parsing the old document's own
        // sections, and an unreadable old document exempts nothing. (A
        // document that is not TOML at all is never replaced by an ordinary
        // write: it could state the governance section.)
        let unreadable = "[tools]\nmcp_servers = 5\n";
        tokio::fs::write(&path, unreadable).await?;
        let mut valid: Config = toml::from_str(original)?;
        valid.tools.mcp_servers = vec![serde_json::from_value(serde_json::json!({
            "name": "docs", "command": "docs-tool"
        }))?];
        store.set(valid).await?;
        tokio::fs::write(&path, unreadable).await?;
        let mut invalid: Config = toml::from_str(original)?;
        invalid.tools.mcp_servers = vec![serde_json::from_value(referencing("legacy-a"))?];
        assert!(matches!(
            store.set(invalid).await,
            Err(ConfigError::RealmMcpServerEnvReference(_))
        ));
        assert_eq!(tokio::fs::read_to_string(&path).await?, unreadable);
        Ok(())
    }

    /// Comments, formatting and key order outside the patched keys survive.
    #[tokio::test]
    async fn file_store_patch_keeps_comments_and_order_of_untouched_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        let original = "# Operator notes for this realm.\n\
            max_tokens = 100 # per-turn cap\n\n\
            [agent]\n# pinned for the review team\nmodel = \"test-anthropic-third\"\n\n\
            [tools]\nmax_concurrent = 4   # keep low on this host\nshell_enabled = true\n";
        tokio::fs::write(&path, original).await?;
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );

        store
            .patch(ConfigDelta(serde_json::json!({
                "max_tokens": 200,
                "tools": { "shell_enabled": false },
            })))
            .await?;

        let written = tokio::fs::read_to_string(&path).await?;
        assert_eq!(
            written,
            "# Operator notes for this realm.\n\
            max_tokens = 200 # per-turn cap\n\n\
            [agent]\n# pinned for the review team\nmodel = \"test-anthropic-third\"\n\n\
            [tools]\nmax_concurrent = 4   # keep low on this host\nshell_enabled = false\n",
            "only the patched values change"
        );
        Ok(())
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

    const GOVERNED: &str = "[[governance.review.rules]]\n\
target = { kind = \"mcp_server\", server = \"mail\" }\n\
tier = \"r2\"\n";

    fn governed() -> Config {
        Config::from_persisted_toml(GOVERNED)
            .expect("governed document")
            .0
    }

    /// An ordinary write is not governance administration: neither a full
    /// replace nor a patch can change or remove the document's governance
    /// section, and a refused write leaves the document's bytes as they
    /// were. A write that keeps the section exactly, or never names it,
    /// succeeds and keeps it.
    #[tokio::test]
    async fn an_ordinary_write_never_changes_the_documents_governance()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        let administered = format!("max_tokens = 4096\n\n{GOVERNED}");
        tokio::fs::write(&path, &administered).await?;
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let committed = store.get().await?;
        assert_eq!(committed.governance, governed().governance);

        let mut changed = committed.clone();
        let rule = &mut changed.governance.as_mut().expect("stated").review.rules[0];
        rule.tier = crate::StockReviewTier::R3;
        rule.consent_assurance = Some(crate::StockConsentAssurance::PersonAuthenticated);
        let mut omitted = committed.clone();
        omitted.governance = None;
        let mut emptied = committed.clone();
        emptied.governance = Some(crate::GovernanceConfig::default());
        for candidate in [changed, omitted, emptied] {
            assert!(matches!(
                store.set(candidate).await,
                Err(ConfigError::GovernanceAdministration)
            ));
            assert_eq!(tokio::fs::read_to_string(&path).await?, administered);
        }
        for delta in [
            serde_json::json!({ "governance": null }),
            serde_json::json!({ "governance": { "review": null } }),
            serde_json::json!({ "governance": { "review": { "rules": [] } } }),
            serde_json::json!({ "governance": { "review": { "reviewer": {
                "identity": { "model": "test-anthropic-second", "provider": "anthropic" },
                "max_output_tokens": 64
            } } } }),
        ] {
            assert!(
                matches!(
                    store.patch(ConfigDelta(delta.clone())).await,
                    Err(ConfigError::GovernanceAdministration)
                ),
                "{delta}"
            );
            assert_eq!(tokio::fs::read_to_string(&path).await?, administered);
        }

        let patched = store
            .patch(ConfigDelta(serde_json::json!({ "max_tokens": 2048 })))
            .await?;
        assert_eq!(patched.governance, committed.governance);
        let mut kept = store.get().await?;
        assert_eq!(kept.max_tokens, Some(2048));
        kept.max_tokens = Some(1024);
        store.set(kept).await?;
        assert_eq!(store.get().await?.governance, committed.governance);
        Ok(())
    }

    /// A write cannot state a governance section in a document that states
    /// none, even one equal to what the document inherits: the restated
    /// copy would shadow every later change of the parent's. A document that
    /// cannot be parsed but names the section is never replaced; one that
    /// cannot name it is, as a full replace never otherwise parses it.
    #[tokio::test]
    async fn an_ordinary_write_cannot_state_governance_a_document_does_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        tokio::fs::write(&path, "max_tokens = 4096\n").await?;
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let inherited = governed().governance;
        let mut restated = store.get().await?;
        restated.governance = inherited.clone();
        assert!(matches!(
            store.set(restated).await,
            Err(ConfigError::GovernanceAdministration)
        ));
        let section = serde_json::to_value(&inherited)?;
        assert!(matches!(
            store
                .patch(ConfigDelta(serde_json::json!({ "governance": section })))
                .await,
            Err(ConfigError::GovernanceAdministration)
        ));
        assert_eq!(
            tokio::fs::read_to_string(&path).await?,
            "max_tokens = 4096\n"
        );

        // A document that is not TOML could state the section: it is never
        // replaced by an ordinary write, whatever it names.
        for unreadable in ["[governance\n", "[tools\n"] {
            tokio::fs::write(&path, unreadable).await?;
            assert!(matches!(
                store.set(Config::default()).await,
                Err(ConfigError::GovernanceAdministration)
            ));
            assert_eq!(tokio::fs::read_to_string(&path).await?, unreadable);
        }
        Ok(())
    }

    /// An explicitly empty section is stated: an ordinary write can neither
    /// add one to a document that states none nor remove one a document
    /// states. A section is found by its parsed key, so a quoted or escaped
    /// key names it too.
    #[tokio::test]
    async fn an_explicitly_empty_or_quoted_section_is_kept_exactly()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        let store = FileConfigStore::new(
            path.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        // Adding an empty section to a document that states none.
        tokio::fs::write(&path, "max_tokens = 4096\n").await?;
        let mut added = store.get().await?;
        added.governance = Some(crate::GovernanceConfig::default());
        assert!(matches!(
            store.set(added).await,
            Err(ConfigError::GovernanceAdministration)
        ));
        assert!(matches!(
            store
                .patch(ConfigDelta(serde_json::json!({ "governance": {} })))
                .await,
            Err(ConfigError::GovernanceAdministration)
        ));
        assert_eq!(
            tokio::fs::read_to_string(&path).await?,
            "max_tokens = 4096\n"
        );
        // Removing an empty section a document states.
        let stated = "max_tokens = 4096\n\n[governance]\n";
        tokio::fs::write(&path, stated).await?;
        let read = store.get().await?;
        assert_eq!(read.governance, Some(crate::GovernanceConfig::default()));
        let mut removed = read.clone();
        removed.governance = None;
        assert!(matches!(
            store.set(removed).await,
            Err(ConfigError::GovernanceAdministration)
        ));
        assert!(matches!(
            store
                .patch(ConfigDelta(serde_json::json!({ "governance": null })))
                .await,
            Err(ConfigError::GovernanceAdministration)
        ));
        assert_eq!(tokio::fs::read_to_string(&path).await?, stated);
        // Keeping it is not a change: the section is written back as stated.
        let mut kept = read;
        kept.max_tokens = Some(2048);
        store.set(kept).await?;
        assert_eq!(
            store.get().await?.governance,
            Some(crate::GovernanceConfig::default())
        );
        // A quoted, escaped key is the same section.
        let quoted = "[\"gov\\u0065rnance\".review]\nrules = []\n";
        tokio::fs::write(&path, quoted).await?;
        assert!(matches!(
            store.set(Config::default()).await,
            Err(ConfigError::GovernanceAdministration)
        ));
        assert_eq!(tokio::fs::read_to_string(&path).await?, quoted);
        Ok(())
    }

    #[tokio::test]
    async fn the_memory_store_keeps_its_governance_section() {
        let store = MemoryConfigStore::new(
            governed(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let mut omitted = governed();
        omitted.governance = None;
        assert!(matches!(
            store.set(omitted).await,
            Err(ConfigError::GovernanceAdministration)
        ));
        assert!(matches!(
            store
                .patch(ConfigDelta(serde_json::json!({ "governance": null })))
                .await,
            Err(ConfigError::GovernanceAdministration)
        ));
        store
            .patch(ConfigDelta(serde_json::json!({ "max_tokens": 2048 })))
            .await
            .expect("an unrelated patch");
        assert_eq!(
            store.get().await.expect("config").governance,
            governed().governance
        );
    }

    /// The generation the sidecar beside `document` publishes.
    async fn generation_beside(document: &Path) -> u64 {
        crate::config_runtime::read_generation_at(&parent_directory(document).join(GENERATION_FILE))
            .await
            .expect("a readable generation")
    }

    /// Every public write of a file store is fenced and moves the generation
    /// a config runtime over the same document reads: a full replace, a
    /// patch and a patch that returns its warnings.
    #[tokio::test]
    async fn every_file_store_write_moves_the_generation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let document = temp.path().join("config.toml");
        let store = FileConfigStore::new(
            document.clone(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        );
        let runtime =
            crate::ConfigRuntime::new(Arc::new(store.clone()), temp.path().join(GENERATION_FILE));
        assert_eq!(generation_beside(&document).await, 0);

        let config = Config {
            max_tokens: Some(100),
            ..Config::default()
        };
        store.set(config).await.expect("a full replace");
        assert_eq!(generation_beside(&document).await, 1);
        store
            .patch(ConfigDelta(serde_json::json!({ "max_tokens": 200 })))
            .await
            .expect("a patch");
        assert_eq!(generation_beside(&document).await, 2);
        store
            .patch_with_warnings(ConfigDelta(serde_json::json!({ "max_tokens": 300 })))
            .await
            .expect("a patch with warnings");
        let snapshot = runtime.get().await.expect("the runtime's read");
        assert_eq!(snapshot.generation, 3);
        assert_eq!(snapshot.config.max_tokens, Some(300));
    }

    /// A store that bootstraps its document on read creates it only when
    /// its admission fence and its config custody are both free now, and
    /// never waits: under a held fence, under a held custody, and inside a
    /// config runtime's own read (whose read custody is held), it creates
    /// nothing and reads the missing document as empty. With both free it
    /// creates the document from its template and moves the generation.
    #[tokio::test]
    async fn create_on_read_creates_nothing_under_a_held_fence_or_custody_and_never_waits() {
        let temp = tempfile::tempdir().expect("tempdir");
        let document = temp.path().join("config.toml");
        let bootstrapping = || FileConfigStore {
            path: document.clone(),
            create_if_missing: true,
            catalog: *crate::model_profile::test_catalog::TEST_CATALOG,
            realm_chain: None,
        };
        let store = bootstrapping();
        let quickly = std::time::Duration::from_secs(1);

        let fence = store.admission_fence().expect("a document fence");
        fence.ensure_exists().expect("the fence file");
        let mut hold = crate::realm_admission::AdmissionHold::try_acquire(vec![fence])
            .expect("a holder takes the fence");
        let read = tokio::time::timeout(quickly, store.get())
            .await
            .expect("create-on-read never waits for a held fence")
            .expect("a read");
        assert_eq!(read.max_tokens, Config::default().max_tokens);
        assert!(!document.exists(), "nothing is created under a held fence");
        hold.release();

        let custody = crate::config_runtime::ConfigWriterLock::try_hold(&store.custody_path())
            .expect("the custody lock")
            .expect("the custody is free");
        tokio::time::timeout(quickly, store.get())
            .await
            .expect("create-on-read never waits for a held custody")
            .expect("a read");
        assert!(
            !document.exists(),
            "nothing is created under a held custody"
        );
        drop(custody);

        let runtime =
            crate::ConfigRuntime::new(Arc::new(bootstrapping()), temp.path().join(GENERATION_FILE));
        tokio::time::timeout(quickly, runtime.get())
            .await
            .expect("a runtime's read never waits for its own custody")
            .expect("a runtime's read");
        assert!(
            !document.exists(),
            "nothing is created inside a runtime's own read"
        );

        store.get().await.expect("a read with both free");
        assert_eq!(
            tokio::fs::read_to_string(&document)
                .await
                .expect("the created document"),
            Config::template_toml()
        );
        assert_eq!(generation_beside(&document).await, 1);
    }

    /// The mode of the files a file store writes, on Unix. A document can
    /// hold credential material (an inline secret, a command source's
    /// environment, a realm MCP server's literal arguments, environment and
    /// headers), so the store writes it, and the generation sidecar beside
    /// it, owner-only whatever the process umask; in a directory whose setgid
    /// bit says it is shared through its group, also readable by that group.
    #[cfg(unix)]
    mod document_modes {
        use super::*;
        use crate::process_isolation_tests::{
            create_setgid_directory, mode_bits, set_mode, under_each_umask,
            under_each_umask_refusing_writes,
        };
        use std::os::unix::fs::MetadataExt as _;

        fn store_at(document: &Path) -> FileConfigStore {
            FileConfigStore::new(
                document.to_path_buf(),
                *crate::model_profile::test_catalog::TEST_CATALOG,
            )
        }

        /// A file an earlier release or an operator left at `path` with
        /// `mode`, whatever this process's umask.
        fn existing(path: &Path, content: &str, mode: u32) {
            std::fs::write(path, content).expect("write the existing file");
            set_mode(path, mode);
        }

        async fn patch_max_tokens(store: &FileConfigStore, max_tokens: u32) {
            store
                .patch(ConfigDelta(serde_json::json!({ "max_tokens": max_tokens })))
                .await
                .expect("a patch");
        }

        fn temporary_files(directory: &Path, prefix: &str) -> Vec<PathBuf> {
            std::fs::read_dir(directory)
                .expect("the document's directory")
                .map(|entry| entry.expect("a directory entry").path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
                })
                .collect()
        }

        /// A document a store creates, and the generation sidecar beside
        /// it, are 0600 under any umask. The directory the write created
        /// keeps the mode the umask gives it. RED before: both files took
        /// the umask's mode (0644 under 022), readable by every local user.
        #[test]
        fn a_document_a_store_creates_is_owner_only_under_any_umask() {
            under_each_umask(
                module_path!(),
                "a_document_a_store_creates_is_owner_only_under_any_umask",
                |umask| async move {
                    let temp = tempfile::tempdir().expect("tempdir");
                    let directory = temp.path().join("created-by-the-write");
                    let document = directory.join("config.toml");
                    let config = Config {
                        max_tokens: Some(100),
                        ..Config::default()
                    };
                    store_at(&document)
                        .set(config)
                        .await
                        .expect("a full replace");
                    assert_eq!(
                        mode_bits(&directory) & 0o777,
                        0o777 & !umask,
                        "the directory keeps the umask's mode"
                    );
                    assert_eq!(mode_bits(&document), 0o600, "the document");
                    assert_eq!(
                        mode_bits(&directory.join(GENERATION_FILE)),
                        0o600,
                        "the generation sidecar"
                    );
                    assert!(
                        temporary_files(&directory, ".config").is_empty(),
                        "no temporary file is left"
                    );
                },
            );
        }

        /// The document a bootstrapping store creates from its template is
        /// 0600 under any umask, with its generation sidecar.
        #[test]
        fn a_template_document_is_owner_only_under_any_umask() {
            under_each_umask(
                module_path!(),
                "a_template_document_is_owner_only_under_any_umask",
                |_umask| async move {
                    let temp = tempfile::tempdir().expect("tempdir");
                    let document = temp.path().join("config.toml");
                    let store = FileConfigStore {
                        path: document.clone(),
                        create_if_missing: true,
                        catalog: *crate::model_profile::test_catalog::TEST_CATALOG,
                        realm_chain: None,
                    };
                    store.get().await.expect("a read that creates the template");
                    assert_eq!(
                        std::fs::read_to_string(&document).expect("the created document"),
                        Config::template_toml()
                    );
                    assert_eq!(mode_bits(&document), 0o600, "the template document");
                    assert_eq!(
                        mode_bits(&temp.path().join(GENERATION_FILE)),
                        0o600,
                        "the generation sidecar"
                    );
                },
            );
        }

        /// A rewrite of a document an earlier release left readable by
        /// group and others ends owner-only, and so does its sidecar.
        #[test]
        fn a_rewrite_of_a_readable_document_ends_owner_only_under_any_umask() {
            under_each_umask(
                module_path!(),
                "a_rewrite_of_a_readable_document_ends_owner_only_under_any_umask",
                |_umask| async move {
                    let temp = tempfile::tempdir().expect("tempdir");
                    let document = temp.path().join("config.toml");
                    let sidecar = temp.path().join(GENERATION_FILE);
                    existing(&document, "max_tokens = 100\n", 0o644);
                    existing(&sidecar, "{\"generation\":3}", 0o644);
                    let store = store_at(&document);
                    patch_max_tokens(&store, 200).await;
                    assert_eq!(store.get().await.expect("a read").max_tokens, Some(200));
                    assert_eq!(generation_beside(&document).await, 4);
                    assert_eq!(mode_bits(&document), 0o600, "the rewritten document");
                    assert_eq!(mode_bits(&sidecar), 0o600, "the rewritten sidecar");
                },
            );
        }

        /// A rewrite never loosens a mode: a 0600 document stays 0600, and
        /// a document its owner made read-only (0400) stays 0400, with its
        /// sidecar. RED before: the rewrite gave both the umask's mode.
        #[test]
        fn a_rewrite_never_loosens_a_stricter_mode_under_any_umask() {
            under_each_umask(
                module_path!(),
                "a_rewrite_never_loosens_a_stricter_mode_under_any_umask",
                |_umask| async move {
                    for mode in [0o600, 0o400] {
                        let temp = tempfile::tempdir().expect("tempdir");
                        let document = temp.path().join("config.toml");
                        let sidecar = temp.path().join(GENERATION_FILE);
                        existing(&document, "max_tokens = 100\n", mode);
                        existing(&sidecar, "{\"generation\":3}", mode);
                        let store = store_at(&document);
                        patch_max_tokens(&store, 200).await;
                        assert_eq!(store.get().await.expect("a read").max_tokens, Some(200));
                        assert_eq!(generation_beside(&document).await, 4);
                        assert_eq!(mode_bits(&document), mode, "the document keeps {mode:o}");
                        assert_eq!(mode_bits(&sidecar), mode, "the sidecar keeps {mode:o}");
                    }
                },
            );
        }

        /// A write the filesystem refuses fails, and publishes nothing: no
        /// new document, the old one still in place (the same file), and no
        /// generation sidecar. Every write to a regular file is refused in
        /// this row's process. RED before: the write's error was dropped
        /// (the file was synced without waiting for the write's result), so
        /// the replace reported success and renamed its empty temporary
        /// file over the document.
        #[test]
        fn a_refused_write_fails_and_publishes_nothing() {
            under_each_umask_refusing_writes(
                module_path!(),
                "a_refused_write_fails_and_publishes_nothing",
                |_umask| async move {
                    let config = || Config {
                        max_tokens: Some(100),
                        ..Config::default()
                    };
                    let temp = tempfile::tempdir().expect("tempdir");
                    let document = temp.path().join("config.toml");
                    let sidecar = temp.path().join(GENERATION_FILE);
                    store_at(&document)
                        .set(config())
                        .await
                        .expect_err("the refused write fails the replace");
                    assert!(!document.exists(), "no document was published");
                    assert!(!sidecar.exists(), "no generation was published");

                    // An old document (empty: this process can write none)
                    // stays the file it was.
                    let kept = temp.path().join("kept");
                    std::fs::create_dir(&kept).expect("a directory");
                    let document = kept.join("config.toml");
                    std::fs::File::create(&document).expect("the old document");
                    let old = std::fs::metadata(&document).expect("its metadata").ino();
                    store_at(&document)
                        .set(config())
                        .await
                        .expect_err("the refused write fails the replace");
                    assert_eq!(
                        std::fs::metadata(&document).expect("its metadata").ino(),
                        old,
                        "the old document was not replaced"
                    );

                    let guard = AdmissionGuard::try_acquire(vec![AdmissionFence::beside(&sidecar)])
                        .expect("the directory's fence");
                    crate::config_runtime::write_generation_at(&guard, &sidecar, 1)
                        .await
                        .expect_err("the refused write fails the publication");
                    assert!(!sidecar.exists(), "no generation was published");
                },
            );
        }

        /// The temporary file of a write is owner-only from its creation,
        /// before any byte of the document is in it. Every write to a
        /// regular file is refused in this row's process, so the write stops
        /// right after it created its temporary file, which stays as the
        /// creation left it: empty, and 0600 by name. The same for the
        /// sidecar's temporary file. RED before: both were created with the
        /// umask's mode (0644 under 022).
        #[test]
        fn a_temporary_file_is_owner_only_before_any_byte_under_any_umask() {
            under_each_umask_refusing_writes(
                module_path!(),
                "a_temporary_file_is_owner_only_before_any_byte_under_any_umask",
                |_umask| async move {
                    let temp = tempfile::tempdir().expect("tempdir");
                    let document = temp.path().join("config.toml");
                    let config = Config {
                        max_tokens: Some(100),
                        ..Config::default()
                    };
                    store_at(&document)
                        .set(config)
                        .await
                        .expect_err("the refused write fails the replace");
                    assert!(!document.exists(), "nothing was published");
                    let temporaries = temporary_files(temp.path(), ".config.tmp.");
                    assert_eq!(temporaries.len(), 1, "the write's temporary file stays");
                    let created = std::fs::metadata(&temporaries[0]).expect("its metadata");
                    assert_eq!(created.len(), 0, "no byte was written");
                    assert_eq!(
                        mode_bits(&temporaries[0]),
                        0o600,
                        "the document's temporary file"
                    );

                    let sidecar = temp.path().join(GENERATION_FILE);
                    let guard = AdmissionGuard::try_acquire(vec![AdmissionFence::beside(&sidecar)])
                        .expect("the directory's fence");
                    crate::config_runtime::write_generation_at(&guard, &sidecar, 1)
                        .await
                        .expect_err("the refused write fails the publication");
                    let temporaries = temporary_files(temp.path(), ".config_state.tmp.");
                    assert_eq!(temporaries.len(), 1, "the sidecar's temporary file stays");
                    let created = std::fs::metadata(&temporaries[0]).expect("its metadata");
                    assert_eq!(created.len(), 0, "no byte was written");
                    assert_eq!(
                        mode_bits(&temporaries[0]),
                        0o600,
                        "the sidecar's temporary file"
                    );
                },
            );
        }

        /// A document path that is a symbolic link behaves as before: the
        /// read follows the link, and the write replaces the link itself
        /// with a regular file, leaving the file it pointed to untouched
        /// (its bytes and its mode). The new file is owner-only.
        #[test]
        fn a_write_through_a_symbolic_link_replaces_the_link_under_any_umask() {
            under_each_umask(
                module_path!(),
                "a_write_through_a_symbolic_link_replaces_the_link_under_any_umask",
                |_umask| async move {
                    let temp = tempfile::tempdir().expect("tempdir");
                    let target = temp.path().join("elsewhere.toml");
                    existing(&target, "max_tokens = 100\n", 0o644);
                    let directory = temp.path().join("linked");
                    std::fs::create_dir(&directory).expect("the document's directory");
                    let document = directory.join("config.toml");
                    std::os::unix::fs::symlink(&target, &document).expect("the link");

                    let store = store_at(&document);
                    assert_eq!(
                        store
                            .get()
                            .await
                            .expect("a read through the link")
                            .max_tokens,
                        Some(100)
                    );
                    patch_max_tokens(&store, 200).await;

                    let written = std::fs::symlink_metadata(&document).expect("the document");
                    assert!(
                        written.file_type().is_file(),
                        "the write replaced the link with a regular file"
                    );
                    assert_eq!(store.get().await.expect("a read").max_tokens, Some(200));
                    assert_eq!(
                        std::fs::read_to_string(&target).expect("the link's old target"),
                        "max_tokens = 100\n",
                        "the file the link pointed to keeps its bytes"
                    );
                    assert_eq!(mode_bits(&target), 0o644, "and its mode");
                    assert_eq!(mode_bits(&document), 0o600, "the new file is owner-only");
                },
            );
        }

        /// A directory with the setgid bit is shared through its group on
        /// purpose: a document written there, and its sidecar, are 0640
        /// under any umask and belong to the directory's group, never
        /// group-writable and never readable by others. The same directory
        /// without the bit gets 0600, and a document an operator made 0600
        /// in the shared directory stays 0600. RED before: every one of
        /// them took the umask's mode.
        ///
        /// The row asserts the mode and the group. It does not read the
        /// document as a second user of that group: the test user cannot
        /// become one.
        #[test]
        fn a_setgid_directory_shares_a_document_with_its_group_under_any_umask() {
            under_each_umask(
                module_path!(),
                "a_setgid_directory_shares_a_document_with_its_group_under_any_umask",
                |_umask| async move {
                    let temp = tempfile::tempdir().expect("tempdir");
                    let directory = temp.path().join("shared");
                    let group = create_setgid_directory(&directory);

                    let document = directory.join("config.toml");
                    let sidecar = directory.join(GENERATION_FILE);
                    let store = store_at(&document);
                    patch_max_tokens(&store, 100).await;
                    for file in [&document, &sidecar] {
                        assert_eq!(
                            mode_bits(file),
                            0o640,
                            "{} is group-readable",
                            file.display()
                        );
                        assert_eq!(
                            std::fs::metadata(file).expect("the file").gid(),
                            group,
                            "{} belongs to the directory's group",
                            file.display()
                        );
                    }

                    // A document an earlier release left group-writable and
                    // readable by others is narrowed to 0640.
                    set_mode(&document, 0o664);
                    patch_max_tokens(&store, 200).await;
                    assert_eq!(
                        mode_bits(&document),
                        0o640,
                        "never group-writable or public"
                    );

                    // An operator's 0600 in the shared directory stays 0600.
                    set_mode(&document, 0o600);
                    patch_max_tokens(&store, 300).await;
                    assert_eq!(mode_bits(&document), 0o600, "an operator's 0600 is kept");

                    // The same directory without the bit: owner-only.
                    set_mode(&directory, 0o770);
                    std::fs::remove_file(&document).expect("remove the document");
                    std::fs::remove_file(&sidecar).expect("remove the sidecar");
                    patch_max_tokens(&store, 400).await;
                    assert_eq!(mode_bits(&document), 0o600, "no setgid bit, no group read");
                    assert_eq!(mode_bits(&sidecar), 0o600, "nor for the sidecar");
                },
            );
        }
    }
}
