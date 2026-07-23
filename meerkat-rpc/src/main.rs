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
    /// Exposes the `/live/ws` endpoint for live audio/text channels with
    /// model-gated image input.
    /// Example: --live-ws 127.0.0.1:4900
    #[arg(long)]
    live_ws: Option<String>,
    /// Enable live WebRTC signaling over JSON-RPC.
    ///
    /// Requires the `live-webrtc` Cargo feature. The browser/client still
    /// calls `live/open` followed by `live/webrtc/answer`; no separate media
    /// dependency is pulled into the RPC crate.
    #[cfg(feature = "live-webrtc")]
    #[arg(long)]
    live_webrtc: bool,
    /// Per-tool-call timeout for live provider tool calls, in milliseconds.
    ///
    /// Defaults to `meerkat_live::DEFAULT_LIVE_TOOL_TIMEOUT`. Longer values
    /// are useful for manual smoke tests of slow tools such as image
    /// generation; stuck tools still terminalize through the typed live timeout
    /// path instead of holding the provider turn indefinitely.
    #[arg(long)]
    live_tool_timeout_ms: Option<u64>,
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
    Memory,
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
            RealmBackendArg::Memory => RealmBackend::Memory,
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
    let tcp_bind_policy = resolved_tcp_bind_policy(cli.allow_remote);
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
    // Realm-id-first dual-root resolution through the storage layout (the
    // single path authority; the layout's realm-root candidates arm the
    // cross-candidate first-start reservation at open). The project-local
    // candidate is probed only when an explicit --context-root was given,
    // keeping the no-flags server behavior (user-global data-dir root)
    // unchanged.
    let invocation_context = cli
        .context_root
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let meerkat_core::ResolvedStorage {
        layout,
        locator,
        root_choice,
    } = meerkat_core::StorageLayout::resolve(
        meerkat_core::StorageLayoutInputs {
            invocation_context,
            explicit_state_root: cli.state_root.clone(),
            user_config_root: cli.user_config_root.clone(),
            default_root: Some(meerkat_core::RealmRootDefault::UserGlobal),
            probe_local_candidate: cli.context_root.is_some(),
        },
        &realm_cfg,
    )?;
    tracing::info!(
        realm = %locator.realm,
        state_root = %locator.state_root.display(),
        root_choice = ?root_choice,
        "resolved realm storage"
    );

    let backend_hint = cli.realm_backend.map(Into::into);
    let (manifest, persistence) = meerkat::storage_provider::open_realm_persistence_with_layout(
        layout.clone(),
        locator.realm.as_str(),
        backend_hint,
        Some(origin_hint),
    )
    .await?;
    let session_store = persistence.session_store();
    let realm_paths =
        meerkat_store::realm_paths_in(&locator.state_root, &locator.realm.to_string());

    // Shared filesystem realm-config source, composed before any config
    // store so the head store routes through the SAME doc mapping. The
    // reserved `global` realm maps to the layout's user-global config doc
    // (honoring --user-config-root); when no home-like root resolves, a
    // path under the state root that will never exist, so `global` yields
    // `None`. Routing the head store through `config_doc_path` means an
    // explicit --state-root cannot shadow the global document with
    // `<state_root>/global/config.toml`.
    let global_doc = layout
        .global_config_path()
        .unwrap_or_else(|| locator.state_root.join("__no_global__").join("config.toml"));
    let fs_realm_config_source = meerkat_store::FilesystemRealmConfigSource::new(
        locator.state_root.clone(),
        global_doc,
        meerkat_models::canonical(),
    );
    let head_config_doc = fs_realm_config_source.config_doc_path(&locator.realm);
    let realm_config_source: Arc<dyn meerkat_core::RealmConfigSource> =
        Arc::new(fs_realm_config_source);

    let base_store: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(
        head_config_doc.clone(),
        meerkat_models::canonical(),
    ));
    let tagged = TaggedConfigStore::new(
        base_store,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(locator.realm.as_str().to_string()),
            instance_id: cli.instance.clone(),
            backend: Some(manifest.backend.as_str().to_string()),
            resolved_paths: Some(ConfigResolvedPaths {
                root: realm_paths.root.display().to_string(),
                manifest_path: realm_paths.manifest_path.display().to_string(),
                config_path: head_config_doc.display().to_string(),
                sessions_sqlite_path: Some(realm_paths.sessions_sqlite_path.display().to_string()),
                sessions_jsonl_dir: realm_paths.sessions_jsonl_dir.display().to_string(),
            }),
        },
    );
    let config_store: Arc<dyn ConfigStore> = Arc::new(tagged);
    // Compose inherited startup facts before building any process-owned
    // runtime capability. In particular, `[mob_host]` listen/advertise may
    // live in a parent realm while the head only tightens a resource bound.
    let head_config = config_store
        .get()
        .await
        .unwrap_or_else(|_| Config::default());
    let mut config = meerkat_core::EffectiveConfigReader::new(Arc::clone(&realm_config_source))
        .effective_config_over_head(&locator.realm, head_config)
        .await
        .map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "failed to compose effective config for realm '{}': {err}",
                    locator.realm
                ),
            )
        })?;
    config.apply_env_overrides()?;
    config
        .validate(meerkat_models::canonical())
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
            RealmBackend::Memory => realm_paths.root.clone(),
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
        .schedule(config.tools.schedule_enabled)
        .memory(true);
    if let Some(context_root) = cli.context_root.clone() {
        factory = factory.context_root(context_root);
    }
    if let Some(user_root) = cli.user_config_root.clone() {
        factory = factory.user_config_root(user_root);
    }

    let skill_runtime = factory.build_skill_runtime(&config).await?;

    #[cfg(feature = "live-webrtc")]
    let live_webrtc_enabled = cli.live_webrtc;
    #[cfg(not(feature = "live-webrtc"))]
    let live_webrtc_enabled = false;
    let live_transport_enabled = cli.live_ws.is_some() || live_webrtc_enabled;

    // N74 updated: only build the OpenAI realtime factory when a live
    // transport is configured; otherwise live/* is not exposed and the
    // factory work is wasted. B15 demoted to a wiring/feature preflight:
    // no credential is resolved at startup. Credential truth is owned by
    // the per-open resolving factory, which re-resolves the owning
    // session's auth binding on every open and fails the open closed with
    // its typed error — so explicit-binding-only configs boot cleanly.
    #[cfg(not(feature = "openai-realtime"))]
    let live_session_factory: Option<
        Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>,
    > = if live_transport_enabled {
        return Err(
            "a live transport is configured but this rkat-rpc build does not include the \
             openai-realtime feature"
                .into(),
        );
    } else {
        None
    };
    #[cfg(feature = "openai-realtime")]
    let realtime_agent_factory = factory.clone();

    let config_runtime = Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        realm_paths.root.join("config_state.json"),
    ));
    let runtime = meerkat_rpc::session_runtime::SessionRuntime::new_with_config_store(
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
    // `locator.realm` is already a typed `RealmId`; no string round-trip.
    runtime.set_realm_context(
        Some(locator.realm.clone()),
        cli.instance.clone(),
        Some(manifest.backend.as_str().to_string()),
    );
    runtime.set_config_runtime(config_runtime);

    // Inheritance composition (decision 2/3): attach the per-realm config-document
    // source so the auth-resolution read path composes the active realm's parent
    // chain (workspace head ⊕ home-rooted `global`) into the effective config that
    // `resolve_oauth_target` / `resolve_write_owner` consume. Writes stay raw on
    // `config_runtime` (read/write split). The reserved `global` realm maps to the
    // HOME-rooted doc; when no home is resolvable, fall back to a path under the
    // state root that will not exist, so `global` yields `None` (today's behavior).
    runtime.set_realm_config_source(Arc::clone(&realm_config_source));

    // Per-open realtime credential resolution reads the SAME live config
    // truth the session runtime composes for agent builds: the durable head
    // store plus the active realm's parent chain. Every `live/open`
    // re-resolves the owning session's auth binding against this source.
    // The composition itself lives in `meerkat_rpc::live_wiring` so the
    // exact factory the binary ships is covered by unit tests.
    #[cfg(feature = "openai-realtime")]
    let live_session_factory: Option<
        Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>,
    > = if live_transport_enabled {
        Some(
            meerkat_rpc::live_wiring::build_per_open_realtime_session_factory(
                &realtime_agent_factory,
                Arc::clone(&config_store),
                Arc::clone(&realm_config_source),
                locator.realm.clone(),
            ),
        )
    } else {
        None
    };

    let runtime = Arc::new(runtime);

    #[cfg(feature = "mob")]
    {
        let controlling_acceptor = meerkat_mob::ControllingAcceptorConfig::from_mob_host_config(
            &config.mob_host,
            runtime.session_service(),
        )
        .map_err(|detail| std::io::Error::new(std::io::ErrorKind::InvalidInput, detail))?;
        let mob_state = meerkat_rpc::router::compose_rpc_mob_state(
            &runtime,
            &config_store,
            controlling_acceptor,
        );
        runtime.set_mob_state(mob_state);
    }

    let lease = meerkat_store::start_realm_lease_in(
        &locator.state_root,
        locator.realm.as_str(),
        cli.instance.as_deref(),
        "rkat-rpc",
    )
    .await?;
    let (live_host, live_close_feedback, live_status_feedback, live_ws_token_authority) =
        if live_transport_enabled {
            // Wave-3: install the surface projection sink so adapter observations
            // become canonical Meerkat semantic facts (A1-A6, A14). The host is
            // shared by every enabled transport; surfaces stay thin skins.
            let live_authority_sink = std::sync::Arc::new(
                meerkat_rpc::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                    &runtime,
                )),
            );
            let projection_sink: std::sync::Arc<dyn meerkat_live::LiveProjectionSink> =
                live_authority_sink.clone();
            let close_feedback: std::sync::Arc<dyn meerkat_live::LiveChannelCloseFeedback> =
                live_authority_sink.clone();
            let status_feedback: std::sync::Arc<dyn meerkat_live::LiveChannelStatusFeedback> =
                live_authority_sink.clone();
            let token_authority: std::sync::Arc<dyn meerkat_live::LiveWsTokenAuthority> =
                live_authority_sink;
            // G6 (P2): every production live channel runs with the canonical
            // per-tool-call dispatch deadline.
            let live_tool_timeout = cli
                .live_tool_timeout_ms
                .map(std::time::Duration::from_millis)
                .unwrap_or(meerkat_live::DEFAULT_LIVE_TOOL_TIMEOUT);
            (
                Some(std::sync::Arc::new(
                    meerkat_live::LiveAdapterHost::new(projection_sink)
                        .with_tool_timeout(live_tool_timeout),
                )),
                Some(close_feedback),
                Some(status_feedback),
                Some(token_authority),
            )
        } else {
            (None, None, None, None)
        };

    #[cfg(feature = "live-webrtc")]
    let live_webrtc_state = if cli.live_webrtc {
        let Some(host) = live_host.as_ref() else {
            return Err("internal error: live host missing for --live-webrtc".into());
        };
        eprintln!(
            "rkat-rpc live-webrtc signaling enabled over JSON-RPC method {}",
            meerkat_live::LIVE_WEBRTC_ANSWER_METHOD
        );
        let Some(close_feedback) = live_close_feedback.as_ref() else {
            return Err("internal error: live close feedback missing for --live-webrtc".into());
        };
        let Some(status_feedback) = live_status_feedback.as_ref() else {
            return Err("internal error: live status feedback missing for --live-webrtc".into());
        };
        Some(std::sync::Arc::new(meerkat_live::LiveWebrtcState::new(
            std::sync::Arc::clone(host),
            std::sync::Arc::clone(close_feedback),
            std::sync::Arc::clone(status_feedback),
        )))
    } else {
        None
    };

    let live_ws = if let Some(ref live_ws_addr) = cli.live_ws {
        validate_live_ws_bind(live_ws_addr, tcp_bind_policy).map_err(|err| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string())
        })?;
        let listener = tokio::net::TcpListener::bind(live_ws_addr).await?;
        let actual_addr = listener.local_addr()?;
        eprintln!(
            "rkat-rpc live-ws listening on ws://{actual_addr}{path} (advertised scheme: {scheme})",
            path = meerkat_live::LIVE_WS_PATH,
            scheme = cli.live_ws_scheme.as_str(),
        );
        let Some(host) = live_host.as_ref() else {
            return Err("internal error: live host missing for --live-ws".into());
        };
        let Some(close_feedback) = live_close_feedback.as_ref() else {
            return Err("internal error: live close feedback missing for --live-ws".into());
        };
        let Some(status_feedback) = live_status_feedback.as_ref() else {
            return Err("internal error: live status feedback missing for --live-ws".into());
        };
        let Some(token_authority) = live_ws_token_authority.as_ref() else {
            return Err("internal error: live token authority missing for --live-ws".into());
        };
        let ws_state = std::sync::Arc::new(meerkat_live::LiveWsState::new(
            std::sync::Arc::clone(host),
            std::sync::Arc::clone(close_feedback),
            std::sync::Arc::clone(status_feedback),
            std::sync::Arc::clone(token_authority),
        ));
        let ws_state_clone = std::sync::Arc::clone(&ws_state);
        #[cfg(feature = "live-webrtc")]
        let webrtc_state_for_http = live_webrtc_state.clone();
        let handle = tokio::spawn(async move {
            #[cfg(feature = "live-webrtc")]
            if let Some(webrtc_state) = webrtc_state_for_http {
                return meerkat_live::serve_live_ws_and_webrtc_listener(
                    listener,
                    ws_state_clone,
                    webrtc_state,
                )
                .await;
            }
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
    let live_config = if live_transport_enabled {
        Some(meerkat_rpc::LiveConfig {
            ws: live_ws
                .as_ref()
                .map(|(state, addr, _)| meerkat_rpc::LiveWsConfig {
                    state: std::sync::Arc::clone(state),
                    base_url: format!("{live_ws_scheme}://{addr}"),
                }),
            #[cfg(feature = "live-webrtc")]
            webrtc_state: live_webrtc_state.clone(),
            session_factory: live_session_factory.clone(),
        })
    } else {
        None
    };

    let serve_result = if let Some(ref tcp_addr) = cli.tcp {
        eprintln!("rkat-rpc listening on tcp://{tcp_addr}");
        meerkat_rpc::serve_tcp_with_options(
            tcp_addr,
            runtime,
            config_store,
            skill_runtime,
            live_config,
        )
        .await
    } else {
        meerkat_rpc::serve_stdio_with_options(runtime, config_store, skill_runtime, live_config)
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

/// The ONE bind policy shared by every TCP listener this binary opens
/// (`--tcp` and `--live-ws`): local-only unless `--allow-remote`.
fn resolved_tcp_bind_policy(allow_remote: bool) -> meerkat_rpc::secure_rpc::TcpBindPolicy {
    if allow_remote {
        meerkat_rpc::secure_rpc::TcpBindPolicy::allow_remote()
    } else {
        meerkat_rpc::secure_rpc::TcpBindPolicy::local_only()
    }
}

/// DL7: the live-ws listener obeys the same conservative bind policy as
/// `--tcp`. A non-loopback `--live-ws` without `--allow-remote` is a typed
/// startup error (previously it bound silently).
fn validate_live_ws_bind(
    live_ws_addr: &str,
    policy: meerkat_rpc::secure_rpc::TcpBindPolicy,
) -> Result<(), meerkat_rpc::secure_rpc::TcpBindPolicyError> {
    meerkat_rpc::secure_rpc::validate_tcp_bind_policy("live-ws", live_ws_addr, policy)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// §W4.1 DL7 row: `rkat-rpc --live-ws 0.0.0.0:0` without
    /// `--allow-remote` is a typed startup error — driven through the SAME
    /// policy-derivation + validation pair `async_main` wires.
    #[test]
    fn live_ws_wildcard_bind_without_allow_remote_is_a_typed_startup_reject() {
        let err = validate_live_ws_bind("0.0.0.0:0", resolved_tcp_bind_policy(false))
            .expect_err("wildcard live-ws bind must be refused without --allow-remote");
        assert!(
            err.to_string().contains("live-ws"),
            "the typed error names the offending surface: {err}"
        );
    }

    #[test]
    fn live_ws_bind_policy_admits_loopback_and_explicit_remote() {
        validate_live_ws_bind("127.0.0.1:0", resolved_tcp_bind_policy(false))
            .expect("loopback live-ws bind is always admissible");
        validate_live_ws_bind("0.0.0.0:0", resolved_tcp_bind_policy(true))
            .expect("--allow-remote admits the wildcard live-ws bind");
    }
}
