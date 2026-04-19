use clap::{Parser, ValueEnum};
use meerkat::AgentFactory;
use meerkat_core::{
    Config, ConfigResolvedPaths, ConfigRuntime, ConfigStore, FileConfigStore, RealmConfig,
    RealmSelection, TaggedConfigStore,
};
use meerkat_store::{RealmBackend, RealmOrigin};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser, Debug)]
#[command(name = env!("CARGO_BIN_NAME"), version = env!("CARGO_PKG_VERSION"))]
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
    /// Listen on a TCP address instead of stdin/stdout.
    ///
    /// Example: --tcp 127.0.0.1:4800 or --tcp 0.0.0.0:4800
    #[arg(long)]
    tcp: Option<String>,
    /// Listen on a sibling WebSocket address for realtime channels.
    ///
    /// Example: --realtime-ws 127.0.0.1:4900
    #[arg(long)]
    realtime_ws: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    Jsonl,
    Sqlite,
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            RealmBackendArg::Jsonl => RealmBackend::Jsonl,
            RealmBackendArg::Sqlite => RealmBackend::Sqlite,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

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

    let backend_hint = cli.realm_backend.map(Into::into);
    let (manifest, persistence) = meerkat::open_realm_persistence_in(
        &locator.state_root,
        &locator.realm_id,
        backend_hint,
        Some(origin_hint),
    )
    .await?;
    let session_store = persistence.session_store();
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
                sessions_sqlite_path: Some(realm_paths.sessions_sqlite_path.display().to_string()),
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

    let store_path = persistence
        .store_path()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| match manifest.backend {
            RealmBackend::Jsonl => realm_paths.sessions_jsonl_dir.clone(),
            RealmBackend::Sqlite => realm_paths.root.clone(),
        });
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
        .schedule(true)
        .memory(true);
    if let Some(context_root) = cli.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = cli.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    let skill_runtime = factory.build_skill_runtime(&config).await;

    // Boot-time OpenAI credential acquisition for the realtime WS
    // sideband. Routes through the canonical ProviderRuntimeRegistry
    // (dogma §1/§7/§14) — no helper-local env read. When config binds
    // openai via `[realm.*]`, that binding wins; otherwise the
    // env-default realm synthesis reads OPENAI_API_KEY (with RKAT_*
    // precedence) via the resolver's env_lookup seam. Resolved here
    // before `config` is moved into SessionRuntime.
    let realtime_openai_factory: Option<Arc<dyn meerkat_client::RealtimeSessionFactory>> =
        meerkat::resolve_provider_api_key(&config, meerkat_core::Provider::OpenAI)
            .await
            .map(|api_key| {
                Arc::new(meerkat_client::OpenAiRealtimeSessionFactory::new(Arc::new(
                    meerkat_client::OpenAiLiveClient::new(api_key),
                ))) as Arc<dyn meerkat_client::RealtimeSessionFactory>
            });

    let config_runtime = Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        realm_paths.root.join("config_state.json"),
    ));
    let mut runtime = meerkat_rpc::session_runtime::SessionRuntime::new_with_config_store(
        factory,
        config,
        Arc::clone(&config_store),
        64,
        persistence,
        meerkat_rpc::router::NotificationSink::noop(),
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
    let realtime_ws = if let Some(ref realtime_ws_addr) = cli.realtime_ws {
        let listener = tokio::net::TcpListener::bind(realtime_ws_addr).await?;
        let actual_realtime_ws_addr = listener.local_addr()?;
        let actual_ws_url = format!(
            "ws://{actual_realtime_ws_addr}{}",
            meerkat_rpc::REALTIME_WS_PATH
        );
        let mut host = meerkat_rpc::RealtimeWsHost::new(actual_ws_url.clone());
        if let Some(session_factory) = realtime_openai_factory.clone() {
            host = host.with_session_factory(session_factory);
        }
        let host = Arc::new(host);
        eprintln!("rkat-rpc listening on {actual_ws_url}");
        let rt = Arc::clone(&runtime);
        let cs = Arc::clone(&config_store);
        let ws_host = Arc::clone(&host);
        Some((
            host,
            tokio::spawn(async move {
                meerkat_rpc::serve_realtime_ws_listener(listener, ws_host, rt, cs).await
            }),
        ))
    } else {
        None
    };

    let serve_result = if let Some(ref tcp_addr) = cli.tcp {
        eprintln!("rkat-rpc listening on tcp://{tcp_addr}");
        if let Some((realtime_ws_host, _)) = &realtime_ws {
            meerkat_rpc::serve_tcp_with_realtime_ws_host(
                tcp_addr,
                runtime,
                config_store,
                skill_runtime,
                Some(Arc::clone(realtime_ws_host)),
            )
            .await
        } else {
            meerkat_rpc::serve_tcp(tcp_addr, runtime, config_store, skill_runtime).await
        }
    } else if let Some((realtime_ws_host, _)) = &realtime_ws {
        meerkat_rpc::serve_stdio_with_skill_runtime_and_realtime_ws_host(
            runtime,
            config_store,
            skill_runtime,
            Some(Arc::clone(realtime_ws_host)),
        )
        .await
    } else {
        meerkat_rpc::serve_stdio_with_skill_runtime(runtime, config_store, skill_runtime).await
    };
    if let Some((_, realtime_ws_handle)) = realtime_ws {
        realtime_ws_handle.abort();
        let _ = realtime_ws_handle.await;
    }
    lease.shutdown().await;
    serve_result?;

    Ok(())
}
