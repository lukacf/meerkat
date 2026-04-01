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
//! mcm-target <HOST:PORT> [--name NAME] [--model MODEL]
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
use meerkat::{AgentFactory, FactoryAgentBuilder, PersistenceBundle, build_persistent_service};
use meerkat_comms::MessageKind;
use meerkat_comms::agent::CommsToolDispatcher;
use meerkat_comms::{PeerMeta, TrustedPeer};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunApplyBoundary, RunPrimitive};
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, SessionError, SessionService,
    StartTurnRequest,
};
use meerkat_core::types::{ContentInput, HandlingMode, SessionId};
use meerkat_core::{AgentEvent, AgentToolDispatcher, Config};
use meerkat_runtime::RuntimeSessionAdapter;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use meerkat_store::{JsonlStore, MemoryBlobStore, SessionFilter, SessionStore};

use mdm_tux::{
    CommsNode, DirectControlPayload, KennelPayload, ProviderKind, RegResponse, auto_detect,
    build_signed_envelope, direct_control_request, load_or_generate_keypair,
    parse_direct_control_message, read_envelope, register_with_backoff, verify_envelope,
    write_envelope,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    // ── Parse CLI args ────────────────────────────────────────────────────────
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!(
            "Usage: mcm-target <HOST:PORT> [--name NAME] [--model MODEL --provider PROVIDER]"
        );
        eprintln!(
            "   or: mcm-target --kennel HOST:PORT [--advertise IP] [--name NAME] [--model MODEL --provider PROVIDER]"
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

    // ── 1. Load or generate identity ──────────────────────────────────────────
    let keypair = load_or_generate_keypair(&data_dir.join("identity")).await?;

    // ── 2. Create comms node + bind a random free port ────────────────────────
    // listen("0.0.0.0:0") binds once and returns the actual address, avoiding
    // the TOCTOU race of probing a port and then re-binding.
    let node = CommsNode::new(keypair);
    let comms_addr = node.listen("0.0.0.0:0").await?;
    let comms_port = comms_addr.port();

    println!("=== MDM Target: {name} ===");
    println!("comms     : tcp://0.0.0.0:{comms_port}");
    println!("provider  : {provider} ({model})");

    // ── 3. Build runtime-backed session service ───────────────────────────────
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;

    let factory = AgentFactory::new(&session_dir).shell(true).builtins(true);
    let config = Config::default();

    let jsonl_store = JsonlStore::new(session_dir.clone());
    jsonl_store.init().await?;
    // Separate store instance for the persistence bundle (JsonlStore doesn't Clone)
    let bundle_store = JsonlStore::new(session_dir.clone());
    bundle_store.init().await?;
    let persistence = PersistenceBundle::new(
        Arc::new(bundle_store) as Arc<dyn SessionStore>,
        None, // no runtime store for jsonl
        Arc::new(MemoryBlobStore::new()),
    );
    let runtime_adapter = persistence.runtime_adapter();

    let service: Arc<PersistentSessionService<FactoryAgentBuilder>> =
        Arc::new(build_persistent_service(factory, config, 10, persistence));

    let system_prompt = SYSTEM_PROMPT.replace("{name}", &name);

    // Comms tools: send/peers tools backed by the CommsNode's router
    let comms_tools: Arc<dyn AgentToolDispatcher> = Arc::new(CommsToolDispatcher::new(
        node.router.clone(),
        node.trusted.clone(),
    ));

    // ── 4. Create or auto-resume session ──────────────────────────────────────
    let mut current_session_id = create_or_resume_session(
        &service,
        &runtime_adapter,
        &jsonl_store,
        &model,
        &system_prompt,
        &comms_tools,
        &provider,
    )
    .await?;

    // ── 5. Subscribe to session events for forwarding ─────────────────────────
    let mut event_stream = service
        .subscribe_session_events(&current_session_id)
        .await
        .map_err(|e| anyhow::anyhow!("subscribe session events: {e}"))?;
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

    let router = node.router.clone();
    let mut node = node;

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
            match register_with_backoff(&reg_addr, &name, &node.pubkey_string(), &advertised_addr)
                .await
            {
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
            let stale_keys: Vec<_> = node
                .trusted
                .read()
                .peers
                .iter()
                .filter(|p| p.name == resp_name)
                .map(|p| p.pubkey)
                .collect();
            for pk in stale_keys {
                node.router.remove_trusted_peer(&pk);
            }
        }
        node.add_peer(TrustedPeer {
            name: resp_name,
            pubkey: host_pubkey,
            addr: format!("tcp://{host_addr}"),
            meta: PeerMeta::default(),
        });

        // Disconnect signal: heartbeat + event forwarder can trigger reconnect
        let (disconnect_tx, disconnect_rx) = tokio::sync::watch::channel(false);

        // Spawn heartbeat (pings TUX every 10s via typed Request, triggers disconnect on 3 failures)
        let hb = spawn_heartbeat(router.clone(), "tux", disconnect_tx.clone());

        eprintln!("[target] ready — waiting for commands\n");

        // Run inbox loop (exits on disconnect signal or DISMISS command)
        run_inbox_loop(
            &mut node,
            &router,
            disconnect_rx,
            disconnect_tx,
            &service,
            &runtime_adapter,
            &jsonl_store,
            &model,
            &system_prompt,
            &comms_tools,
            &provider,
            &mut session_binding_state,
            &mut current_session_id,
            &mut event_stream,
        )
        .await;

        hb.abort();
        eprintln!("[target] disconnected — reconnecting...");
    }
}

// ── Inbox loop with select!-based disconnect ─────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_inbox_loop(
    node: &mut CommsNode,
    router: &Arc<meerkat_comms::Router>,
    mut disconnect_rx: tokio::sync::watch::Receiver<bool>,
    disconnect_tx: tokio::sync::watch::Sender<bool>,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    comms_tools: &Arc<dyn AgentToolDispatcher>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_stream: &mut meerkat_core::comms::EventStream,
) {
    loop {
        // select! over inbox message, session events, and disconnect signal
        tokio::select! {
            msg = node.recv_message() => {
                let Some(msg) = msg else { break };
                match handle_target_message(
                    &msg,
                    router,
                    service,
                    runtime_adapter,
                    jsonl_store,
                    model,
                    system_prompt,
                    comms_tools,
                    provider,
                    session_binding_state,
                    current_session_id,
                    event_stream,
                )
                .await {
                    TargetLoopAction::Continue => {}
                    TargetLoopAction::ExitProcess => std::process::exit(0),
                }
            }

            // Forward session events to TUX
            event = event_stream.next() => {
                let Some(envelope) = event else { continue };
                let event = &envelope.payload;
                if should_forward(event) && !forward_stream_event(router, "tux", event).await {
                    let _ = disconnect_tx.send(true);
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
    node: &mut CommsNode,
    router: &Arc<meerkat_comms::Router>,
    effects: &[TaEffect],
    heartbeat: &mut Option<tokio::task::JoinHandle<()>>,
    disconnect_tx: &tokio::sync::watch::Sender<bool>,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
) -> anyhow::Result<bool> {
    for effect in effects {
        match effect {
            TaEffect::EnsureTuxPeer {
                tux_pubkey,
                tux_direct_addr,
            } => {
                let tux_pk = meerkat_comms::identity::PubKey::from_peer_id(tux_pubkey)
                    .context("parse adopted tux pubkey")?;
                let stale_keys: Vec<_> = node
                    .trusted
                    .read()
                    .peers
                    .iter()
                    .filter(|p| p.name == "tux")
                    .map(|p| p.pubkey)
                    .collect();
                for pk in stale_keys {
                    node.router.remove_trusted_peer(&pk);
                }
                node.add_peer(TrustedPeer {
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
    node: &mut CommsNode,
    router: &Arc<meerkat_comms::Router>,
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
        node,
        router,
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
    router: &Arc<meerkat_comms::Router>,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    comms_tools: &Arc<dyn AgentToolDispatcher>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_stream: &mut meerkat_core::comms::EventStream,
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
                comms_tools,
                provider,
                session_binding_state,
                current_session_id,
                event_stream,
            )
            .await;

            // Timeout-guard the reply to prevent a half-open controller from
            // wedging the inbox loop (same bound as heartbeat/event sends).
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
                    params: serde_json::Value::Null,
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
    comms_tools: &Arc<dyn AgentToolDispatcher>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_stream: &mut meerkat_core::comms::EventStream,
) -> String {
    if cmd == "NEW_SESSION" {
        match switch_session(
            service,
            runtime_adapter,
            None,
            model,
            system_prompt,
            comms_tools,
            provider,
            session_binding_state,
            current_session_id,
            event_stream,
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
                    comms_tools,
                    provider,
                    session_binding_state,
                    current_session_id,
                    event_stream,
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
    comms_tools: &Arc<dyn AgentToolDispatcher>,
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
                comms_tools,
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
        comms_tools,
        provider,
    )
    .await
}

/// Switch to a new or resumed session. Drops the old event subscription,
/// creates/resumes the session, registers with the runtime adapter,
/// and subscribes to the new session's events.
#[allow(clippy::too_many_arguments)]
async fn switch_session(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    resume_id: Option<SessionId>,
    model: &str,
    system_prompt: &str,
    comms_tools: &Arc<dyn AgentToolDispatcher>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_stream: &mut meerkat_core::comms::EventStream,
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
                comms_tools,
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

    let mut new_event_stream = None;
    for effect in effects {
        match effect {
            SessionBindingEffect::SubscribeSessionEvents { session_id } => {
                match service.subscribe_session_events(&session_id).await {
                    Ok(stream) => new_event_stream = Some(stream),
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
                                    if let Err(discard_err) =
                                        service.discard_live_session(&session_id).await
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
                if let Err(e) = service.discard_live_session(&session_id).await
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
                if let Err(e) = service.discard_live_session(&session_id).await
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

    if let Some(stream) = new_event_stream {
        *event_stream = stream;
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
    comms_tools: &Arc<dyn AgentToolDispatcher>,
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
        None => None,
    };

    let build_opts = SessionBuildOptions {
        provider: Some(meerkat_core::Provider::from_name(provider)),
        external_tools: Some(comms_tools.clone()),
        override_builtins: Some(true),
        override_shell: Some(true),
        resume_session,
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
        build: Some(build_opts),
        labels: None,
    };

    let result = service
        .create_session(req)
        .await
        .map_err(|e| anyhow::anyhow!("create session: {e}"))?;

    let session_id = result.session_id;
    eprintln!("[target] session ready: {session_id}");

    // Create executor and register with runtime adapter for Steer support
    let executor = Box::new(TargetCoreExecutor::new(service.clone(), session_id.clone()));
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
    session_id: SessionId,
}

impl TargetCoreExecutor {
    fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        session_id: SessionId,
    ) -> Self {
        Self {
            service,
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
                let discard_result = self.service.discard_live_session(&self.session_id).await;
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

    let keypair = load_or_generate_keypair(&data_dir.join("identity")).await?;
    let node = CommsNode::new(keypair);
    let comms_addr = node.listen("0.0.0.0:0").await?;
    let comms_port = comms_addr.port();
    let target_id = node.pubkey_string();

    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let factory = AgentFactory::new(&session_dir).shell(true).builtins(true);
    let config = Config::default();
    let jsonl_store = JsonlStore::new(session_dir.clone());
    jsonl_store.init().await?;
    let bundle_store = JsonlStore::new(session_dir.clone());
    bundle_store.init().await?;
    let persistence = PersistenceBundle::new(
        Arc::new(bundle_store) as Arc<dyn SessionStore>,
        None,
        Arc::new(MemoryBlobStore::new()),
    );
    let runtime_adapter = persistence.runtime_adapter();
    let service: Arc<PersistentSessionService<FactoryAgentBuilder>> =
        Arc::new(build_persistent_service(factory, config, 10, persistence));
    let system_prompt = SYSTEM_PROMPT.replace("{name}", &name);
    let comms_tools: Arc<dyn AgentToolDispatcher> = Arc::new(CommsToolDispatcher::new(
        node.router.clone(),
        node.trusted.clone(),
    ));
    let mut current_session_id = create_or_resume_session(
        &service,
        &runtime_adapter,
        &jsonl_store,
        &model,
        &system_prompt,
        &comms_tools,
        &provider,
    )
    .await?;
    let mut event_stream = service
        .subscribe_session_events(&current_session_id)
        .await
        .map_err(|e| anyhow::anyhow!("subscribe session events: {e}"))?;
    let (mut session_binding_state, _) = target_session_binding::transition(
        SessionBindingState::Unbound,
        SessionBindingEvent::BootResolved {
            session_id: current_session_id.clone(),
        },
    )
    .map_err(|e| anyhow::anyhow!("session binding bootstrap: {e}"))?;

    let explicit_ip = find_flag(args, "--advertise");
    let mut node = node;
    let router = node.router.clone();
    let attachment_hint_path = data_dir.join("attachment_hint.json");

    println!("=== MCM Target: {name} ===");
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
            node.router.keypair_arc().as_ref(),
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
                        node.router.keypair_arc().as_ref(),
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
                                &mut node,
                                &router,
                                &mut reader,
                                &mut write_half,
                                adoption,
                                true,
                                &service,
                                &runtime_adapter,
                                &jsonl_store,
                                &model,
                                &system_prompt,
                                &comms_tools,
                                &provider,
                                &mut session_binding_state,
                                &mut current_session_id,
                                &mut event_stream,
                                &mut attached_tux_hint,
                                &attachment_hint_path,
                            ).await?;

                            let re_register = build_signed_envelope(
                                node.router.keypair_arc().as_ref(),
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
                                &mut node,
                                &router,
                                &mut reader,
                                &mut write_half,
                                adoption,
                                false,
                                &service,
                                &runtime_adapter,
                                &jsonl_store,
                                &model,
                                &system_prompt,
                                &comms_tools,
                                &provider,
                                &mut session_binding_state,
                                &mut current_session_id,
                                &mut event_stream,
                                &mut attached_tux_hint,
                                &attachment_hint_path,
                            ).await?;

                            let re_register = build_signed_envelope(
                                node.router.keypair_arc().as_ref(),
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
    node: &mut CommsNode,
    router: &Arc<meerkat_comms::Router>,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    adoption: ActiveAdoption,
    attach_required: bool,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
    jsonl_store: &JsonlStore,
    model: &str,
    system_prompt: &str,
    comms_tools: &Arc<dyn AgentToolDispatcher>,
    provider: &str,
    session_binding_state: &mut SessionBindingState,
    current_session_id: &mut SessionId,
    event_stream: &mut meerkat_core::comms::EventStream,
    attachment_hint: &mut Option<PersistedAttachmentHint>,
    attachment_hint_path: &Path,
) -> anyhow::Result<TkcState> {
    let (disconnect_tx, mut disconnect_rx) = tokio::sync::watch::channel(false);
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
        node,
        router,
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
            node,
            router,
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
            msg = node.recv_message() => {
                let Some(msg) = msg else {
                    if let Some(h) = heartbeat.take() { h.abort(); }
                    let (s, effects) = target_kennel_control::transition(
                        machine_state,
                        TkcEvent::DirectLinkLost,
                    )
                    .map_err(|e| anyhow::anyhow!("direct-link-lost transition: {e}"))?;
                    let _ = apply_target_control_effects(
                        node,
                        router,
                        &effects,
                        &mut heartbeat,
                        &disconnect_tx,
                        attachment_hint,
                        attachment_hint_path,
                    )
                    .await?;
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
                            node,
                            router,
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
                    &msg, router, service, runtime_adapter, jsonl_store, model,
                    system_prompt, comms_tools, provider, session_binding_state, current_session_id, event_stream,
                ).await {
                    TargetLoopAction::Continue => {}
                    TargetLoopAction::ExitProcess => std::process::exit(0),
                }
            }
            event = event_stream.next() => {
                let Some(envelope) = event else { continue };
                let ev = &envelope.payload;
                if should_forward(ev) && !forward_stream_event(router, "tux", ev).await {
                    if let Some(h) = heartbeat.take() { h.abort(); }
                    let (s, effects) = target_kennel_control::transition(
                        machine_state,
                        TkcEvent::DirectLinkLost,
                    )
                    .map_err(|e| anyhow::anyhow!("stream-direct-link-lost transition: {e}"))?;
                    let _ = apply_target_control_effects(
                        node,
                        router,
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
            _ = kennel_heartbeat.tick(), if poll_kennel => {
                let kennel_hb = build_signed_envelope(
                    node.router.keypair_arc().as_ref(), &adoption.target_id,
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
                        node,
                        router,
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
                                node,
                                router,
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
                                    node,
                                    router,
                                    &effects,
                                    &mut heartbeat,
                                    &disconnect_tx,
                                    attachment_hint,
                                    attachment_hint_path,
                                )
                                .await?;
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
                                    node,
                                    router,
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
                            node,
                            router,
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
                        node,
                        router,
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
    use super::parse_provider_override;
    use mdm_tux::ProviderKind;

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
}
