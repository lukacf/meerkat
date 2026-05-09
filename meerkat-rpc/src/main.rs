#![allow(clippy::large_futures)]

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
    /// Loopback binds are accepted by default. Non-loopback binds require
    /// --allow-remote and should be used only behind an authenticated and
    /// encrypted transport wrapper.
    ///
    /// Example: --tcp 127.0.0.1:4800
    #[arg(long)]
    tcp: Option<String>,
    /// Permit --tcp to bind non-loopback addresses.
    ///
    /// This is an explicit transport exposure opt-in, not an auth mechanism.
    #[arg(long)]
    allow_remote: bool,
    /// Start a live WebSocket listener on this address.
    ///
    /// Exposes the `/live/ws` endpoint for live audio/text channels.
    /// Example: --live-ws 127.0.0.1:4900
    #[arg(long)]
    live_ws: Option<String>,
    /// Scheme advertised in the `live/open` bootstrap URL: `ws` or `wss`.
    ///
    /// Defaults to `ws`. Use `wss` when the live listener is fronted by a
    /// TLS-terminating proxy. The scheme is purely advertisement; the
    /// actual `--live-ws` listener accepts plain WebSocket bytes.
    #[arg(long, value_enum, default_value_t = LiveWsScheme::Ws)]
    live_ws_scheme: LiveWsScheme,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LiveWsScheme {
    Ws,
    Wss,
}

impl LiveWsScheme {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Ws => "ws",
            Self::Wss => "wss",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    Jsonl,
    Sqlite,
}

/// Non-blocking shared-lock file writer for `tracing_subscriber`'s
/// `with_writer`. Flushes on every write so live scenarios see events as
/// they land. Errors are swallowed — the primary stderr layer is still
/// present, so this path cannot drop a scenario.
struct FileTraceWriter {
    inner: std::sync::Arc<std::sync::Mutex<std::fs::File>>,
}

impl std::io::Write for FileTraceWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut guard) = self.inner.lock() {
            let _ = guard.write_all(buf);
            let _ = guard.flush();
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if let Ok(mut guard) = self.inner.lock() {
            let _ = guard.flush();
        }
        Ok(())
    }
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            RealmBackendArg::Jsonl => RealmBackend::Jsonl,
            RealmBackendArg::Sqlite => RealmBackend::Sqlite,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    // Optional on-disk trace sink: when `RKAT_RPC_TRACE_FILE` is set,
    // append-write every tracing event to the given path in addition to
    // the usual stderr writer. Targeted debugging helper for live smoke
    // scenarios where stderr might be consumed by a pump/harness.
    let file_writer = std::env::var("RKAT_RPC_TRACE_FILE")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .and_then(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
                .map(std::sync::Mutex::new)
                .map(std::sync::Arc::new)
        });
    let registry = tracing_subscriber::registry().with(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    );
    if let Some(file_writer) = file_writer {
        let make_writer = move || FileTraceWriter {
            inner: file_writer.clone(),
        };
        registry
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(tracing_subscriber::fmt::layer().with_writer(make_writer))
            .init();
    } else {
        registry
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
    }

    let cli = Cli::parse();
    let tcp_bind_policy = if cli.allow_remote {
        meerkat_rpc::secure_rpc::TcpBindPolicy::allow_remote()
    } else {
        meerkat_rpc::secure_rpc::TcpBindPolicy::local_only()
    };
    if let Some(ref tcp_addr) = cli.tcp {
        meerkat_rpc::secure_rpc::validate_tcp_bind_policy("rpc", tcp_addr, tcp_bind_policy)
            .map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string())
            })?;
    }
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
        locator.realm.as_str(),
        backend_hint,
        Some(origin_hint),
    )
    .await?;
    let session_store = persistence.session_store();
    let realm_paths =
        meerkat_store::realm_paths_in(&locator.state_root, &locator.realm.to_string());

    let base_store: Arc<dyn ConfigStore> =
        Arc::new(FileConfigStore::new(realm_paths.config_path.clone()));
    let tagged = TaggedConfigStore::new(
        base_store,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(locator.realm.as_str().to_string()),
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
    config
        .validate()
        .map_err(|err| std::io::Error::other(format!("invalid runtime config: {err}")))?;
    let cli_user_root = cli.user_config_root.clone();
    let default_user_root = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let identity_registry =
        meerkat_rpc::session_runtime::SessionRuntime::build_skill_identity_registry(
            &config,
            cli.context_root.as_deref(),
            cli_user_root.as_deref().or(default_user_root.as_deref()),
        )
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

    let skill_runtime = factory.build_skill_runtime(&config).await?;

    // N74: only build the OpenAI realtime factory when --live-ws is configured;
    // otherwise live/* is not exposed at all and the factory work is wasted.
    // B15: when --live-ws IS set, factory build failure must fail at startup
    // rather than leaving live/open exposed with no provider wired.
    let live_session_factory: Option<
        Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>,
    > = if cli.live_ws.is_some() {
        match factory.build_openai_realtime_session_factory(&config).await {
            Ok(f) => Some(f),
            Err(err) => {
                return Err(format!(
                    "--live-ws is configured but the OpenAI realtime session factory \
                     could not be built: {err}"
                )
                .into());
            }
        }
    } else {
        None
    };

    let config_runtime = Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        realm_paths.root.join("config_state.json"),
    ));
    let mut runtime = meerkat_rpc::session_runtime::SessionRuntime::new_with_config_store(
        factory,
        config.clone(),
        Arc::clone(&config_store),
        config.max_sessions(),
        persistence,
        meerkat_rpc::router::NotificationSink::noop(),
    );
    runtime.set_skill_identity_roots(
        cli.context_root.clone(),
        cli_user_root.clone().or(default_user_root.clone()),
    );
    runtime.set_skill_identity_registry(identity_registry);
    let realm_id_typed = meerkat_core::connection::RealmId::parse(
        locator.realm.as_str().to_string(),
    )
    .map_err(|e| {
        std::io::Error::other(format!(
            "invalid realm id '{}': {e}",
            locator.realm.as_str()
        ))
    })?;
    runtime.set_realm_context(
        Some(realm_id_typed),
        cli.instance.clone(),
        Some(manifest.backend.as_str().to_string()),
    );
    runtime.set_config_runtime(config_runtime);
    let runtime = Arc::new(runtime);

    let lease = meerkat_store::start_realm_lease_in(
        &locator.state_root,
        locator.realm.as_str(),
        cli.instance.as_deref(),
        "rkat-rpc",
    )
    .await?;
    let live_ws = if let Some(ref live_ws_addr) = cli.live_ws {
        let listener = tokio::net::TcpListener::bind(live_ws_addr).await?;
        let actual_addr = listener.local_addr()?;
        eprintln!(
            "rkat-rpc live-ws listening on ws://{actual_addr}{path} (advertised scheme: {scheme})",
            path = meerkat_live::LIVE_WS_PATH,
            scheme = cli.live_ws_scheme.as_str(),
        );
        // Wave-3: install the surface projection sink so adapter observations
        // become canonical Meerkat semantic facts (A1-A6, A14). Without the
        // sink, `apply_observation` only updates host status and projection
        // becomes a silent no-op.
        let projection_sink: std::sync::Arc<dyn meerkat_live::LiveProjectionSink> =
            std::sync::Arc::new(
                meerkat_rpc::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                    &runtime,
                )),
            );
        let host = std::sync::Arc::new(
            meerkat_live::LiveAdapterHost::new().with_projection_sink(projection_sink),
        );
        let ws_state = std::sync::Arc::new(meerkat_live::LiveWsState::new(host));
        let ws_state_clone = std::sync::Arc::clone(&ws_state);
        let handle = tokio::spawn(async move {
            meerkat_live::serve_live_ws_listener(listener, ws_state_clone).await
        });
        Some((ws_state, actual_addr, handle))
    } else {
        None
    };

    // N82: scheme is configurable via --live-ws-scheme (default `ws`).
    // The actual listener is plaintext WebSocket; `wss` is for advertisement
    // when a TLS-terminating proxy fronts the live listener.
    let live_ws_scheme = cli.live_ws_scheme.as_str();
    let live_ws_config = live_ws
        .as_ref()
        .map(|(state, addr, _)| meerkat_rpc::LiveWsConfig {
            state: std::sync::Arc::clone(state),
            base_url: format!("{live_ws_scheme}://{addr}"),
            session_factory: live_session_factory.clone(),
        });

    let serve_result = if let Some(ref tcp_addr) = cli.tcp {
        eprintln!("rkat-rpc listening on tcp://{tcp_addr}");
        meerkat_rpc::serve_tcp_with_options(
            tcp_addr,
            runtime,
            config_store,
            skill_runtime,
            live_ws_config,
        )
        .await
    } else {
        meerkat_rpc::serve_stdio_with_options(runtime, config_store, skill_runtime, live_ws_config)
            .await
    };

    if let Some((_, _, handle)) = live_ws {
        handle.abort();
        let _ = handle.await;
    }
    lease.shutdown().await;
    serve_result?;

    Ok(())
}
