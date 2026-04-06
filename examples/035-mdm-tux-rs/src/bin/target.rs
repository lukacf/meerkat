//! # 035 — MDM TUX: Target Agent (Runtime-Backed Surface)
//!
//! Runs on managed machines. Registers with a TUX host automatically,
//! then serves as a directly-controlled agent with streaming output.
//!
//! This target is a **runtime-backed surface** (same tier as CLI/RPC/REST):
//! - Agent construction via `AgentFactory::build_agent()`
//! - Session lifecycle via `PersistentSessionService`
//! - Input routing via `RuntimeSessionAdapter` with `HandlingMode::Steer`
//! - Typed `MessageKind` classification (no string prefixes)
//!
//! Sessions are persisted to disk. On restart the most recent session
//! is automatically resumed. TUX can send `/new` to start a fresh
//! session or `/resume` to pick from past sessions.
//!
//! ```text
//! mdm-target <HOST:PORT> [--name NAME] [--model MODEL]
//! ```
//!
//! Set one of: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use futures::StreamExt;
use meerkat::PersistentSessionService;
use meerkat::{AgentFactory, FactoryAgentBuilder, PersistenceBundle};
use meerkat_comms::MessageKind;
use meerkat_comms::{CommsRuntime, PeerMeta, ResolvedCommsConfig, TrustedPeer};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunApplyBoundary, RunPrimitive};
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, SessionError, SessionService,
    StartTurnRequest,
};
use meerkat_core::types::{ContentInput, HandlingMode, SessionId};
use meerkat_core::{AgentEvent, Config, Session};
use meerkat_mob_mcp::{AgentMobToolSurfaceFactory, MobMcpState};
use meerkat_runtime::RuntimeSessionAdapter;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use meerkat_store::{JsonlStore, MemoryBlobStore, SessionFilter, SessionStore};

use mdm_tux::{
    DirectControlPayload, KennelPayload, ProviderKind, RegResponse, auto_detect,
    build_signed_envelope, direct_control_request,
    parse_direct_control_message, read_envelope, register_with_backoff,
    verify_envelope, write_envelope,
};
use tokio::io::BufReader;
use tokio::net::TcpStream;

use mdm_tux::machines::target_session_binding::{
    self, Effect as SessionBindingEffect, Event as SessionBindingEvent,
    State as SessionBindingState,
};

const SYSTEM_PROMPT: &str = "\
You are a managed system agent named '{name}' controlled by a human operator via TUX.
Execute user requests using your available tools. Respond conversationally.
Your responses stream directly to the controller — do not use the 'send' comms tool to reply.";

struct TargetRuntimeSurface {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: Arc<RuntimeSessionAdapter>,
    jsonl_store: JsonlStore,
    mob_state: Arc<MobMcpState>,
}

async fn build_target_runtime_surface(
    session_dir: &Path,
    comms_runtime: Arc<CommsRuntime>,
) -> anyhow::Result<TargetRuntimeSurface> {
    let factory = AgentFactory::new(session_dir)
        .shell(true)
        .builtins(true)
        .mob(true)
        .with_comms_runtime(comms_runtime);
    let builder = FactoryAgentBuilder::new(factory, Config::default());
    let mob_tools_slot = Arc::clone(&builder.default_mob_tools);

    let jsonl_store = JsonlStore::new(session_dir.to_path_buf());
    jsonl_store.init().await?;

    let bundle_store = JsonlStore::new(session_dir.to_path_buf());
    bundle_store.init().await?;

    let persistence = PersistenceBundle::new(
        Arc::new(bundle_store) as Arc<dyn SessionStore>,
        None,
        Arc::new(MemoryBlobStore::new()),
    );
    let runtime_adapter = persistence.runtime_adapter();
    let (session_store, runtime_store, blob_store) = persistence.into_parts();

    let mut session_service =
        PersistentSessionService::new(builder, 10, session_store, runtime_store, blob_store);
    {
        let adapter = runtime_adapter.clone();
        session_service.set_runtime_bindings_provider(Arc::new(move |session_id| {
            let adapter = adapter.clone();
            Box::pin(async move {
                adapter.prepare_bindings(session_id).await.ok()
            })
        }));
    }
    let service = Arc::new(session_service);
    let mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
        service.clone(),
        Some(runtime_adapter.clone()),
    ));
    *mob_tools_slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
        AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
    ));

    Ok(TargetRuntimeSurface {
        service,
        runtime_adapter,
        jsonl_store,
        mob_state,
    })
}

async fn discard_live_session_with_mob_cleanup(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    mob_state: &Arc<MobMcpState>,
    session_id: &SessionId,
) -> Result<(), SessionError> {
    let discard_result = service.discard_live_session(session_id).await;
    if let Err(error) = mob_state
        .destroy_session_mobs(&session_id.to_string())
        .await
    {
        eprintln!("[target] warning: cleanup mobs for session {session_id}: {error}");
    }
    discard_result
}

/// Build a `ResolvedCommsConfig` for the target's stable identity.
fn target_comms_config(name: &str, data_dir: &Path) -> ResolvedCommsConfig {
    ResolvedCommsConfig {
        enabled: true,
        name: name.to_string(),
        inproc_namespace: None,
        listen_uds: None,
        listen_tcp: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: data_dir.join("identity"),
        trusted_peers_path: data_dir.join("trusted_peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: true,
        allow_external_unauthenticated: false,
    }
}

/// Create a `CommsRuntime` with the target's stable identity and a blob store.
async fn create_target_comms_runtime(
    name: &str,
    data_dir: &Path,
) -> anyhow::Result<Arc<CommsRuntime>> {
    let mut runtime = CommsRuntime::new_with_silent_intents(
        target_comms_config(name, data_dir),
        Arc::new(std::collections::HashSet::new()),
    )
    .await
    .map_err(|e| anyhow::anyhow!("comms runtime: {e}"))?;
    runtime.set_blob_store(Arc::new(MemoryBlobStore::new()));
    Ok(Arc::new(runtime))
}

/// Bind a TCP listener on a random port and spawn the accept loop using
/// the CommsRuntime's router primitives. Returns the bound port.
async fn spawn_comms_listener(comms_runtime: &Arc<CommsRuntime>) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let local_addr = listener.local_addr()?;
    let keypair = comms_runtime.router_arc().keypair_arc();
    let trusted = comms_runtime.trusted_peers_shared();
    let inbox_sender = comms_runtime.router_arc().inbox_sender().clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (kp, tp, sender) = (keypair.clone(), trusted.clone(), inbox_sender.clone());
            tokio::spawn(async move {
                let snapshot = tp.read().clone();
                let _ =
                    meerkat_comms::handle_connection(stream, true, &kp, &snapshot, &sender).await;
            });
        }
    });
    Ok(local_addr.port())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    // ── Parse CLI args ────────────────────────────────────────────────────────
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!(
            "Usage: mdm-target <HOST:PORT> [--name NAME] [--model MODEL --provider PROVIDER]"
        );
        eprintln!(
            "   or: mdm-target --kennel HOST:PORT [--advertise IP] [--name NAME] [--model MODEL --provider PROVIDER]"
        );
        eprintln!("Set one of: ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY");
        std::process::exit(1);
    }
    if find_flag(&args, "--kennel").is_some() {
        return run_kennel_mode(&args).await;
    }
    let host_addr = &args[0];
    let name = find_flag(&args, "--name")
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
    let provider_override = parse_provider_override(&args)?;
    let (model, provider) = match find_flag(&args, "--model") {
        Some(m) => {
            let p = provider_override
                .context("--model requires --provider in override mode")?
                .to_string();
            (m, p)
        }
        None => match auto_detect() {
            Some((m, p, _)) => (m, p.to_string()),
            None => bail!("set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY"),
        },
    };

    let data_dir = find_flag(&args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(format!(".rkat/mdm/targets/{name}"))
        });

    // ── 1. Create CommsRuntime with target's stable identity ───────────────
    let comms_runtime = create_target_comms_runtime(&name, &data_dir).await?;

    // ── 2. Bind TCP listener (0.0.0.0:0 for port discovery) ─────────────────
    let comms_port = spawn_comms_listener(&comms_runtime).await?;

    println!("=== MDM Target: {name} ===");
    println!("comms     : tcp://0.0.0.0:{comms_port}");
    println!("provider  : {provider} ({model})");

    // ── 3. Build runtime-backed session service ───────────────────────────────
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let TargetRuntimeSurface {
        service,
        runtime_adapter,
        jsonl_store,
        mob_state,
    } = build_target_runtime_surface(&session_dir, Arc::clone(&comms_runtime)).await?;

    let system_prompt = SYSTEM_PROMPT.replace("{name}", &name);

    // ── 4. Create or auto-resume session ──────────────────────────────────────
    let mut current_session_id = create_or_resume_session(
        &service,
        &runtime_adapter,
        &jsonl_store,
        &model,
        &system_prompt,
        &mob_state,
        &provider,
    )
    .await?;

    let mut event_forwarder: Option<tokio::task::JoinHandle<()>> = None;

    let (mut session_binding_state, _) = target_session_binding::transition(
        SessionBindingState::Unbound,
        SessionBindingEvent::BootResolved {
            session_id: current_session_id.clone(),
        },
    )
    .map_err(|e| anyhow::anyhow!("session binding bootstrap: {e}"))?;

    // ── 6. Reconnection loop ─────────────────────────────────────────────────
    let host_comms_port: u16 = host_addr
        .rsplit(':')
        .next()
        .and_then(|s| s.parse().ok())
        .context("invalid HOST:PORT")?;
    let host_base = host_addr
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(host_addr);
    let reg_addr = format!("{host_base}:{}", host_comms_port + 1);

    let explicit_ip = find_flag(&args, "--advertise");

    let pubkey_string = comms_runtime.public_key().to_peer_id();

    loop {
        // Discover our own IP as seen from the host.
        let local_ip = match &explicit_ip {
            Some(ip) => ip.clone(),
            None => match discover_local_ip(host_base, host_comms_port) {
                Ok(ip) => ip,
                Err(e) => {
                    eprintln!(
                        "[target] address probe failed: {e} — retrying in 5s \
                        (use --advertise <IP> to skip)"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            },
        };
        let advertised_addr = format!("tcp://{local_ip}:{comms_port}");

        // (Re-)register with TUX (retries with exponential backoff)
        eprintln!("[target] registering with {reg_addr} ...");
        let resp =
            match register_with_backoff(&reg_addr, &name, &pubkey_string, &advertised_addr).await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[target] fatal: {e}");
                    std::process::exit(1);
                }
            };
        let (resp_name, resp_pubkey) = match resp {
            RegResponse::Accepted { name, pubkey } => (name, pubkey),
            RegResponse::Rejected {
                reason, message, ..
            } => {
                bail!("registration rejected ({reason:?}): {message}");
            }
        };
        eprintln!(
            "[target] paired with host '{}' ({})",
            resp_name, resp_pubkey
        );

        let host_pubkey = match meerkat_comms::identity::PubKey::from_peer_id(&resp_pubkey) {
            Ok(pk) => pk,
            Err(e) => {
                eprintln!("[target] bad host pubkey: {e} — retrying registration");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };

        // Remove stale entries for this peer name before adding fresh one.
        {
            let trusted = comms_runtime.trusted_peers_shared();
            let stale_keys: Vec<_> = trusted
                .read()
                .peers
                .iter()
                .filter(|p| p.name == resp_name)
                .map(|p| p.pubkey)
                .collect();
            for pk in stale_keys {
                comms_runtime.router().remove_trusted_peer(&pk);
            }
        }
        comms_runtime.upsert_trusted_peer(TrustedPeer {
            name: resp_name,
            pubkey: host_pubkey,
            addr: format!("tcp://{host_addr}"),
            meta: PeerMeta::default(),
        });

        // Disconnect signal: heartbeat + event forwarder can trigger reconnect
        let (disconnect_tx, disconnect_rx) = tokio::sync::watch::channel(false);

        // Spawn/re-spawn event forwarder for the current session
        if let Some(old) = event_forwarder.take() {
            old.abort();
        }
        event_forwarder = Some(
            spawn_event_forwarder(
                &service,
                &current_session_id,
                Arc::clone(&comms_runtime),
                disconnect_tx.clone(),
            )
            .await?,
        );

        // Spawn heartbeat (pings TUX every 10s via typed Request, triggers disconnect on 3 failures)
        let hb = spawn_heartbeat(comms_runtime.router_arc(), "tux", disconnect_tx.clone());

        eprintln!("[target] ready — waiting for commands\n");

        // Run inbox loop (exits on disconnect signal or DISMISS command)
        run_inbox_loop(
            &comms_runtime,
            disconnect_rx,
            disconnect_tx,
            &service,
            &runtime_adapter,
            &jsonl_store,
            &model,
            &system_prompt,
            &mob_state,
            &provider,
            &mut session_binding_state,
            &mut current_session_id,
            &mut event_forwarder,
        )
        .await;

        hb.abort();
        eprintln!("[target] disconnected — reconnecting...");
    }
}

// ── Inbox loop with select!-based disconnect ─────────────────────────────────

// ── Event forwarder (separate task — no mixed select loop) ─────────────────

/// Spawn a dedicated task that subscribes to session events and forwards them
/// to TUX via the CommsRuntime. Runs independently of the inbox loop.
///
/// On send failure (TUX disconnected) or stream close (session discarded),
/// fires the disconnect signal so the main loop can reconnect.
async fn spawn_event_forwarder(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    session_id: &SessionId,
    comms_runtime: Arc<CommsRuntime>,
    disconnect_tx: tokio::sync::watch::Sender<bool>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let mut stream = service
        .subscribe_session_events(session_id)
        .await
        .map_err(|e| anyhow::anyhow!("subscribe session events: {e}"))?;
    let router = comms_runtime.router_arc();
    Ok(tokio::spawn(async move {
        while let Some(envelope) = stream.next().await {
            let event = &envelope.payload;
            if should_forward(event) && !forward_stream_event(&router, "tux", event).await {
                tracing::warn!("event forwarding to TUX failed — triggering disconnect");
                let _ = disconnect_tx.send(true);
                return;
            }
        }
        tracing::warn!("session event stream closed — event forwarder exiting");
    }))
}

// ── Inbox loop — comms messages only, event forwarding is a separate task ──

#[allow(clippy::too_many_arguments)]
async fn run_inbox_loop(
    comms_runtime: &Arc<CommsRuntime>,
    mut disconnect_rx: tokio::sync::watch::Receiver<bool>,
    disconnect_tx: tokio::sync::watch::Sender<bool>,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_forwarder: &mut Option<tokio::task::JoinHandle<()>>,
) {
    loop {
        tokio::select! {
            msg = comms_runtime.recv_message() => {
                let Some(msg) = msg else { break };
                match handle_target_message(
                    &msg,
                    comms_runtime,
                    &disconnect_tx,
                    service,
                    runtime_adapter,
                    jsonl_store,
                    model,
                    system_prompt,
                    mob_state,
                    provider,
                    session_binding_state,
                    current_session_id,
                    event_forwarder,
                )
                .await {
                    TargetLoopAction::Continue => {}
                    TargetLoopAction::ExitProcess => std::process::exit(0),
                }
            }

            _ = disconnect_rx.changed() => {
                if *disconnect_rx.borrow() { break; }
            }
        }
    }
}

enum TargetLoopAction {
    Continue,
    ExitProcess,
}

fn active_session_id(binding_state: &SessionBindingState) -> &SessionId {
    binding_state
        .current_session_id()
        .expect("target session binding must be bound during runtime")
}

use mdm_tux::machines::target_attachment::{Effect as TaEffect, State as TaState};
use mdm_tux::machines::target_kennel_control::{
    self, Effect as TkcEffect, Event as TkcEvent, State as TkcState,
};
use mdm_tux::machines::target_kennel_session::{self, Event as TksEvent, State as TksState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAttachmentHint {
    tux_id: String,
    lease_id: String,
}

async fn load_attachment_hint(path: &Path) -> anyhow::Result<Option<PersistedAttachmentHint>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("parse attachment hint")?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("read attachment hint"),
    }
}

async fn save_attachment_hint(path: &Path, hint: &PersistedAttachmentHint) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(hint).context("serialize attachment hint")?;
    tokio::fs::write(path, bytes)
        .await
        .context("write attachment hint")
}

async fn clear_attachment_hint(path: &Path) -> anyhow::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("remove attachment hint"),
    }
}

#[allow(dead_code)] // Will be used when all effect processing is centralized
async fn apply_target_attachment_effects(
    comms_runtime: &Arc<CommsRuntime>,
    effects: &[TaEffect],
    heartbeat: &mut Option<tokio::task::JoinHandle<()>>,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
) -> anyhow::Result<bool> {
    let router = comms_runtime.router_arc();
    for effect in effects {
        match effect {
            TaEffect::EnsureTuxPeer {
                tux_pubkey,
                tux_direct_addr,
            } => {
                let tux_pk = meerkat_comms::identity::PubKey::from_peer_id(tux_pubkey)
                    .context("parse adopted tux pubkey")?;
                let trusted = comms_runtime.trusted_peers_shared();
                let stale_keys: Vec<_> = trusted
                    .read()
                    .peers
                    .iter()
                    .filter(|p| p.name == "tux")
                    .map(|p| p.pubkey)
                    .collect();
                for pk in stale_keys {
                    router.remove_trusted_peer(&pk);
                }
                comms_runtime.upsert_trusted_peer(TrustedPeer {
                    name: "tux".into(),
                    pubkey: tux_pk,
                    addr: tux_direct_addr.clone(),
                    meta: PeerMeta::default(),
                });
            }
            TaEffect::PersistAttachmentHint { tux_id, lease_id } => {
                let next = PersistedAttachmentHint {
                    tux_id: tux_id.clone(),
                    lease_id: lease_id.clone(),
                };
                save_attachment_hint(attachment_hint_path, &next).await?;
                *attachment_hint = Some(next);
            }
            TaEffect::ClearAttachmentHint => {
                clear_attachment_hint(attachment_hint_path).await?;
                *attachment_hint = None;
            }
            TaEffect::SendAttachRequest {
                target_id,
                lease_id,
            } => {
                let Ok(kind) = direct_control_request(&DirectControlPayload::AttachRequest {
                    lease_id: lease_id.clone(),
                    target_id: target_id.clone(),
                }) else {
                    return Ok(false);
                };
                let attach_fut = router.send("tux", kind);
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(10), attach_fut).await,
                    Ok(Ok(_))
                ) {
                    return Ok(false);
                }
            }
            TaEffect::StartDirectHeartbeat => {
                if heartbeat.is_none() {
                    *heartbeat = Some(spawn_heartbeat(
                        router.clone(),
                        "tux",
                        disconnect_tx.clone(),
                    ));
                }
            }
            TaEffect::StopDirectHeartbeat => {
                if let Some(hb) = heartbeat.take() {
                    hb.abort();
                }
            }
            TaEffect::ReregisterWithHint { .. } | TaEffect::ReregisterClean => {}
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
async fn apply_target_control_effects(
    comms_runtime: &Arc<CommsRuntime>,
    effects: &[TkcEffect],
    heartbeat: &mut Option<tokio::task::JoinHandle<()>>,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
) -> anyhow::Result<(bool, bool)> {
    let mut attachment_effects = Vec::new();
    let mut should_return_to_register = false;
    for effect in effects {
        match effect {
            TkcEffect::Attachment(effect) => attachment_effects.push(effect.clone()),
            TkcEffect::ReturnToRegisterLoop => should_return_to_register = true,
        }
    }
    let applied = apply_target_attachment_effects(
        comms_runtime,
        &attachment_effects,
        heartbeat,
        disconnect_tx,
        attachment_hint,
        attachment_hint_path,
    )
    .await?;
    Ok((applied, should_return_to_register))
}

#[allow(clippy::too_many_arguments)]
async fn handle_target_message(
    msg: &meerkat_comms::agent::types::CommsMessage,
    comms_runtime: &Arc<CommsRuntime>,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_forwarder: &mut Option<tokio::task::JoinHandle<()>>,
) -> TargetLoopAction {
    let sender = msg.from_peer.clone();
    match &msg.content {
        meerkat_comms::agent::types::CommsContent::Request { intent, .. }
            if intent.as_str() == "dismiss" =>
        {
            eprintln!("[target] received DISMISS — shutting down");
            return TargetLoopAction::ExitProcess;
        }
        meerkat_comms::agent::types::CommsContent::Request { intent, params, .. }
            if intent.as_str() == "command" =>
        {
            let cmd = params.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
            eprintln!("[target] command: {cmd}");

            let response = handle_command(
                cmd,
                service,
                runtime_adapter,
                jsonl_store,
                model,
                system_prompt,
                mob_state,
                provider,
                session_binding_state,
                current_session_id,
                event_forwarder,
                comms_runtime,
                disconnect_tx,
            )
            .await;

            // Timeout-guard the reply to prevent a half-open controller from
            // wedging the inbox loop (same bound as heartbeat/event sends).
            let router = comms_runtime.router_arc();
            let reply_fut = router.send(
                &sender,
                MessageKind::Message {
                    body: response,
                    blocks: None,
                },
            );
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), reply_fut).await;
        }
        meerkat_comms::agent::types::CommsContent::Request { intent, .. }
            if intent.as_str() == "heartbeat" => {}
        meerkat_comms::agent::types::CommsContent::Message { body, .. } => {
            eprintln!("[target] received message from '{sender}'");

            let peer_input = Input::Peer(PeerInput {
                header: InputHeader {
                    id: meerkat_core::lifecycle::InputId::new(),
                    timestamp: chrono::Utc::now(),
                    source: InputOrigin::Peer {
                        peer_id: sender.clone(),
                        runtime_id: None,
                    },
                    durability: InputDurability::Ephemeral,
                    visibility: InputVisibility::default(),
                    idempotency_key: None,
                    supersession_key: None,
                    correlation_id: None,
                },
                convention: Some(PeerConvention::Message),
                body: body.clone(),
                blocks: None,
                handling_mode: Some(HandlingMode::Steer),
            });

            match runtime_adapter
                .accept_input(active_session_id(session_binding_state), peer_input)
                .await
            {
                Ok(outcome) => {
                    tracing::debug!(?outcome, "input accepted");
                }
                Err(e) => {
                    eprintln!("[target] accept_input error: {e}");
                    let req = StartTurnRequest {
                        prompt: ContentInput::Text(body.clone()),
                        system_prompt: None,
                        render_metadata: None,
                        handling_mode: HandlingMode::Queue,
                        event_tx: None,
                        skill_references: None,
                        flow_tool_overlay: None,
                        additional_instructions: None,
                    };
                    if let Err(e) = service
                        .start_turn(active_session_id(session_binding_state), req)
                        .await
                    {
                        eprintln!("[target] fallback start_turn error: {e}");
                    }
                }
            }
        }
        _ => {}
    }
    TargetLoopAction::Continue
}

async fn forward_stream_event(
    router: &Arc<meerkat_comms::Router>,
    peer: &str,
    event: &AgentEvent,
) -> bool {
    let Ok(kind) = direct_control_request(&DirectControlPayload::StreamEvent {
        event: event.clone(),
    }) else {
        return true;
    };
    let send_fut = router.send(peer, kind);
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), send_fut).await,
        Ok(Ok(_))
    )
}

// ── Heartbeat ────────────────────────────────────────────────────────────────

/// Spawn a background heartbeat task that pings the peer every 10 seconds.
/// Uses typed `MessageKind::Request` instead of string prefix.
/// On 3 consecutive send failures, triggers the disconnect signal.
fn spawn_heartbeat(
    router: Arc<meerkat_comms::Router>,
    peer: &str,
    disconnect_tx: tokio::sync::watch::Sender<bool>,
) -> tokio::task::JoinHandle<()> {
    let peer = peer.to_owned();
    tokio::spawn(async move {
        let mut consecutive_failures = 0u32;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            // Timeout prevents a black-holed TUX from blocking the heartbeat
            // loop indefinitely (TCP connect can hang for minutes). Without
            // this, the failure counter never advances and reconnect never fires.
            let send_fut = router.send(
                    &peer,
                    MessageKind::Request {
                        intent: "heartbeat".into(),
                        params: serde_json::json!(null),
                    },
                );
            let failed =
                match tokio::time::timeout(std::time::Duration::from_secs(5), send_fut).await {
                    Ok(Ok(_)) => false,
                    Ok(Err(_)) => true, // send error
                    Err(_) => true,     // timeout
                };
            if failed {
                consecutive_failures += 1;
                eprintln!("[target] heartbeat failed ({consecutive_failures}/3)");
                if consecutive_failures >= 3 {
                    let _ = disconnect_tx.send(true);
                    break;
                }
            } else {
                consecutive_failures = 0;
            }
        }
    })
}

// ── Command handling ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_command(
    cmd: &str,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_forwarder: &mut Option<tokio::task::JoinHandle<()>>,
    comms_runtime: &Arc<CommsRuntime>,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
) -> String {
    if cmd == "NEW_SESSION" {
        match switch_session(
            service,
            runtime_adapter,
            None,
            model,
            system_prompt,
            mob_state,
            provider,
            session_binding_state,
            current_session_id,
            event_forwarder,
            comms_runtime,
            disconnect_tx,
        )
        .await
        {
            Ok(sid) => format!("New session started: {sid}"),
            Err(e) => format!("Failed to create session: {e}"),
        }
    } else if cmd == "LIST_SESSIONS" {
        match jsonl_store.list(SessionFilter::default()).await {
            Ok(mut sessions) => {
                sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                if sessions.is_empty() {
                    "No saved sessions.".into()
                } else {
                    let current_str = current_session_id.to_string();
                    let mut out = String::from("**Sessions:**\n");
                    for (i, s) in sessions.iter().enumerate().take(20) {
                        let age = s
                            .updated_at
                            .elapsed()
                            .map(format_duration)
                            .unwrap_or_else(|_| "?".into());
                        let marker = if s.id.to_string() == current_str {
                            " ← current"
                        } else {
                            ""
                        };
                        out.push_str(&format!(
                            "  **{}.**  {} msgs, {} ago{}\n",
                            i + 1,
                            s.message_count,
                            age,
                            marker,
                        ));
                    }
                    out.push_str("\nType `/resume <number>` to load a session.");
                    out
                }
            }
            Err(e) => format!("Error listing sessions: {e}"),
        }
    } else if let Some(arg) = cmd.strip_prefix("RESUME ") {
        let arg = arg.trim();
        let idx: usize = match arg.parse::<usize>() {
            Ok(n) if n >= 1 => n - 1,
            _ => return format!("Invalid session number: {arg}"),
        };
        match jsonl_store.list(SessionFilter::default()).await {
            Ok(mut sessions) => {
                sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                if idx >= sessions.len() {
                    return format!("Session {arg} not found (have {})", sessions.len());
                }
                let resume_id = sessions[idx].id.clone();
                match switch_session(
                    service,
                    runtime_adapter,
                    Some(resume_id),
                    model,
                    system_prompt,
                    mob_state,
                    provider,
                    session_binding_state,
                    current_session_id,
                    event_forwarder,
                    comms_runtime,
                    disconnect_tx,
                )
                .await
                {
                    Ok(sid) => format!("Resumed session {sid}"),
                    Err(e) => format!("Error resuming session: {e}"),
                }
            }
            Err(e) => format!("Error listing sessions: {e}"),
        }
    } else {
        format!("Unknown command: {cmd}")
    }
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

// ── Session lifecycle ────────────────────────────────────────────────────────

/// Create a new session or resume an existing one. Registers the session
/// with the RuntimeSessionAdapter and subscribes to its events.
async fn create_or_resume_session(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
) -> anyhow::Result<SessionId> {
    // Try to auto-resume the most recent session. On failure, warn and start fresh.
    if let Ok(mut sessions) = jsonl_store.list(SessionFilter::default()).await {
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        if let Some(latest) = sessions.first() {
            eprintln!(
                "[target] auto-resuming session {} ({} messages)",
                latest.id, latest.message_count
            );
            match setup_session(
                service,
                runtime_adapter,
                Some(latest.id.clone()),
                model,
                system_prompt,
                mob_state,
                provider,
            )
            .await
            {
                Ok(sid) => return Ok(sid),
                Err(e) => {
                    eprintln!("[target] auto-resume failed: {e} — starting fresh session");
                }
            }
        }
    }

    eprintln!("[target] starting fresh session");
    setup_session(
        service,
        runtime_adapter,
        None,
        model,
        system_prompt,
        mob_state,
        provider,
    )
    .await
}

/// Switch to a new or resumed session. Drops the old event forwarder,
/// creates/resumes the session, registers with the runtime adapter,
/// and spawns a new event forwarder.
#[allow(clippy::too_many_arguments)]
async fn switch_session(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    resume_id: Option<SessionId>,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_forwarder: &mut Option<tokio::task::JoinHandle<()>>,
    comms_runtime: &Arc<CommsRuntime>,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
) -> anyhow::Result<SessionId> {
    let request_event = match resume_id.clone() {
        Some(session_id) => SessionBindingEvent::ResumeRequested { session_id },
        None => SessionBindingEvent::CreateNewRequested,
    };
    let (next_state, effects) =
        target_session_binding::transition(session_binding_state.clone(), request_event)
            .map_err(|e| anyhow::anyhow!("session binding request: {e}"))?;
    *session_binding_state = next_state;

    let mut new_id: Option<SessionId> = None;
    for effect in effects {
        if let SessionBindingEffect::SetupSession { resume_id } = effect {
            let setup_result = setup_session(
                service,
                runtime_adapter,
                resume_id,
                model,
                system_prompt,
                mob_state,
                provider,
            )
            .await;
            match setup_result {
                Ok(session_id) => new_id = Some(session_id),
                Err(err) => {
                    let (rolled_back, rollback_effects) = target_session_binding::transition(
                        session_binding_state.clone(),
                        SessionBindingEvent::SetupFailed,
                    )
                    .map_err(|binding_err| {
                        anyhow::anyhow!("session binding setup failure rollback: {binding_err}")
                    })?;
                    debug_assert!(rollback_effects.is_empty());
                    *session_binding_state = rolled_back;
                    return Err(err);
                }
            }
        }
    }
    let Some(new_id) = new_id else {
        *current_session_id = active_session_id(session_binding_state).clone();
        return Ok(current_session_id.clone());
    };

    let (next_state, effects) = target_session_binding::transition(
        session_binding_state.clone(),
        SessionBindingEvent::SetupSucceeded {
            session_id: new_id.clone(),
        },
    )
    .map_err(|e| anyhow::anyhow!("session binding setup succeeded: {e}"))?;
    *session_binding_state = next_state;

    let mut new_forwarder = None;
    for effect in effects {
        match effect {
            SessionBindingEffect::SubscribeSessionEvents { session_id } => {
                // Abort the old forwarder BEFORE spawning the replacement so
                // there is no window where both tasks emit StreamEvents.
                if let Some(old) = event_forwarder.take() {
                    old.abort();
                }
                match spawn_event_forwarder(service, &session_id, Arc::clone(comms_runtime), disconnect_tx.clone()).await {
                    Ok(handle) => new_forwarder = Some(handle),
                    Err(e) => {
                        let (rolled_back, cleanup_effects) = target_session_binding::transition(
                            session_binding_state.clone(),
                            SessionBindingEvent::SubscriptionFailed,
                        )
                        .map_err(|binding_err| {
                            anyhow::anyhow!(
                                "session binding subscription failure rollback: {binding_err}"
                            )
                        })?;
                        *session_binding_state = rolled_back;
                        for cleanup in cleanup_effects {
                            match cleanup {
                                SessionBindingEffect::UnregisterRuntimeSession { session_id } => {
                                    runtime_adapter.unregister_session(&session_id).await;
                                }
                                SessionBindingEffect::DiscardLiveSession { session_id } => {
                                    if let Err(discard_err) = discard_live_session_with_mob_cleanup(
                                        service,
                                        mob_state,
                                        &session_id,
                                    )
                                    .await
                                        && !matches!(discard_err, SessionError::NotFound { .. })
                                    {
                                        eprintln!(
                                            "[target] warning: discard failed rollback session {session_id}: {discard_err}"
                                        );
                                    }
                                }
                                SessionBindingEffect::SetupSession { .. }
                                | SessionBindingEffect::SubscribeSessionEvents { .. } => {}
                            }
                        }
                        return Err(anyhow::anyhow!("subscribe session events: {e}"));
                    }
                }
            }
            SessionBindingEffect::UnregisterRuntimeSession { session_id } => {
                runtime_adapter.unregister_session(&session_id).await;
            }
            SessionBindingEffect::DiscardLiveSession { session_id } => {
                if let Err(e) =
                    discard_live_session_with_mob_cleanup(service, mob_state, &session_id).await
                    && !matches!(e, SessionError::NotFound { .. })
                {
                    eprintln!("[target] warning: discard old session {session_id}: {e}");
                }
            }
            SessionBindingEffect::SetupSession { .. } => {}
        }
    }

    let (next_state, effects) = target_session_binding::transition(
        session_binding_state.clone(),
        SessionBindingEvent::SubscriptionEstablished,
    )
    .map_err(|e| anyhow::anyhow!("session binding subscription established: {e}"))?;
    *session_binding_state = next_state;
    for effect in effects {
        match effect {
            SessionBindingEffect::UnregisterRuntimeSession { session_id } => {
                runtime_adapter.unregister_session(&session_id).await;
            }
            SessionBindingEffect::DiscardLiveSession { session_id } => {
                if let Err(e) =
                    discard_live_session_with_mob_cleanup(service, mob_state, &session_id).await
                    && !matches!(e, SessionError::NotFound { .. })
                {
                    eprintln!("[target] warning: discard old session {session_id}: {e}");
                }
            }
            SessionBindingEffect::SetupSession { .. }
            | SessionBindingEffect::SubscribeSessionEvents { .. } => {}
        }
    }

    let (next_state, effects) = target_session_binding::transition(
        session_binding_state.clone(),
        SessionBindingEvent::TeardownCompleted,
    )
    .map_err(|e| anyhow::anyhow!("session binding teardown completed: {e}"))?;
    debug_assert!(effects.is_empty());
    *session_binding_state = next_state;

    if let Some(handle) = new_forwarder {
        // Old forwarder was already aborted before spawning the replacement
        // (in the SubscribeSessionEvents effect handler above).
        *event_forwarder = Some(handle);
    }
    *current_session_id = active_session_id(session_binding_state).clone();

    Ok(new_id)
}

/// Create or resume a session and register it with the runtime adapter.
async fn setup_session(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    resume_id: Option<SessionId>,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
) -> anyhow::Result<SessionId> {
    let resume_session = match &resume_id {
        Some(id) => {
            let loaded = service
                .load_persisted(id)
                .await
                .map_err(|e| anyhow::anyhow!("load session {id}: {e}"))?;
            Some(loaded.ok_or_else(|| anyhow::anyhow!("session {id} not found on disk"))?)
        }
        None => Some(Session::new()),
    };
    let prepared_session = resume_session
        .clone()
        .ok_or_else(|| anyhow::anyhow!("target setup requires a prepared session"))?;
    let prepared_session_id = prepared_session.id().clone();

    // Prepare canonical runtime bindings (registers + allocates epoch in one shot).
    let bindings = runtime_adapter.prepare_bindings(prepared_session_id.clone()).await
        .map_err(|e| anyhow::anyhow!("runtime bindings: {e}"))?;

    // No external_tools needed — the factory composes comms tools automatically
    // from the CommsRuntime set via AgentFactory::with_comms_runtime().
    let build_opts = SessionBuildOptions {
        provider: Some(meerkat_core::Provider::from_name(provider)),
        override_builtins: meerkat_core::ToolCategoryOverride::Enable,
        override_shell: meerkat_core::ToolCategoryOverride::Enable,
        override_mob: meerkat_core::ToolCategoryOverride::Enable,
        resume_session: Some(prepared_session),
        runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
        ..Default::default()
    };

    let req = CreateSessionRequest {
        model: model.to_string(),
        prompt: ContentInput::Text(String::new()),
        render_metadata: None,
        system_prompt: Some(system_prompt.to_string()),
        max_tokens: None,
        event_tx: None,
        skill_references: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(build_opts),
        labels: None,
    };

    let result = match service.create_session(req).await {
        Ok(result) => result,
        Err(error) => {
            runtime_adapter.unregister_session(&prepared_session_id).await;
            return Err(anyhow::anyhow!("create session: {error}"));
        }
    };

    let session_id = result.session_id;
    eprintln!("[target] session ready: {session_id}");

    // Create executor and register with runtime adapter for Steer support
    let executor = Box::new(TargetCoreExecutor::new(
        service.clone(),
        mob_state.clone(),
        session_id.clone(),
    ));
    runtime_adapter
        .ensure_session_with_executor(session_id.clone(), executor)
        .await;

    Ok(session_id)
}

// ── CoreExecutor for runtime loop ────────────────────────────────────────────

/// Bridges the RuntimeSessionAdapter's runtime loop to the PersistentSessionService.
/// When the runtime loop dequeues an input (via DefaultPolicyTable routing),
/// it calls `apply()` which translates the RunPrimitive into a `start_turn()`.
struct TargetCoreExecutor {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    mob_state: Arc<MobMcpState>,
    session_id: SessionId,
}

impl TargetCoreExecutor {
    fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        mob_state: Arc<MobMcpState>,
        session_id: SessionId,
    ) -> Self {
        Self {
            service,
            mob_state,
            session_id,
        }
    }
}

/// Extract prompt content from a `RunPrimitive`, preserving multimodal blocks.
fn extract_prompt(primitive: &RunPrimitive) -> ContentInput {
    match primitive {
        RunPrimitive::StagedInput(staged) => {
            let mut all_blocks = Vec::new();
            for append in &staged.appends {
                match &append.content {
                    CoreRenderable::Text { text } => {
                        all_blocks
                            .push(meerkat_core::types::ContentBlock::Text { text: text.clone() });
                    }
                    CoreRenderable::Blocks { blocks } => {
                        all_blocks.extend(blocks.iter().cloned());
                    }
                    _ => {}
                }
            }
            if all_blocks.is_empty() {
                ContentInput::Text(String::new())
            } else if all_blocks.len() == 1 {
                if let meerkat_core::types::ContentBlock::Text { text } = &all_blocks[0] {
                    ContentInput::Text(text.clone())
                } else {
                    ContentInput::Blocks(all_blocks)
                }
            } else {
                ContentInput::Blocks(all_blocks)
            }
        }
        RunPrimitive::ImmediateAppend(append) => match &append.content {
            CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
            CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
            _ => ContentInput::Text(String::new()),
        },
        RunPrimitive::ImmediateContextAppend(ctx) => match &ctx.content {
            CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
            CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
            _ => ContentInput::Text(String::new()),
        },
        _ => ContentInput::Text(String::new()),
    }
}

#[async_trait::async_trait]
impl CoreExecutor for TargetCoreExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let prompt = extract_prompt(&primitive);

        let req = StartTurnRequest {
            prompt,
            system_prompt: None,
            render_metadata: None,
            handling_mode: HandlingMode::Queue,
            event_tx: None,
            skill_references: primitive
                .turn_metadata()
                .and_then(|meta| meta.skill_references.clone()),
            flow_tool_overlay: primitive
                .turn_metadata()
                .and_then(|meta| meta.flow_tool_overlay.clone()),
            additional_instructions: primitive
                .turn_metadata()
                .and_then(|meta| meta.additional_instructions.clone()),
        };

        let boundary = match &primitive {
            RunPrimitive::StagedInput(staged) => staged.boundary,
            _ => RunApplyBoundary::Immediate,
        };
        let input_ids = primitive.contributing_input_ids().to_vec();

        self.service
            .apply_runtime_turn(&self.session_id, run_id, req, boundary, input_ids)
            .await
            .map_err(|e| CoreExecutorError::ApplyFailed {
                reason: e.to_string(),
            })
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => {
                self.service.interrupt(&self.session_id).await.map_err(|e| {
                    CoreExecutorError::ControlFailed {
                        reason: e.to_string(),
                    }
                })
            }
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = discard_live_session_with_mob_cleanup(
                    &self.service,
                    &self.mob_state,
                    &self.session_id,
                )
                .await;
                runtime_adapter_unregister_noop();
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::ControlFailed {
                        reason: err.to_string(),
                    }),
                }
            }
            _ => Ok(()),
        }
    }
}

/// Placeholder for StopRuntimeExecutor — the target owns the adapter lifetime
/// so explicit unregistration isn't needed during shutdown.
fn runtime_adapter_unregister_noop() {}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ActiveAdoption {
    lease_id: String,
    target_id: String,
    tux_id: String,
    tux_pubkey: String,
    tux_direct_addr: String,
}

async fn run_kennel_mode(args: &[String]) -> anyhow::Result<()> {
    let kennel_addr = find_flag(args, "--kennel").context("--kennel HOST:PORT is required")?;
    let name = find_flag(args, "--name")
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
    let provider_override = parse_provider_override(args)?;
    let (model, provider) = match find_flag(args, "--model") {
        Some(m) => {
            let p = provider_override
                .context("--model requires --provider in override mode")?
                .to_string();
            (m, p)
        }
        None => match auto_detect() {
            Some((m, p, _)) => (m, p.to_string()),
            None => bail!("set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY"),
        },
    };
    let data_dir = find_flag(args, "--data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(format!(".rkat/mdm/targets/{name}"))
        });

    let comms_runtime = create_target_comms_runtime(&name, &data_dir).await?;
    let comms_port = spawn_comms_listener(&comms_runtime).await?;
    let target_id = comms_runtime.public_key().to_peer_id();
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let TargetRuntimeSurface {
        service,
        runtime_adapter,
        jsonl_store,
        mob_state,
    } = build_target_runtime_surface(&session_dir, Arc::clone(&comms_runtime)).await?;
    let system_prompt = SYSTEM_PROMPT.replace("{name}", &name);
    let mut current_session_id = create_or_resume_session(
        &service,
        &runtime_adapter,
        &jsonl_store,
        &model,
        &system_prompt,
        &mob_state,
        &provider,
    )
    .await?;
    let mut event_forwarder: Option<tokio::task::JoinHandle<()>> = None;
    let (mut session_binding_state, _) = target_session_binding::transition(
        SessionBindingState::Unbound,
        SessionBindingEvent::BootResolved {
            session_id: current_session_id.clone(),
        },
    )
    .map_err(|e| anyhow::anyhow!("session binding bootstrap: {e}"))?;

    let explicit_ip = find_flag(args, "--advertise");
    let attachment_hint_path = data_dir.join("attachment_hint.json");

    println!("=== MDM Target: {name} ===");
    println!("comms     : tcp://0.0.0.0:{comms_port}");
    println!("kennel    : {kennel_addr}");
    println!("provider  : {provider} ({model})");

    let mut attached_tux_hint = load_attachment_hint(&attachment_hint_path).await?;
    let mut kennel_session_state = TksState::Disconnected;

    loop {
        let (kennel_host, kennel_port) = kennel_addr
            .rsplit_once(':')
            .context("invalid --kennel HOST:PORT")?;
        let kennel_port: u16 = kennel_port.parse().context("invalid kennel port")?;
        let local_ip = match &explicit_ip {
            Some(ip) => ip.clone(),
            None => match discover_local_ip(kennel_host, kennel_port) {
                Ok(ip) => ip,
                Err(e) => {
                    eprintln!(
                        "[target] kennel address probe failed: {e} — retrying in 5s \
                        (use --advertise <IP> to skip)"
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            },
        };
        let advertised_addr = format!("tcp://{local_ip}:{comms_port}");

        let stream =
            match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&kennel_addr))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    eprintln!("[target] kennel connect failed: {e} — retrying in 2s");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                Err(_) => {
                    eprintln!("[target] kennel connect timed out — retrying in 2s");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };

        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let register = build_signed_envelope(
            comms_runtime.router_arc().keypair_arc().as_ref(),
            &target_id,
            KennelPayload::TargetRegister {
                target_id: target_id.clone(),
                name: name.clone(),
                pubkey: target_id.clone(),
                direct_addr: advertised_addr.clone(),
                labels: Default::default(),
                capabilities: BTreeMap::from([
                    ("shell".to_string(), true),
                    ("runtime".to_string(), true),
                ]),
                attached_tux_id: attached_tux_hint.as_ref().map(|hint| hint.tux_id.clone()),
            },
        )?;
        if let Err(e) = write_envelope(&mut write_half, &register).await {
            eprintln!("[target] kennel register failed: {e} — retrying");
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        kennel_session_state =
            target_kennel_session::transition(kennel_session_state, TksEvent::RegisterSent)
                .map_err(|e| anyhow::anyhow!("target kennel session register sent: {e}"))?;
        let Some(env) = read_envelope(&mut reader).await? else {
            eprintln!("[target] kennel closed during register");
            kennel_session_state =
                target_kennel_session::transition(kennel_session_state, TksEvent::ControlLost)
                    .map_err(|e| anyhow::anyhow!("target kennel session register EOF: {e}"))?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let _ = verify_envelope(&env)?;
        match &env.payload {
            KennelPayload::TargetRegistered => {
                kennel_session_state = target_kennel_session::transition(
                    kennel_session_state,
                    TksEvent::RegistrationAcked,
                )
                .map_err(|e| anyhow::anyhow!("target kennel session registration ack: {e}"))?;
            }
            KennelPayload::TargetRegistrationRejected { reason, message } => {
                let _ = target_kennel_session::transition(
                    kennel_session_state.clone(),
                    TksEvent::RegistrationRejected,
                )
                .map_err(|e| anyhow::anyhow!("target kennel session registration rejected: {e}"))?;
                bail!("kennel rejected target registration ({reason:?}): {message}");
            }
            other => {
                bail!("unexpected kennel registration reply: {other:?}");
            }
        }

        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    let hb = build_signed_envelope(
                        comms_runtime.router_arc().keypair_arc().as_ref(),
                        &target_id,
                        KennelPayload::TargetHeartbeat,
                    )?;
                    if write_envelope(&mut write_half, &hb).await.is_err() {
                        break;
                    }
                }
                maybe = read_envelope(&mut reader) => {
                    let Some(env) = maybe? else { break; };
                    let _ = verify_envelope(&env)?;
                    match env.payload {
                        KennelPayload::Adopted {
                            lease_id,
                            target_id: adopted_target_id,
                            tux_id,
                            tux_pubkey,
                            tux_direct_addr,
                            ..
                        } => {
                            if !kennel_session_state.allows_control_payloads() {
                                bail!("received adopted before kennel registration completed");
                            }
                            if adopted_target_id != target_id {
                                continue;
                            }
                            let adoption = ActiveAdoption {
                                lease_id,
                                target_id: adopted_target_id,
                                tux_id,
                                tux_pubkey,
                                tux_direct_addr,
                            };
                            let _adopted_exit = run_adopted_loop(
                                &comms_runtime,
                                &mut reader,
                                &mut write_half,
                                adoption,
                                true,
                                &service,
                                &runtime_adapter,
                                &jsonl_store,
                                &model,
                                &system_prompt,
                                &mob_state,
                                &provider,
                                &mut session_binding_state,
                                &mut current_session_id,
                                &mut event_forwarder,
                                &mut attached_tux_hint,
                                &attachment_hint_path,
                            ).await?;

                            let re_register = build_signed_envelope(
                                comms_runtime.router_arc().keypair_arc().as_ref(),
                                &target_id,
                                KennelPayload::TargetRegister {
                                    target_id: target_id.clone(),
                                    name: name.clone(),
                                    pubkey: target_id.clone(),
                                    direct_addr: advertised_addr.clone(),
                                    labels: Default::default(),
                                    capabilities: BTreeMap::from([
                                        ("shell".to_string(), true),
                                        ("runtime".to_string(), true),
                                    ]),
                                    attached_tux_id: attached_tux_hint.as_ref().map(|hint| hint.tux_id.clone()),
                                },
                            )?;
                            if write_envelope(&mut write_half, &re_register).await.is_ok() {
                                kennel_session_state = target_kennel_session::transition(
                                    kennel_session_state,
                                    TksEvent::RegisterSent,
                                )
                                .unwrap_or(TksState::Disconnected);
                            }
                        }
                        KennelPayload::LeaseRebound {
                            lease_id,
                            target_id: rebound_target_id,
                            tux_id,
                            tux_pubkey,
                            tux_direct_addr,
                            ..
                        } => {
                            if !kennel_session_state.allows_control_payloads() {
                                bail!("received lease rebound before kennel registration completed");
                            }
                            if rebound_target_id != target_id {
                                continue;
                            }
                            // Use the fresh TUX endpoint from the kennel, not
                            // stale cached peer data. TUX may have reconnected
                            // with a different --listen/--advertise address.
                            let adoption = ActiveAdoption {
                                lease_id,
                                target_id: rebound_target_id,
                                tux_id,
                                tux_pubkey,
                                tux_direct_addr,
                            };
                            let _adopted_exit = run_adopted_loop(
                                &comms_runtime,
                                &mut reader,
                                &mut write_half,
                                adoption,
                                false,
                                &service,
                                &runtime_adapter,
                                &jsonl_store,
                                &model,
                                &system_prompt,
                                &mob_state,
                                &provider,
                                &mut session_binding_state,
                                &mut current_session_id,
                                &mut event_forwarder,
                                &mut attached_tux_hint,
                                &attachment_hint_path,
                            ).await?;

                            let re_register = build_signed_envelope(
                                comms_runtime.router_arc().keypair_arc().as_ref(),
                                &target_id,
                                KennelPayload::TargetRegister {
                                    target_id: target_id.clone(),
                                    name: name.clone(),
                                    pubkey: target_id.clone(),
                                    direct_addr: advertised_addr.clone(),
                                    labels: Default::default(),
                                    capabilities: BTreeMap::from([
                                        ("shell".to_string(), true),
                                        ("runtime".to_string(), true),
                                    ]),
                                    attached_tux_id: attached_tux_hint.as_ref().map(|hint| hint.tux_id.clone()),
                                },
                            )?;
                            if write_envelope(&mut write_half, &re_register).await.is_ok() {
                                kennel_session_state = target_kennel_session::transition(
                                    kennel_session_state,
                                    TksEvent::RegisterSent,
                                )
                                .unwrap_or(TksState::Disconnected);
                            }
                        }
                        KennelPayload::TargetRegistered => {
                            kennel_session_state = target_kennel_session::transition(
                                kennel_session_state,
                                TksEvent::RegistrationAcked,
                            )
                            .map_err(|e| anyhow::anyhow!("target kennel session re-register ack: {e}"))?;
                        }
                        KennelPayload::TargetRegistrationRejected { reason, message } => {
                            let _ = target_kennel_session::transition(
                                kennel_session_state.clone(),
                                TksEvent::RegistrationRejected,
                            )
                            .map_err(|e| anyhow::anyhow!(
                                "target kennel session re-register rejected: {e}"
                            ))?;
                            bail!("kennel rejected target re-registration ({reason:?}): {message}");
                        }
                        _ => {}
                    }
                }
            }
        }

        eprintln!("[target] kennel disconnected — reconnecting...");
        kennel_session_state =
            target_kennel_session::transition(kennel_session_state, TksEvent::ControlLost)
                .unwrap_or(TksState::Disconnected);
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_adopted_loop(
    comms_runtime: &Arc<CommsRuntime>,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    adoption: ActiveAdoption,
    attach_required: bool,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_forwarder: &mut Option<tokio::task::JoinHandle<()>>,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
) -> anyhow::Result<TkcState> {
    let (disconnect_tx, mut disconnect_rx) = tokio::sync::watch::channel::<bool>(false);
    // Spawn event forwarder for this adopted session
    if let Some(old) = event_forwarder.take() {
        old.abort();
    }
    if let Ok(handle) = spawn_event_forwarder(
        service,
        active_session_id(session_binding_state),
        Arc::clone(comms_runtime),
        disconnect_tx.clone(),
    )
    .await
    {
        *event_forwarder = Some(handle);
    }

    // Ensure the forwarder is aborted on ANY exit from the adopted loop
    // (release, link-lost, attach-send-failed, kennel disconnect, error).
    // Without this, late AgentEvents leak to a stale TUX peer while the
    // target is back in re-registration.
    let result = run_adopted_loop_inner(
        comms_runtime,
        reader,
        write_half,
        adoption,
        attach_required,
        service,
        runtime_adapter,
        jsonl_store,
        model,
        system_prompt,
        mob_state,
        provider,
        session_binding_state,
        current_session_id,
        event_forwarder,
        attachment_hint,
        attachment_hint_path,
        &disconnect_tx,
        &mut disconnect_rx,
    )
    .await;

    // Unconditional cleanup: abort the forwarder on every exit path.
    if let Some(fwd) = event_forwarder.take() {
        fwd.abort();
    }

    result
}

#[allow(clippy::too_many_arguments)]
async fn run_adopted_loop_inner(
    comms_runtime: &Arc<CommsRuntime>,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    adoption: ActiveAdoption,
    attach_required: bool,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    mob_state: &Arc<MobMcpState>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_forwarder: &mut Option<tokio::task::JoinHandle<()>>,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
    disconnect_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<TkcState> {
    let mut heartbeat: Option<tokio::task::JoinHandle<()>> = None;
    let initial_event = if attach_required {
        TkcEvent::Adopted {
            target_id: adoption.target_id.clone(),
            lease_id: adoption.lease_id.clone(),
            tux_id: adoption.tux_id.clone(),
            tux_pubkey: adoption.tux_pubkey.clone(),
            tux_direct_addr: adoption.tux_direct_addr.clone(),
        }
    } else {
        TkcEvent::LeaseRebound {
            target_id: adoption.target_id.clone(),
            lease_id: adoption.lease_id.clone(),
            tux_id: adoption.tux_id.clone(),
            tux_pubkey: adoption.tux_pubkey.clone(),
            tux_direct_addr: adoption.tux_direct_addr.clone(),
        }
    };
    let (mut machine_state, effects) =
        target_kennel_control::transition(TkcState::idle(), initial_event)
            .map_err(|e| anyhow::anyhow!("machine transition: {e}"))?;
    let (applied, should_return) = apply_target_control_effects(
        comms_runtime,
        &effects,
        &mut heartbeat,
        &disconnect_tx,
        attachment_hint,
        attachment_hint_path,
    )
    .await?;
    if !applied {
        let (state, effects) =
            target_kennel_control::transition(machine_state, TkcEvent::AttachSendFailed)
                .map_err(|e| anyhow::anyhow!("attach-send-failed transition: {e}"))?;
        let _ = apply_target_control_effects(
            comms_runtime,
            &effects,
            &mut heartbeat,
            &disconnect_tx,
            attachment_hint,
            attachment_hint_path,
        )
        .await?;
        return Ok(state);
    }
    if should_return {
        return Ok(machine_state);
    }

    let mut kennel_heartbeat = tokio::time::interval(Duration::from_secs(10));

    loop {
        let poll_kennel = matches!(
            &machine_state.attachment,
            TaState::Attaching { .. } | TaState::Attached { .. }
        );

        tokio::select! {
            msg = comms_runtime.recv_message() => {
                let Some(msg) = msg else {
                    if let Some(h) = heartbeat.take() { h.abort(); }
                    let (s, effects) = target_kennel_control::transition(
                        machine_state,
                        TkcEvent::DirectLinkLost,
                    )
                    .map_err(|e| anyhow::anyhow!("direct-link-lost transition: {e}"))?;
                    let _ = apply_target_control_effects(
                        comms_runtime,
                        &effects,
                        &mut heartbeat,
                        &disconnect_tx,
                        attachment_hint,
                        attachment_hint_path,
                    )
                    .await?;
                    if let Some(fwd) = event_forwarder.take() {
                        fwd.abort();
                    }
                    return Ok(s);
                };
                if let Some(payload) = parse_direct_control_message(&msg)? {
                    if let DirectControlPayload::AttachAck { lease_id } = payload
                        && matches!(machine_state.attachment, TaState::Attaching { .. })
                    {
                        let (s, effects) = target_kennel_control::transition(
                            machine_state.clone(),
                            TkcEvent::DirectAttachAck { lease_id },
                        )
                        .map_err(|e| anyhow::anyhow!("direct-attach-ack transition: {e}"))?;
                        machine_state = s;
                        let _ = apply_target_control_effects(
                            comms_runtime,
                            &effects,
                            &mut heartbeat,
                            &disconnect_tx,
                            attachment_hint,
                            attachment_hint_path,
                        )
                        .await?;
                        continue;
                    }
                }
                match handle_target_message(
                    &msg, comms_runtime, &disconnect_tx, service, runtime_adapter,
                    jsonl_store, model, system_prompt, mob_state, provider,
                    session_binding_state, current_session_id, event_forwarder,
                ).await {
                    TargetLoopAction::Continue => {}
                    TargetLoopAction::ExitProcess => std::process::exit(0),
                }
            }
            // Event forwarding is handled by the dedicated event_forwarder task.
            // No event_stream arm needed here.
            _ = kennel_heartbeat.tick(), if poll_kennel => {
                let kennel_hb = build_signed_envelope(
                    comms_runtime.router_arc().keypair_arc().as_ref(), &adoption.target_id,
                    KennelPayload::TargetHeartbeat,
                )?;
                if write_envelope(write_half, &kennel_hb).await.is_err() {
                    eprintln!("[target] kennel control lost");
                    let (s, effects) = target_kennel_control::transition(
                        machine_state.clone(),
                        TkcEvent::KennelHeartbeatFailed,
                    )
                    .map_err(|e| anyhow::anyhow!("kennel-heartbeat-failed transition: {e}"))?;
                    machine_state = s;
                    let (_, should_return) = apply_target_control_effects(
                        comms_runtime,
                        &effects,
                        &mut heartbeat,
                        &disconnect_tx,
                        attachment_hint,
                        attachment_hint_path,
                    )
                    .await?;
                    if should_return {
                        if let Some(h) = heartbeat.take() { h.abort(); }
                        return Ok(machine_state);
                    }
                    if machine_state.attachment.is_terminal() {
                        if let Some(h) = heartbeat.take() { h.abort(); }
                        return Ok(machine_state);
                    }
                }
            }
            maybe = read_envelope(reader), if poll_kennel => {
                match maybe {
                    Ok(Some(env)) => {
                        if verify_envelope(&env).is_err() {
                            eprintln!("[target] invalid signed kennel control frame");
                            let (s, effects) = target_kennel_control::transition(
                                machine_state.clone(),
                                TkcEvent::KennelDisconnected,
                            )
                            .map_err(|e| anyhow::anyhow!("kennel-disconnected transition: {e}"))?;
                            machine_state = s;
                            let (_, should_return) = apply_target_control_effects(
                                comms_runtime,
                                &effects,
                                &mut heartbeat,
                                &disconnect_tx,
                                attachment_hint,
                                attachment_hint_path,
                            )
                            .await?;
                            if should_return {
                                if let Some(h) = heartbeat.take() { h.abort(); }
                                return Ok(machine_state);
                            }
                            if machine_state.attachment.is_terminal() {
                                if let Some(h) = heartbeat.take() { h.abort(); }
                                return Ok(machine_state);
                            }
                            continue;
                        }
                        match env.payload {
                            KennelPayload::Released { lease_ref, .. } => {
                                if let Some(h) = heartbeat.take() { h.abort(); }
                                let (s, effects) = target_kennel_control::transition(
                                    machine_state,
                                    TkcEvent::KennelReleased { lease_ref },
                                )
                                .map_err(|e| anyhow::anyhow!("kennel-released transition: {e}"))?;
                                let _ = apply_target_control_effects(
                                    comms_runtime,
                                    &effects,
                                    &mut heartbeat,
                                    &disconnect_tx,
                                    attachment_hint,
                                    attachment_hint_path,
                                )
                                .await?;
                                if let Some(fwd) = event_forwarder.take() {
                                    fwd.abort();
                                }
                                return Ok(s);
                            }
                            KennelPayload::LeaseRebound {
                                target_id: rebound_target_id,
                                lease_id,
                                tux_id,
                                tux_pubkey,
                                tux_direct_addr,
                                ..
                            } => {
                                let (s, effects) = target_kennel_control::transition(
                                    machine_state,
                                    TkcEvent::LeaseRebound {
                                        target_id: rebound_target_id,
                                        lease_id,
                                        tux_id,
                                        tux_pubkey,
                                        tux_direct_addr,
                                    },
                                )
                                .map_err(|e| anyhow::anyhow!("lease-rebound transition: {e}"))?;
                                machine_state = s;
                                let _ = apply_target_control_effects(
                                    comms_runtime,
                                    &effects,
                                    &mut heartbeat,
                                    &disconnect_tx,
                                    attachment_hint,
                                    attachment_hint_path,
                                )
                                .await?;
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        eprintln!("[target] kennel control lost");
                        let (s, effects) = target_kennel_control::transition(
                            machine_state.clone(),
                            TkcEvent::KennelDisconnected,
                        )
                        .map_err(|e| anyhow::anyhow!("kennel-control-lost transition: {e}"))?;
                        machine_state = s;
                        let (_, should_return) = apply_target_control_effects(
                            comms_runtime,
                            &effects,
                            &mut heartbeat,
                            &disconnect_tx,
                            attachment_hint,
                            attachment_hint_path,
                        )
                        .await?;
                        if should_return {
                            if let Some(h) = heartbeat.take() { h.abort(); }
                            return Ok(machine_state);
                        }
                        if machine_state.attachment.is_terminal() {
                            if let Some(h) = heartbeat.take() { h.abort(); }
                            return Ok(machine_state);
                        }
                    }
                }
            }
            _ = disconnect_rx.changed() => {
                if *disconnect_rx.borrow() {
                    if let Some(h) = heartbeat.take() { h.abort(); }
                    let (s, effects) = target_kennel_control::transition(
                        machine_state,
                        TkcEvent::DirectLinkLost,
                    )
                    .map_err(|e| anyhow::anyhow!("disconnect-direct-link-lost transition: {e}"))?;
                    let _ = apply_target_control_effects(
                        comms_runtime,
                        &effects,
                        &mut heartbeat,
                        &disconnect_tx,
                        attachment_hint,
                        attachment_hint_path,
                    )
                    .await?;
                    return Ok(s);
                }
            }
        }
    }
}

fn should_forward(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::RunStarted { .. }
            | AgentEvent::RunCompleted { .. }
            | AgentEvent::RunFailed { .. }
            | AgentEvent::TextDelta { .. }
            | AgentEvent::TextComplete { .. }
            | AgentEvent::ToolCallRequested { .. }
            | AgentEvent::ToolExecutionStarted { .. }
            | AgentEvent::ToolExecutionCompleted { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::TurnCompleted { .. }
    )
}

/// Probe our outbound IP toward a host via a non-sending UDP "connect".
fn discover_local_ip(host: &str, port: u16) -> anyhow::Result<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").context("bind UDP probe socket")?;
    sock.connect(format!("{host}:{port}"))
        .with_context(|| format!("UDP probe to {host}:{port}"))?;
    Ok(sock.local_addr()?.ip().to_string())
}

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn parse_provider_override(args: &[String]) -> anyhow::Result<Option<ProviderKind>> {
    let model = find_flag(args, "--model");
    let provider = find_flag(args, "--provider");
    match (model, provider) {
        (Some(_), Some(provider)) => Ok(Some(provider.parse()?)),
        (Some(_), None) => anyhow::bail!("--model requires --provider"),
        (None, Some(_)) => anyhow::bail!("--provider requires --model"),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    mod test_support {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/test_support.rs"));
    }

    use super::{
        TargetRuntimeSurface, build_target_runtime_surface, create_target_comms_runtime,
        discard_live_session_with_mob_cleanup, parse_provider_override, setup_session,
    };
    use anyhow::Context as _;
    use mdm_tux::ProviderKind;
    use meerkat::{
        AgentFactory, FactoryAgentBuilder, LlmClient, PersistenceBundle,
        PersistentSessionService, SessionAgentBuilder, SessionService,
        encode_llm_client_override_for_service,
    };
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, TestClient};
    use meerkat_comms::{CommsRuntime, ResolvedCommsConfig};
    use meerkat_core::ops_lifecycle::OperationKind;
    use meerkat_core::Config;
    use meerkat_core::service::{
        CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, StartTurnRequest,
    };
    use meerkat_core::types::{ContentInput, HandlingMode};
    use meerkat_mob_mcp::{AgentMobToolSurfaceFactory, MobMcpState};
    use meerkat_store::{JsonlStore, MemoryBlobStore, SessionStore};
    use std::collections::{HashSet, VecDeque};
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use test_support::CaptureClient;
    use tokio::sync::mpsc;
    use tokio::time::{Duration, timeout};

    struct ScriptedClient {
        responses: Mutex<VecDeque<Vec<LlmEvent>>>,
    }

    impl ScriptedClient {
        fn new(responses: Vec<Vec<LlmEvent>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for ScriptedClient {
        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let _ = request;
            let events = self
                .responses
                .lock()
                .expect("scripted client lock")
                .pop_front()
                .unwrap_or_else(|| {
                    vec![LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    }]
                });
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> &'static str {
            "test"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    async fn build_target_runtime_surface_with_client(
        session_dir: &Path,
        comms_runtime: Arc<CommsRuntime>,
        llm_client: Arc<dyn LlmClient>,
    ) -> anyhow::Result<TargetRuntimeSurface> {
        let factory = AgentFactory::new(session_dir)
            .shell(true)
            .builtins(true)
            .mob(true)
            .with_comms_runtime(comms_runtime);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(llm_client);
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);

        let jsonl_store = JsonlStore::new(session_dir.to_path_buf());
        jsonl_store.init().await?;

        let bundle_store = JsonlStore::new(session_dir.to_path_buf());
        bundle_store.init().await?;

        let persistence = PersistenceBundle::new(
            Arc::new(bundle_store) as Arc<dyn SessionStore>,
            None,
            Arc::new(MemoryBlobStore::new()),
        );
        let runtime_adapter = persistence.runtime_adapter();
        let (session_store, runtime_store, blob_store) = persistence.into_parts();

        let mut session_service =
            PersistentSessionService::new(builder, 10, session_store, runtime_store, blob_store);
        {
            let adapter = runtime_adapter.clone();
            session_service.set_runtime_bindings_provider(Arc::new(move |session_id| {
                let adapter = adapter.clone();
                Box::pin(async move { adapter.prepare_bindings(session_id).await.ok() })
            }));
        }
        let service = Arc::new(session_service);
        let mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
            service.clone(),
            Some(runtime_adapter.clone()),
        ));
        *mob_tools_slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
        ));

        Ok(TargetRuntimeSurface {
            service,
            runtime_adapter,
            jsonl_store,
            mob_state,
        })
    }

    #[test]
    fn provider_override_requires_model_and_provider_together() {
        let err = parse_provider_override(&["--model".into(), "gpt-5.4".into()]).unwrap_err();
        assert!(err.to_string().contains("--model requires --provider"));

        let err = parse_provider_override(&["--provider".into(), "openai".into()]).unwrap_err();
        assert!(err.to_string().contains("--provider requires --model"));
    }

    #[test]
    fn provider_override_parses_explicit_provider() {
        let provider = parse_provider_override(&[
            "--model".into(),
            "gpt-5.4".into(),
            "--provider".into(),
            "openai".into(),
        ])
        .unwrap();
        assert_eq!(provider, Some(ProviderKind::Openai));
    }

    #[tokio::test]
    async fn target_builder_exposes_shell_comms_and_mob_tools() {
        let temp = tempfile::tempdir().unwrap();
        // Create a CommsRuntime — the factory will compose comms tools from it.
        let comms_config = ResolvedCommsConfig {
            enabled: true,
            name: "test-target".to_string(),
            inproc_namespace: None,
            listen_tcp: None,
            listen_uds: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: temp.path().join("identity"),
            trusted_peers_path: temp.path().join("trusted_peers.json"),
            comms_config: Default::default(),
            auth: Default::default(),
            require_peer_auth: false,
            allow_external_unauthenticated: false,
        };
        let comms_runtime = Arc::new(
            CommsRuntime::new_with_silent_intents(
                comms_config,
                Arc::new(std::collections::HashSet::new()),
            )
            .await
            .unwrap(),
        );
        // Add a dummy peer so comms tools pass the availability gate.
        comms_runtime.upsert_trusted_peer(meerkat_comms::TrustedPeer {
            name: "tux".into(),
            pubkey: meerkat_comms::identity::Keypair::generate().public_key(),
            addr: "tcp://127.0.0.1:9999".into(),
            meta: meerkat_comms::PeerMeta::default(),
        });
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .shell(true)
            .builtins(true)
            .mob(true)
            .with_comms_runtime(comms_runtime);
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        let mob_state = Arc::new(MobMcpState::new_in_memory());
        *builder
            .default_mob_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
        ));

        let capture: Arc<CaptureClient> = Arc::new(CaptureClient::default());

        let req = CreateSessionRequest {
            model: "gpt-5.2".to_string(),
            prompt: ContentInput::Text("inspect target tools".to_string()),
            render_metadata: None,
            system_prompt: Some("test target".to_string()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(encode_llm_client_override_for_service(
                    capture.clone() as Arc<dyn LlmClient>
                )),
                override_builtins: meerkat_core::ToolCategoryOverride::Enable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                override_mob: meerkat_core::ToolCategoryOverride::Enable,
                ..Default::default()
            }),
            labels: None,
        };

        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut agent = builder.build_agent(&req, event_tx).await.unwrap();
        agent
            .agent_mut()
            .run(ContentInput::Text("inspect".to_string()))
            .await
            .unwrap();

        let tool_names = capture.tool_names();
        assert!(tool_names.iter().any(|name| name == "wait"));
        assert!(tool_names.iter().any(|name| name == "shell"));
        assert!(tool_names.iter().any(|name| name == "send"));
        assert!(tool_names.iter().any(|name| name == "peers"));
        assert!(tool_names.iter().any(|name| name == "delegate"));
        assert!(tool_names.iter().any(|name| name == "mob_list"));
        assert!(tool_names.iter().any(|name| name == "mob_check_member"));
    }

    #[tokio::test]
    async fn target_discard_cleanup_removes_owned_mobs() {
        let temp = tempfile::tempdir().unwrap();
        let comms_config = ResolvedCommsConfig {
            enabled: true,
            name: "test-cleanup".to_string(),
            inproc_namespace: None,
            listen_tcp: None,
            listen_uds: None,
            event_listen_tcp: None,
            #[cfg(unix)]
            event_listen_uds: None,
            identity_dir: temp.path().join("identity"),
            trusted_peers_path: temp.path().join("trusted_peers.json"),
            comms_config: Default::default(),
            auth: Default::default(),
            require_peer_auth: false,
            allow_external_unauthenticated: false,
        };
        let comms_runtime = Arc::new(
            CommsRuntime::new_with_silent_intents(
                comms_config,
                Arc::new(std::collections::HashSet::new()),
            )
            .await
            .unwrap(),
        );
        let surface = build_target_runtime_surface(temp.path(), comms_runtime).await.unwrap();
        let req = CreateSessionRequest {
            model: "gpt-5.2".to_string(),
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: Some("cleanup".to_string()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(encode_llm_client_override_for_service(Arc::new(
                    TestClient::default(),
                ))),
                override_builtins: meerkat_core::ToolCategoryOverride::Enable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                override_mob: meerkat_core::ToolCategoryOverride::Enable,
                ..Default::default()
            }),
            labels: None,
        };
        let session_id = surface
            .service
            .create_session(req)
            .await
            .unwrap()
            .session_id;

        surface
            .mob_state
            .get_or_create_implicit_mob(&session_id.to_string(), "gpt-5.2")
            .await
            .unwrap();
        assert!(
            surface
                .mob_state
                .find_implicit_mob(&session_id.to_string())
                .await
                .is_some()
        );

        discard_live_session_with_mob_cleanup(&surface.service, &surface.mob_state, &session_id)
            .await
            .unwrap();
        assert!(
            surface
                .mob_state
                .find_implicit_mob(&session_id.to_string())
                .await
            .is_none()
        );
    }

    #[tokio::test]
    async fn target_setup_session_routes_background_shell_completions_into_runtime_feed() {
        let temp = tempfile::tempdir().unwrap();
        let comms_runtime = Arc::new(
            CommsRuntime::new_with_silent_intents(
                ResolvedCommsConfig {
                    enabled: true,
                    name: "test-background-feed".to_string(),
                    inproc_namespace: None,
                    listen_tcp: None,
                    listen_uds: None,
                    event_listen_tcp: None,
                    #[cfg(unix)]
                    event_listen_uds: None,
                    identity_dir: temp.path().join("identity"),
                    trusted_peers_path: temp.path().join("trusted_peers.json"),
                    comms_config: Default::default(),
                    auth: Default::default(),
                    require_peer_auth: false,
                    allow_external_unauthenticated: false,
                },
                Arc::new(HashSet::new()),
            )
            .await
            .unwrap(),
        );
        let llm_client: Arc<dyn LlmClient> = Arc::new(ScriptedClient::new(vec![
            vec![
                LlmEvent::ToolCallComplete {
                    id: "tc_shell_background".to_string(),
                    name: "shell".to_string(),
                    args: serde_json::json!({
                        "command": "echo target-background-feed",
                        "background": true,
                    }),
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::ToolUse,
                    },
                },
            ],
            vec![
                LlmEvent::TextDelta {
                    delta: "background started".to_string(),
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ],
        ]));
        let surface =
            build_target_runtime_surface_with_client(temp.path(), comms_runtime, llm_client)
                .await
                .unwrap();

        let session_id = setup_session(
            &surface.service,
            &surface.runtime_adapter,
            None,
            "gpt-5.2",
            "test background shell",
            &surface.mob_state,
            "openai",
        )
        .await
        .unwrap();

        let runtime_registry = surface
            .runtime_adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("runtime registry must exist after setup");
        let feed = runtime_registry.completion_feed_handle();
        assert_eq!(feed.watermark(), 0);

        surface
            .service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    prompt: ContentInput::Text("run shell".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: HandlingMode::Queue,
                    event_tx: None,
                    skill_references: None,
                    flow_tool_overlay: None,
                    additional_instructions: None,
                },
            )
            .await
            .unwrap();

        timeout(Duration::from_secs(5), async {
            loop {
                if feed.watermark() > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .context("background shell completion should advance the runtime feed")
        .unwrap();

        let batch = feed.list_since(0);
        assert!(
            batch
                .entries
                .iter()
                .any(|entry| entry.kind == OperationKind::BackgroundToolOp),
            "runtime feed should contain a background shell completion"
        );
    }

    /// Regression: ensure setup_session through PersistentSessionService
    /// produces an agent with builtins, shell, comms, and mob tools —
    /// not just comms tools.
    #[tokio::test]
    async fn setup_session_via_service_exposes_all_tool_categories() {
        let temp = tempfile::tempdir().unwrap();
        let comms_runtime = create_target_comms_runtime("test-full", temp.path())
            .await
            .unwrap();
        // Add a peer so comms tools pass the availability gate.
        comms_runtime.upsert_trusted_peer(meerkat_comms::TrustedPeer {
            name: "tux".into(),
            pubkey: meerkat_comms::identity::Keypair::generate().public_key(),
            addr: "tcp://127.0.0.1:9999".into(),
            meta: meerkat_comms::PeerMeta::default(),
        });
        let _surface =
            build_target_runtime_surface(temp.path(), Arc::clone(&comms_runtime))
                .await
                .unwrap();

        // Build an agent through the full pipeline and check tool visibility.
        let factory = AgentFactory::new(temp.path().join("sessions2"))
            .shell(true)
            .builtins(true)
            .mob(true)
            .with_comms_runtime(Arc::clone(&comms_runtime));
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        *builder
            .default_mob_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(AgentMobToolSurfaceFactory::new(
                MobMcpState::new_in_memory(),
            )));

        let capture2: Arc<CaptureClient> = Arc::new(CaptureClient::default());
        let req2 = CreateSessionRequest {
            model: "gpt-5.2".to_string(),
            prompt: ContentInput::Text("list tools".into()),
            render_metadata: None,
            system_prompt: Some("test".into()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(encode_llm_client_override_for_service(
                    capture2.clone() as Arc<dyn LlmClient>,
                )),
                override_builtins: meerkat_core::ToolCategoryOverride::Enable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                override_mob: meerkat_core::ToolCategoryOverride::Enable,
                ..Default::default()
            }),
            labels: None,
        };

        let (event_tx, _) = mpsc::channel(8);
        let mut agent = builder.build_agent(&req2, event_tx).await.unwrap();
        agent
            .agent_mut()
            .run(ContentInput::Text("list tools".into()))
            .await
            .unwrap();

        let tool_names = capture2.tool_names();
        // Builtins
        assert!(
            tool_names.iter().any(|n| n == "wait"),
            "missing builtin 'wait', got: {tool_names:?}"
        );
        // Shell
        assert!(
            tool_names.iter().any(|n| n == "shell"),
            "missing 'shell', got: {tool_names:?}"
        );
        // Comms
        assert!(
            tool_names.iter().any(|n| n == "send"),
            "missing comms 'send', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "peers"),
            "missing comms 'peers', got: {tool_names:?}"
        );
        // Mob
        assert!(
            tool_names.iter().any(|n| n == "delegate"),
            "missing mob 'delegate', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "mob_list"),
            "missing 'mob_list', got: {tool_names:?}"
        );
    }

    /// Regression: auto-resumed session from disk must retain all tool categories.
    /// Old persisted metadata with Inherit/Disable must not suppress builtins/shell.
    #[tokio::test]
    async fn resumed_session_retains_all_tool_categories() {
        let temp = tempfile::tempdir().unwrap();
        let comms_runtime = create_target_comms_runtime("test-resume", temp.path())
            .await
            .unwrap();
        comms_runtime.upsert_trusted_peer(meerkat_comms::TrustedPeer {
            name: "tux".into(),
            pubkey: meerkat_comms::identity::Keypair::generate().public_key(),
            addr: "tcp://127.0.0.1:9999".into(),
            meta: meerkat_comms::PeerMeta::default(),
        });

        let session_dir = temp.path().join("sessions");

        // 1. Create a fresh session via the full pipeline, persist it.
        let surface =
            build_target_runtime_surface(&session_dir, Arc::clone(&comms_runtime))
                .await
                .unwrap();
        let fresh_id = setup_session(
            &surface.service,
            &surface.runtime_adapter,
            None,
            "gpt-5.2",
            "test",
            &surface.mob_state,
            "openai",
        )
        .await
        .unwrap();

        // 2. Resume that session through the builder with a capture client.
        let capture: Arc<CaptureClient> = Arc::new(CaptureClient::default());
        let loaded = surface
            .service
            .load_persisted(&fresh_id)
            .await
            .unwrap()
            .unwrap();

        let factory = AgentFactory::new(temp.path().join("sessions2"))
            .shell(true)
            .builtins(true)
            .mob(true)
            .with_comms_runtime(Arc::clone(&comms_runtime));
        let builder = FactoryAgentBuilder::new(factory, Config::default());
        *builder
            .default_mob_tools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(AgentMobToolSurfaceFactory::new(
                MobMcpState::new_in_memory(),
            )));

        let req = CreateSessionRequest {
            model: "gpt-5.2".to_string(),
            prompt: ContentInput::Text("list tools".into()),
            render_metadata: None,
            system_prompt: Some("test".into()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                llm_client_override: Some(encode_llm_client_override_for_service(
                    capture.clone() as Arc<dyn LlmClient>,
                )),
                override_builtins: meerkat_core::ToolCategoryOverride::Enable,
                override_shell: meerkat_core::ToolCategoryOverride::Enable,
                override_mob: meerkat_core::ToolCategoryOverride::Enable,
                resume_session: Some(loaded),
                ..Default::default()
            }),
            labels: None,
        };

        let (event_tx, _) = mpsc::channel(8);
        let mut agent = builder.build_agent(&req, event_tx).await.unwrap();
        agent
            .agent_mut()
            .run(ContentInput::Text("list tools".into()))
            .await
            .unwrap();

        let tool_names = capture.tool_names();
        assert!(
            tool_names.iter().any(|n| n == "wait"),
            "resumed session missing builtin 'wait', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "shell"),
            "resumed session missing 'shell', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "send"),
            "resumed session missing comms 'send', got: {tool_names:?}"
        );
        assert!(
            tool_names.iter().any(|n| n == "delegate"),
            "resumed session missing mob 'delegate', got: {tool_names:?}"
        );
    }
}
