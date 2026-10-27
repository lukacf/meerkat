use std::{path::PathBuf, sync::Arc};

use meerkat_core::{
    Config, EffectiveConfigReader, ResolvedStorage, StorageLayout, StorageLayoutInputs,
    runtime_bootstrap::{RealmConfig, RealmRootDefault, RealmSelection},
};
use meerkat_store::{FilesystemRealmConfigSource, RealmBackend};

use crate::ConnectError;

/// Explicit surface inputs to the native storage resolver.
#[derive(Clone, Debug)]
pub struct RealmOptions {
    pub context_root: PathBuf,
    pub state_root: Option<PathBuf>,
    pub user_config_root: Option<PathBuf>,
}

/// A stored realm and its exact root. Equal names at different roots stay distinct.
#[derive(Clone, Debug)]
pub struct RealmChoice {
    pub realm_id: String,
    pub state_root: PathBuf,
    pub backend: RealmBackend,
}

impl RealmOptions {
    fn inputs(&self) -> StorageLayoutInputs {
        StorageLayoutInputs {
            invocation_context: self.context_root.clone(),
            explicit_state_root: self.state_root.clone(),
            user_config_root: self.user_config_root.clone(),
            default_root: Some(RealmRootDefault::UserGlobal),
            probe_local_candidate: true,
        }
    }

    /// Resolve only. Creating/opening persistence is a separate explicit step.
    pub fn resolve(&self, realm_id: &str) -> Result<ResolvedStorage, ConnectError> {
        StorageLayout::resolve(
            self.inputs(),
            &RealmConfig {
                selection: RealmSelection::Explicit {
                    realm_id: realm_id.to_owned(),
                },
                ..RealmConfig::default()
            },
        )
        .map_err(|e| ConnectError::native("resolve selected realm", e))
    }

    pub fn select(&self, choice: &RealmChoice) -> Result<ResolvedStorage, ConnectError> {
        Self {
            state_root: Some(choice.state_root.clone()),
            ..self.clone()
        }
        .resolve(&choice.realm_id)
    }
}

/// Read native manifests without opening a runtime or creating a realm.
pub async fn discover_realms(options: &RealmOptions) -> Result<Vec<RealmChoice>, ConnectError> {
    let storage = StorageLayout::resolve(options.inputs(), &RealmConfig::default())
        .map_err(|e| ConnectError::native("resolve realm inventory roots", e))?;
    let mut choices = Vec::new();
    for root in storage.layout.realm_root_candidates() {
        for entry in meerkat_store::list_realm_manifests_in(root)
            .await
            .map_err(|e| ConnectError::native("read realm inventory", e))?
        {
            choices.push(RealmChoice {
                realm_id: entry.manifest.realm.as_str().to_owned(),
                state_root: root.clone(),
                backend: entry.manifest.backend,
            });
        }
    }
    choices.sort_by(|a, b| (&a.realm_id, &a.state_root).cmp(&(&b.realm_id, &b.state_root)));
    choices.dedup_by(|a, b| a.realm_id == b.realm_id && a.state_root == b.state_root);
    Ok(choices)
}

/// The native owner of the selected realm's own document and inherited view.
pub(crate) fn config_source(
    storage: &ResolvedStorage,
) -> Result<Arc<FilesystemRealmConfigSource>, ConnectError> {
    let global_config = storage
        .layout
        .user_rkat_root()
        .ok_or_else(|| {
            ConnectError::InvalidInput("No user configuration root is available".into())
        })?
        .join("config.toml");
    let source = FilesystemRealmConfigSource::new(
        &storage.locator.state_root,
        global_config,
        meerkat_models::canonical(),
    );
    Ok(Arc::new(source))
}

/// Compose inherited config through the native reader. Never write this merged view.
pub async fn read_config(storage: &ResolvedStorage) -> Result<Config, ConnectError> {
    let source = config_source(storage)?;
    let mut config = EffectiveConfigReader::new(source)
        .effective_config(&storage.locator.realm)
        .await
        .map_err(|e| ConnectError::native("read selected realm configuration", e))?;
    config
        .apply_env_overrides()
        .map_err(|e| ConnectError::native("apply native environment overrides", e))?;
    config
        .validate(meerkat_models::canonical())
        .map_err(|e| ConnectError::native("validate selected realm configuration", e))?;
    Ok(config)
}

/// Read the selected realm's own MCP list and native configuration generation.
/// This deliberately uses the same document projection as native realm tools,
/// never the inherited/environment-composed view returned by `read_config`.
pub(crate) async fn read_own_mcp_config(
    storage: &ResolvedStorage,
) -> Result<(Vec<meerkat_core::McpServerConfig>, u64), ConnectError> {
    let runtime = own_config_runtime(storage)?;
    meerkat_core::McpRealmPersistTarget::new(&runtime)
        .servers()
        .await
        .map_err(|error| {
            ConnectError::native("Read reviewed realm configuration generation", error)
        })
}

pub(crate) fn own_config_runtime(
    storage: &ResolvedStorage,
) -> Result<meerkat_core::ConfigRuntime, ConnectError> {
    let source = config_source(storage)?;
    let path = source.config_doc_path(&storage.locator.realm);
    Ok(meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            path.clone(),
            meerkat_models::canonical(),
        )),
        path.with_file_name("config_state.json"),
    ))
}

#[cfg(test)]
mod own_config_tests {
    use super::*;
    use meerkat_core::{ConfigRuntime, FileConfigStore, McpRealmPersistTarget, McpServerConfig};

    #[tokio::test]
    async fn own_generation_excludes_parent_servers_and_detects_a_later_write() {
        let directory = tempfile::tempdir().unwrap();
        let storage = RealmOptions {
            context_root: directory.path().to_owned(),
            state_root: Some(directory.path().join("state")),
            user_config_root: Some(directory.path().join("home")),
        }
        .resolve("work")
        .unwrap();
        let global = storage.layout.user_rkat_root().unwrap().join("config.toml");
        let source = FilesystemRealmConfigSource::new(
            &storage.locator.state_root,
            global.clone(),
            meerkat_models::canonical(),
        );
        let path = source.config_doc_path(&storage.locator.realm);
        let runtime = |path: PathBuf| {
            ConfigRuntime::new(
                Arc::new(FileConfigStore::new(
                    path.clone(),
                    meerkat_models::canonical(),
                )),
                path.with_file_name("config_state.json"),
            )
        };
        let server =
            |name: &str| McpServerConfig::stdio(name, "/bin/true", vec![], Default::default());
        let parent = runtime(global);
        McpRealmPersistTarget::new(&parent)
            .add(server("inherited"), Some(0))
            .await
            .unwrap();
        let own = runtime(path);
        McpRealmPersistTarget::new(&own)
            .add(server("existing"), Some(0))
            .await
            .unwrap();
        let (servers, reviewed_generation) = read_own_mcp_config(&storage).await.unwrap();
        assert_eq!(
            servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            ["existing"]
        );
        assert_eq!(reviewed_generation, 1);
        McpRealmPersistTarget::new(&own)
            .add(server("concurrent"), Some(reviewed_generation))
            .await
            .unwrap();
        assert!(
            McpRealmPersistTarget::new(&own)
                .add(server("reviewed"), Some(reviewed_generation))
                .await
                .is_err()
        );
        let (after, generation) = read_own_mcp_config(&storage).await.unwrap();
        assert_eq!(generation, 2);
        assert!(!after.iter().any(|server| server.name == "reviewed"));
    }
}
