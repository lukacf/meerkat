//! # 035 — MDM TUX: Target Agent
//!
//! Runs on managed machines. Registers with a TUX host automatically,
//! then serves as a directly-controlled agent with streaming output.
//!
//! Sessions are persisted to disk. On restart the most recent session
//! is automatically resumed. TUX can send `/new` to start a fresh
//! session or `/resume` to pick from past sessions.
//!
//! ```text
//! target <HOST:PORT> [--name NAME] [--model MODEL]
//! ```
//!
//! Set one of: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use meerkat::{AgentBuilder, AgentEvent, AgentFactory, CompositeDispatcher, MemoryTaskStore};
use meerkat_comms::MessageKind;
use meerkat_comms::agent::CommsToolDispatcher;
use meerkat_comms::{PeerMeta, TrustedPeer};
use meerkat_core::AgentLlmClient;
use meerkat_store::{JsonlStore, SessionFilter, SessionStore, StoreAdapter};
use meerkat_tools::builtin::shell::ShellConfig;
use tokio::sync::mpsc;

use mdm_tux::{
    CMD_PREFIX, CommsNode, HEARTBEAT_PREFIX, STREAM_PREFIX, auto_detect, build_llm_client,
    detect_provider, load_or_generate_keypair, register_with_backoff,
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
        eprintln!("Usage: target <HOST:PORT> [--name NAME] [--model MODEL]");
        eprintln!("Set one of: ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY");
        std::process::exit(1);
    }
    let host_addr = &args[0];
    let name = find_flag(&args, "--name")
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
    let (model, provider) = match find_flag(&args, "--model") {
        Some(m) => {
            let p = detect_provider(&m);
            (m, p.to_string())
        }
        None => match auto_detect() {
            Some((m, p, _)) => (m, p),
            None => bail!("set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY"),
        },
    };

    let data_dir = PathBuf::from(format!("/tmp/mdm-target-{name}"));

    // ── 1. Load or generate identity ──────────────────────────────────────────
    let keypair = load_or_generate_keypair(&data_dir.join("identity")).await?;

    // ── 2. Create comms node + bind a random free port ────────────────────────
    let node = CommsNode::new(keypair);
    let comms_listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let comms_port = comms_listener.local_addr()?.port();
    let comms_addr = format!("0.0.0.0:{comms_port}");
    drop(comms_listener);
    node.listen(&comms_addr).await?;

    println!("=== MDM Target: {name} ===");
    println!("comms     : tcp://0.0.0.0:{comms_port}");
    println!("provider  : {provider} ({model})");

    // ── 3. Build tool dispatcher (shell + comms) ──────────────────────────────
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let factory = AgentFactory::new(&session_dir);

    let mut tools = build_tools(&factory, &data_dir, &node).await?;

    // ── 4. Build agent (auto-resume most recent session) ──────────────────────
    let llm: Arc<dyn AgentLlmClient> = build_llm_client(&factory, &model, &provider).await?;
    let system_prompt = SYSTEM_PROMPT.replace("{name}", &name);

    // Two store handles over the same directory:
    // - jsonl_store: for list/load (session management commands)
    // - store: wrapped in StoreAdapter for the agent's AgentSessionStore
    let jsonl_store = JsonlStore::new(session_dir.clone());
    jsonl_store.init().await?;
    let agent_store = JsonlStore::new(session_dir);
    agent_store.init().await?;
    let store = Arc::new(StoreAdapter::new(Arc::new(agent_store)));

    let mut agent = build_agent_fresh_or_resume(
        &jsonl_store, &llm, &tools, &store, &model, &system_prompt, None,
    )
    .await;

    // ── 5. Reconnection loop ─────────────────────────────────────────────────
    let host_comms_port: u16 = host_addr
        .rsplit(':')
        .next()
        .and_then(|s| s.parse().ok())
        .context("invalid HOST:PORT")?;
    let host_base = host_addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_addr);
    let reg_addr = format!("{host_base}:{}", host_comms_port + 1);

    // Discover our own IP as seen from the host by making a UDP "connection"
    // (no traffic sent) and reading the local address. Falls back to 127.0.0.1.
    let local_ip = {
        let probe = std::net::UdpSocket::bind("0.0.0.0:0").ok();
        probe
            .and_then(|s| s.connect(format!("{host_base}:{host_comms_port}")).ok().map(|_| s))
            .and_then(|s| s.local_addr().ok())
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|| "127.0.0.1".into())
    };
    let advertised_addr = format!("tcp://{local_ip}:{comms_port}");

    let router = node.router.clone();
    let mut node = node;

    loop {
        // (Re-)register with TUX (retries with exponential backoff)
        eprintln!("[target] registering with {reg_addr} ...");
        let resp =
            register_with_backoff(&reg_addr, &name, &node.pubkey_string(), &advertised_addr)
                .await;
        eprintln!("[target] paired with host '{}' ({})", resp.name, resp.pubkey);

        let resp_name = resp.name.clone();

        let host_pubkey = match meerkat_comms::identity::PubKey::from_peer_id(&resp.pubkey) {
            Ok(pk) => pk,
            Err(e) => {
                eprintln!("[target] bad host pubkey: {e} — retrying registration");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };

        // Remove ALL stale entries for this peer name before adding fresh one.
        // Multiple entries accumulate from identity regeneration or restarts;
        // router.send() resolves by name, so any stale entry could win.
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

        // Spawn heartbeat (pings TUX every 10s, triggers disconnect on 3 failures)
        let hb = spawn_heartbeat(router.clone(), "tux", disconnect_tx.clone());

        eprintln!("[target] ready — waiting for commands\n");

        // Run inbox loop (exits on disconnect signal or DISMISS command)
        run_inbox_loop(
            &mut agent,
            &mut node,
            &router,
            disconnect_rx,
            disconnect_tx,
            &jsonl_store,
            &llm,
            &mut tools,
            &store,
            &model,
            &system_prompt,
            &factory,
            &data_dir,
        )
        .await;

        hb.abort();
        eprintln!("[target] disconnected — reconnecting...");
    }
}

// ── Inbox loop with select!-based disconnect ─────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_inbox_loop(
    agent: &mut meerkat::DynAgent,
    node: &mut CommsNode,
    router: &Arc<meerkat_comms::Router>,
    mut disconnect_rx: tokio::sync::watch::Receiver<bool>,
    disconnect_tx: tokio::sync::watch::Sender<bool>,
    jsonl_store: &JsonlStore,
    llm: &Arc<dyn AgentLlmClient>,
    tools: &mut Arc<dyn meerkat_core::AgentToolDispatcher>,
    store: &Arc<StoreAdapter<JsonlStore>>,
    model: &str,
    system_prompt: &str,
    factory: &AgentFactory,
    data_dir: &std::path::Path,
) {
    loop {
        // select! over inbox message and disconnect signal
        let msg = tokio::select! {
            msg = node.recv_message() => match msg {
                Some(m) => m,
                None => break, // inbox closed
            },
            _ = disconnect_rx.changed() => {
                if *disconnect_rx.borrow() { break; }
                continue;
            }
        };

        if is_dismiss(&msg) {
            eprintln!("[target] received DISMISS — shutting down");
            std::process::exit(0);
        }

        let sender = msg.from_peer.clone();

        // Check for slash commands (__CMD__ prefix)
        if let meerkat_comms::agent::types::CommsContent::Message { ref body, .. } = msg.content
            && let Some(cmd) = body.strip_prefix(CMD_PREFIX)
        {
            let cmd = cmd.trim();
            eprintln!("[target] command: {cmd}");

            let response = handle_command(
                cmd, jsonl_store, llm, tools, store, model, system_prompt, agent,
                factory, data_dir, node,
            )
            .await;

            let _ = router
                .send(&sender, MessageKind::Message { body: response, blocks: None })
                .await;
            continue;
        }

        eprintln!("[target] received message from '{sender}'");

        // Normal message: run agent with streaming + disconnect-aware forwarder
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);
        let fwd_router = router.clone();
        let fwd_sender = sender.clone();
        let fwd_disconnect_tx = disconnect_tx.clone();
        let fwd_handle = tokio::spawn(async move {
            let mut consecutive_failures = 0u32;
            while let Some(event) = event_rx.recv().await {
                if should_forward(&event) {
                    let Ok(json) = serde_json::to_string(&event) else {
                        continue;
                    };
                    let body = format!("{STREAM_PREFIX}{json}");
                    match fwd_router
                        .send(&fwd_sender, MessageKind::Message { body, blocks: None })
                        .await
                    {
                        Ok(_) => consecutive_failures = 0,
                        Err(_) => {
                            consecutive_failures += 1;
                            if consecutive_failures >= 3 {
                                let _ = fwd_disconnect_tx.send(true);
                            }
                        }
                    }
                }
            }
        });

        let input = msg.to_user_message_text();
        if let Err(e) = agent.run_with_events(input.into(), event_tx).await {
            eprintln!("[target] agent error: {e}");
        }

        let _ = fwd_handle.await;

        // Check disconnect flag after agent run completes
        if *disconnect_rx.borrow() {
            break;
        }
    }
}

// ── Heartbeat ────────────────────────────────────────────────────────────────

/// Spawn a background heartbeat task that pings the peer every 10 seconds.
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
            let body = format!("{HEARTBEAT_PREFIX}ping");
            match router
                .send(&peer, MessageKind::Message { body, blocks: None })
                .await
            {
                Ok(_) => consecutive_failures = 0,
                Err(_) => {
                    consecutive_failures += 1;
                    eprintln!(
                        "[target] heartbeat failed ({consecutive_failures}/3)"
                    );
                    if consecutive_failures >= 3 {
                        let _ = disconnect_tx.send(true);
                        break;
                    }
                }
            }
        }
    })
}

// ── Command handling ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_command(
    cmd: &str,
    jsonl_store: &JsonlStore,
    llm: &Arc<dyn AgentLlmClient>,
    tools: &mut Arc<dyn meerkat_core::AgentToolDispatcher>,
    store: &Arc<StoreAdapter<JsonlStore>>,
    model: &str,
    system_prompt: &str,
    agent: &mut meerkat::DynAgent,
    factory: &AgentFactory,
    data_dir: &std::path::Path,
    node: &CommsNode,
) -> String {
    if cmd == "NEW_SESSION" {
        // Rebuild tools with a fresh session-scoped dispatcher so background
        // shell jobs and task tools are bound to the new session.
        match build_tools(factory, data_dir, node).await {
            Ok(t) => *tools = t,
            Err(e) => return format!("Failed to rebuild tools: {e}"),
        }
        *agent = AgentBuilder::new()
            .model(model)
            .system_prompt(system_prompt)
            .build(llm.clone(), tools.clone(), store.clone() as _)
            .await;
        let sid = agent.session().id();
        eprintln!("[target] new session: {sid}");
        format!("New session started: {sid}")
    } else if cmd == "LIST_SESSIONS" {
        match jsonl_store.list(SessionFilter::default()).await {
            Ok(mut sessions) => {
                sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
                if sessions.is_empty() {
                    "No saved sessions.".into()
                } else {
                    let current_id = agent.session().id().to_string();
                    let mut out = String::from("**Sessions:**\n");
                    for (i, s) in sessions.iter().enumerate().take(20) {
                        let age = s
                            .updated_at
                            .elapsed()
                            .map(format_duration)
                            .unwrap_or_else(|_| "?".into());
                        let marker = if s.id.to_string() == current_id {
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
        // Parse as 1-based index
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
                let meta = &sessions[idx];
                match jsonl_store.load(&meta.id).await {
                    Ok(Some(session)) => {
                        let msg_count = session.messages().len();
                        // Rebuild tools with fresh session scope
                        match build_tools(factory, data_dir, node).await {
                            Ok(t) => *tools = t,
                            Err(e) => return format!("Failed to rebuild tools: {e}"),
                        }
                        *agent = AgentBuilder::new()
                            .model(model)
                            .system_prompt(system_prompt)
                            .resume_session(session)
                            .build(llm.clone(), tools.clone(), store.clone() as _)
                            .await;
                        eprintln!("[target] resumed session {} ({msg_count} messages)", meta.id);
                        format!("Resumed session {} ({msg_count} messages)", meta.id)
                    }
                    Ok(None) => format!("Session {} not found on disk", meta.id),
                    Err(e) => format!("Error loading session: {e}"),
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

/// Build an agent, auto-resuming the most recent session if one exists.
async fn build_agent_fresh_or_resume(
    jsonl_store: &JsonlStore,
    llm: &Arc<dyn AgentLlmClient>,
    tools: &Arc<dyn meerkat_core::AgentToolDispatcher>,
    store: &Arc<StoreAdapter<JsonlStore>>,
    model: &str,
    system_prompt: &str,
    specific_session: Option<meerkat_core::session::Session>,
) -> meerkat::DynAgent {
    // If a specific session was requested, use it
    if let Some(session) = specific_session {
        let msg_count = session.messages().len();
        eprintln!("[target] resuming session with {msg_count} messages");
        return AgentBuilder::new()
            .model(model)
            .system_prompt(system_prompt)
            .resume_session(session)
            .build(llm.clone(), tools.clone(), store.clone() as _)
            .await;
    }

    // Try to auto-resume the most recent session
    if let Ok(mut sessions) = jsonl_store.list(SessionFilter::default()).await {
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        if let Some(latest) = sessions.first()
            && let Ok(Some(session)) = jsonl_store.load(&latest.id).await
        {
            let msg_count = session.messages().len();
            eprintln!(
                "[target] auto-resuming session {} ({msg_count} messages)",
                latest.id
            );
            return AgentBuilder::new()
                .model(model)
                .system_prompt(system_prompt)
                .resume_session(session)
                .build(llm.clone(), tools.clone(), store.clone() as _)
                .await;
        }
    }

    // Fresh session
    eprintln!("[target] starting fresh session");
    AgentBuilder::new()
        .model(model)
        .system_prompt(system_prompt)
        .build(llm.clone(), tools.clone(), store.clone() as _)
        .await
}

// ── Tool building ─────────────────────────────────────────────────────────────

/// Build a fresh tools dispatcher with a new session-scoped `JobManager`.
/// Called at startup and on `/new` / `/resume` to ensure shell background
/// jobs and task tools are bound to the current session.
async fn build_tools(
    factory: &AgentFactory,
    data_dir: &std::path::Path,
    node: &CommsNode,
) -> anyhow::Result<Arc<dyn meerkat_core::AgentToolDispatcher>> {
    let shell_config = ShellConfig {
        restrict_to_project: false,
        ..ShellConfig::with_project_root(data_dir.to_path_buf())
    };
    let mut builtin_config = meerkat::BuiltinToolConfig::default();
    builtin_config.policy.enable.insert("shell".into());
    builtin_config.policy.enable.insert("shell_job_cancel".into());

    let session_id = meerkat_core::types::SessionId::new().to_string();
    let ops_registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry> =
        Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());

    let builtin: CompositeDispatcher = factory
        .build_composite_dispatcher(
            Arc::new(MemoryTaskStore::new()),
            &builtin_config,
            None,
            Some(shell_config),
            None,
            Some(session_id),
            Some(ops_registry),
            false,
        )
        .await
        .context("build composite dispatcher")?;

    Ok(Arc::new(CommsToolDispatcher::with_inner(
        node.router.clone(),
        node.trusted.clone(),
        Arc::new(builtin),
    )))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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

fn is_dismiss(msg: &meerkat_comms::agent::types::CommsMessage) -> bool {
    match &msg.content {
        meerkat_comms::agent::types::CommsContent::Message { body, .. } => {
            body.trim().eq_ignore_ascii_case("DISMISS")
        }
        _ => false,
    }
}

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}
