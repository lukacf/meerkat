use clap::{Parser, ValueEnum};
use meerkat::AgentFactory;
use meerkat_core::{
    Config, ConfigResolvedPaths, ConfigRuntime, ConfigStore, FileConfigStore, RealmConfig,
    RealmSelection, TaggedConfigStore,
};
use meerkat_store::{RealmBackend, RealmOrigin};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "rkat-rpc")]
#[command(about = "Meerkat JSON-RPC stdio server")]
struct Cli {
    /// Explicit realm ID to join.
    #[arg(long)]
    realm: Option<String>,
    /// Start in isolated mode (new opaque realm).
    #[arg(long)]
    isolated: bool,
    /// Optional instance ID inside a realm.
    #[arg(long)]
    instance: Option<String>,
    /// Backend when creating a new realm.
    #[arg(long, value_enum)]
    realm_backend: Option<RealmBackendArg>,
    /// Override state root (directory that contains realm dirs).
    #[arg(long)]
    state_root: Option<PathBuf>,
    /// Optional context root for conventions (skills/hooks/AGENTS/MCP config).
    #[arg(long)]
    context_root: Option<PathBuf>,
    /// Optional user-global conventions root.
    #[arg(long)]
    user_config_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    Jsonl,
    Redb,
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            RealmBackendArg::Jsonl => RealmBackend::Jsonl,
            RealmBackendArg::Redb => RealmBackend::Redb,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let selection = RealmConfig::selection_from_inputs(
        cli.realm.clone(),
        cli.isolated,
        RealmSelection::Isolated,
    )?;
    let origin_hint = match &selection {
        RealmSelection::Explicit { .. } => RealmOrigin::Explicit,
        RealmSelection::Isolated => RealmOrigin::Generated,
        RealmSelection::WorkspaceDerived { .. } => RealmOrigin::Workspace,
    };
    let realm_cfg = RealmConfig {
        selection,
        instance_id: cli.instance.clone(),
        backend_hint: cli
            .realm_backend
            .map(Into::into)
            .map(|b: RealmBackend| b.as_str().to_string()),
        state_root: cli.state_root.clone(),
    };
    let locator = realm_cfg.resolve_locator()?;

    let backend_hint = cli
        .realm_backend
        .map(Into::into)
        .or(Some(RealmBackend::Redb));
    let (manifest, session_store) = meerkat_store::open_realm_session_store_in(
        &locator.state_root,
        &locator.realm_id,
        backend_hint,
        Some(origin_hint),
    )
    .await?;
    let realm_paths = meerkat_store::realm_paths_in(&locator.state_root, &locator.realm_id);

    let base_store: Arc<dyn ConfigStore> =
        Arc::new(FileConfigStore::new(realm_paths.config_path.clone()));
    let tagged = TaggedConfigStore::new(
        base_store,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(locator.realm_id.clone()),
            instance_id: cli.instance.clone(),
            backend: Some(manifest.backend.as_str().to_string()),
            resolved_paths: Some(ConfigResolvedPaths {
                root: realm_paths.root.display().to_string(),
                manifest_path: realm_paths.manifest_path.display().to_string(),
                config_path: realm_paths.config_path.display().to_string(),
                sessions_redb_path: realm_paths.sessions_redb_path.display().to_string(),
                sessions_jsonl_dir: realm_paths.sessions_jsonl_dir.display().to_string(),
            }),
        },
    );
    let config_store: Arc<dyn ConfigStore> = Arc::new(tagged);
    let mut config = config_store
        .get()
        .await
        .unwrap_or_else(|_| Config::default());
    config.apply_env_overrides()?;
    let identity_registry =
        meerkat_rpc::session_runtime::SessionRuntime::build_skill_identity_registry(&config)
            .map_err(|err| {
                std::io::Error::other(format!(
                    "failed to build skills source-identity registry from config: {err}"
                ))
            })?;

    let store_path = match manifest.backend {
        RealmBackend::Jsonl => realm_paths.sessions_jsonl_dir.clone(),
        RealmBackend::Redb => realm_paths.root.clone(),
    };
    let project_root = cli
        .context_root
        .clone()
        .unwrap_or_else(|| realm_paths.root.clone());

    let mut factory = AgentFactory::new(store_path)
        .session_store(session_store.clone())
        .runtime_root(realm_paths.root.clone())
        .project_root(project_root)
        .builtins(true)
        .shell(true)
        .subagents(true)
        .memory(true);
    if let Some(context_root) = cli.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = cli.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    let config_runtime = Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        realm_paths.root.join("config_state.json"),
    ));
    let mut runtime = meerkat_rpc::session_runtime::SessionRuntime::new_with_config_store(
        factory,
        config,
        Arc::clone(&config_store),
        64,
        session_store,
    );
    runtime.set_skill_identity_registry(identity_registry);
    runtime.set_realm_context(
        Some(locator.realm_id.clone()),
        cli.instance.clone(),
        Some(manifest.backend.as_str().to_string()),
    );
    runtime.set_config_runtime(config_runtime);
    let runtime = Arc::new(runtime);

    let lease = meerkat_store::start_realm_lease_in(
        &locator.state_root,
        &locator.realm_id,
        cli.instance.as_deref(),
        "rkat-rpc",
    )
    .await?;
    let serve_result = meerkat_rpc::serve_stdio(runtime, config_store).await;
    lease.shutdown().await;
    serve_result?;

    Ok(())
}
