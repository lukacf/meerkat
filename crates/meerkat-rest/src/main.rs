//! Meerkat REST API Server
//!
//! Provides HTTP endpoints for running and managing Meerkat agents.
//!
//! # Environment Variables
//!
//! - `ANTHROPIC_API_KEY`: Required API key for Anthropic

use clap::{Parser, ValueEnum};
use meerkat::surface::report_fatal_error;
use meerkat_core::{RealmConfig, RealmSelection, RuntimeBootstrap};
use meerkat_rest::{AppState, router};
use meerkat_store::RealmBackend;
use std::process::ExitCode;
use std::{net::SocketAddr, path::PathBuf};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser, Debug)]
#[command(name = "rkat-rest", version = env!("CARGO_PKG_VERSION"))]
struct Args {
    /// Explicit realm ID. Reuse to share state across processes/surfaces.
    #[arg(long)]
    realm: Option<String>,
    /// Start in isolated mode (new generated realm).
    #[arg(long)]
    isolated: bool,
    /// Optional instance ID for this server process.
    #[arg(long)]
    instance: Option<String>,
    /// Realm backend when creating a new realm.
    #[arg(long, value_enum)]
    realm_backend: Option<RealmBackendArg>,
    /// Optional override for realm state root.
    #[arg(long)]
    state_root: Option<PathBuf>,
    /// Optional context root for filesystem conventions.
    #[arg(long)]
    context_root: Option<PathBuf>,
    /// Optional user-level config root for additive conventions.
    #[arg(long)]
    user_config_root: Option<PathBuf>,
    /// Expose resolved filesystem paths in config API responses.
    #[arg(long, default_value_t = false)]
    expose_paths: bool,
    /// OS process confinement for the shell, command hooks and MCP stdio
    /// servers this server launches. `workspace` confines each launch to
    /// --context-root; `trusted-host` runs them unconfined. Replaces the
    /// config's `[confinement]` for this process.
    #[arg(long, value_enum, value_name = "MODE")]
    confinement: Option<ConfinementModeArg>,
    /// IP network access of workspace-confined launches (default: denied).
    #[arg(long, value_enum, value_name = "ACCESS", requires = "confinement")]
    confinement_network: Option<ConfinementNetworkArg>,
    /// An extra read-only directory for workspace-confined launches.
    /// Repeatable.
    #[arg(long, value_name = "PATH", requires = "confinement")]
    confinement_read: Vec<PathBuf>,
    /// An extra writable directory for workspace-confined launches.
    /// Repeatable.
    #[arg(long, value_name = "PATH", requires = "confinement")]
    confinement_write: Vec<PathBuf>,
}

/// `--confinement`: how this server confines the processes it launches.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConfinementModeArg {
    /// Launch as the trusted host, with no OS confinement.
    TrustedHost,
    /// Confine each launch to --context-root and the extra directories.
    Workspace,
}

impl From<ConfinementModeArg> for meerkat_core::ConfinementFlagMode {
    fn from(value: ConfinementModeArg) -> Self {
        match value {
            ConfinementModeArg::TrustedHost => Self::TrustedHost,
            ConfinementModeArg::Workspace => Self::Workspace,
        }
    }
}

/// `--confinement-network`: IP access of workspace-confined launches.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum ConfinementNetworkArg {
    /// No IP sockets.
    Denied,
    /// IP sockets over the host's own network.
    Unrestricted,
}

impl From<ConfinementNetworkArg> for meerkat_core::WorkspaceNetwork {
    fn from(value: ConfinementNetworkArg) -> Self {
        match value {
            ConfinementNetworkArg::Denied => Self::Denied,
            ConfinementNetworkArg::Unrestricted => Self::Unrestricted,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RealmBackendArg {
    Jsonl,
    Memory,
    Sqlite,
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            RealmBackendArg::Jsonl => RealmBackend::Jsonl,
            RealmBackendArg::Memory => RealmBackend::Memory,
            RealmBackendArg::Sqlite => RealmBackend::Sqlite,
        }
    }
}

/// Filter applied when `RUST_LOG` is unset: the server's own events (its
/// library's and this binary's, whose target is `rkat_rest`), the HTTP
/// layer, and the host's confinement startup report
/// (`meerkat::host_confinement`), which every host prints at startup.
const DEFAULT_TRACE_FILTER: &str =
    "meerkat_rest=info,rkat_rest=info,meerkat::host_confinement=info,tower_http=debug";

fn main() -> ExitCode {
    // SAFETY: the first statement of main: no other thread exists yet and
    // nothing has used SQLite, as the switch requires.
    #[allow(unsafe_code)]
    let _ = unsafe { meerkat_sqlite::disable_memory_statistics_at_process_start() };
    // One documented worker-stack budget for every host binary; the main
    // future runs on a budgeted thread too, not the platform main thread.
    match meerkat_runtime::host_stack::run_host("rkat-rest", || async {
        // Render the Display chain, not `Result`'s Debug: a storage refusal's
        // remedy sentence lives only in Display.
        match run().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => report_fatal_error("rkat-rest", err.as_ref()),
        }
    }) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("rkat-rest: {err}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let selection = RealmConfig::selection_from_inputs(
        args.realm.clone(),
        args.isolated,
        RealmSelection::Isolated,
    )?;
    // Relative confinement paths resolve against the working directory, read
    // only when a flag needs it: a server started without one does not
    // depend on its working directory, which may have been removed.
    let flags = meerkat_core::ConfinementFlags {
        mode: args.confinement.map(Into::into),
        network: args.confinement_network.map(Into::into),
        read: args.confinement_read.clone(),
        write: args.confinement_write.clone(),
    };
    let working_directory = if flags.needs_working_directory() {
        std::env::current_dir()?
    } else {
        PathBuf::new()
    };
    let confinement = flags.selection(&working_directory)?;

    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| DEFAULT_TRACE_FILTER.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Build app state
    let state = AppState::load_with_bootstrap_options_and_confinement(
        RuntimeBootstrap {
            realm: RealmConfig {
                selection,
                instance_id: args.instance,
                backend_hint: args
                    .realm_backend
                    .map(Into::into)
                    .map(|b: RealmBackend| b.as_str().to_string()),
                state_root: args.state_root,
            },
            context: meerkat_core::ContextConfig {
                context_root: args.context_root,
                user_config_root: args.user_config_root,
            },
        },
        args.expose_paths,
        confinement,
    )
    .await?;
    // Read the current config early: API-key warning + startup log below.
    // The bootstrap above already refused on an unloadable head document;
    // this re-read propagates the same typed `ConfigError` so rkat-rest has
    // one read path with one failure mode instead of a silent default.
    let mut config = state.config_store.get().await.map_err(|err| {
        format!(
            "failed to read head realm config for realm '{}': {err}",
            state.realm
        )
    })?;
    if let Err(err) = config.apply_env_overrides() {
        tracing::warn!("Failed to apply env overrides: {}", err);
    }
    // `default_model` is a startup-time snapshot of the canonical
    // create-session default-model ladder, logged for operators; the live
    // value is re-resolved per request from the current config (config is
    // runtime-mutable via PUT/PATCH /config).
    tracing::info!(
        realm_id = %state.realm,
        backend = %state.backend,
        store_path = %state.store_path.display(),
        default_model = %meerkat::resolve_create_session_default_model(&config),
        max_tokens = state.max_tokens,
        "Starting Meerkat REST server"
    );
    // Plan §6.9/§6.10 deleted the legacy `config.provider` enum block
    // and the `providers.{api_keys,base_urls}` shared maps. Post-0.6.0
    // the REST server relies on env vars (ANTHROPIC_API_KEY /
    // OPENAI_API_KEY / GEMINI_API_KEY, RKAT_*-prefixed overrides) or
    // `[realm.<id>]` TOML blocks. The no-key warning inspects both.
    let env_var_names: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "RKAT_ANTHROPIC_API_KEY",
        "RKAT_OPENAI_API_KEY",
        "RKAT_GEMINI_API_KEY",
    ];
    let has_api_key =
        !config.realm.is_empty() || env_var_names.iter().any(|v| std::env::var(v).is_ok());
    if !has_api_key {
        tracing::warn!(
            "No provider API key configured (config, realm, or environment). \
             API calls will fail until a key is set."
        );
    }

    // Parse host and port from config (non-secret settings)
    let addr: SocketAddr = format!("{}:{}", state.rest_host, state.rest_port)
        .parse()
        .map_err(|e| format!("Invalid host:port combination: {e}"))?;

    // Build router with middleware. Keep a clone for shutdown cleanup.
    let shutdown_state = state.clone();
    let app = router(state)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive());

    tracing::info!("Listening on http://{}", addr);

    // Run server with graceful shutdown. The shutdown future awaits the
    // signal, then stops schedule delivery (so no occurrence admits work
    // into a session the shutdown tears down) and unregisters the runtime's
    // sessions, and only then resolves, so axum's drain starts after the
    // unregisters. On a runtime with a persistence store they requeue: the
    // run in flight of a session that commits each round is suspended for the
    // session's next start, and the open request waiting on it is answered
    // (`409` `requeued_across_stop`) instead of holding the drain open until
    // the run ends.
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let signal_state = shutdown_state.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            signal_state.shutdown_schedule_host().await;
            meerkat_rest::unregister_runtime_sessions_for_shutdown(&signal_state).await;
        })
        .await?;

    // The listener accepted requests until the drain began, so a session
    // they registered after the first pass is unregistered here, with the
    // same disposition, before the requests still tracked are cancelled.
    meerkat_rest::unregister_runtime_sessions_for_shutdown(&shutdown_state).await;

    // Cancel the requests still tracked and abort stragglers.
    shutdown_state
        .request_executor
        .shutdown_and_abort_stragglers()
        .await;

    // Shut down all live MCP adapters to close connections cleanly.
    #[cfg(feature = "mcp")]
    meerkat_rest::shutdown_all_mcp_sessions(&shutdown_state).await;

    tracing::info!("Server shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!("Failed to install Ctrl+C handler: {}", e);
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!("Failed to install signal handler: {}", e);
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("Received Ctrl+C, shutting down...");
        },
        () = terminate => {
            tracing::info!("Received SIGTERM, shutting down...");
        },
    }
}
