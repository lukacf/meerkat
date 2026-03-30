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
    CMD_PREFIX, CommsNode, STREAM_PREFIX, auto_detect, build_llm_client, detect_provider,
    load_or_generate_keypair, register_with_host,
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

    // ── 3. Register with TUX ──────────────────────────────────────────────────
    let host_comms_port: u16 = host_addr
        .rsplit(':')
        .next()
        .and_then(|s| s.parse().ok())
        .context("invalid HOST:PORT")?;
    let host_base = host_addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_addr);
    let reg_addr = format!("{host_base}:{}", host_comms_port + 1);
    let advertised_addr = format!("tcp://{host_base}:{comms_port}");

    println!("registering with {reg_addr} ...");
    let resp =
        register_with_host(&reg_addr, &name, &node.pubkey_string(), &advertised_addr).await?;
    println!("paired with host '{}' ({})", resp.name, resp.pubkey);

    let host_pubkey =
        meerkat_comms::identity::PubKey::from_peer_id(&resp.pubkey).context("bad host pubkey")?;
    node.add_peer(TrustedPeer {
        name: resp.name,
        pubkey: host_pubkey,
        addr: format!("tcp://{host_addr}"),
        meta: PeerMeta::default(),
    });

    // ── 4. Build tool dispatcher (shell + comms) ──────────────────────────────
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;
    let factory = AgentFactory::new(&session_dir);

    let shell_config = ShellConfig {
        restrict_to_project: false,
        ..ShellConfig::with_project_root(data_dir)
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

    let tools: Arc<_> = Arc::new(CommsToolDispatcher::with_inner(
        node.router.clone(),
        node.trusted.clone(),
        Arc::new(builtin),
    ));

    // ── 5. Build agent (auto-resume most recent session) ──────────────────────
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

    // ── 6. Listen for commands with streaming ─────────────────────────────────
    println!("\nReady. Waiting for commands...\n");

    let router = node.router.clone();
    let mut node = node;

    loop {
        let Some(msg) = node.recv_message().await else {
            break;
        };
        if is_dismiss(&msg) {
            println!("[target] received DISMISS — shutting down");
            break;
        }

        let sender = msg.from_peer.clone();

        // Check for slash commands (__CMD__ prefix)
        if let meerkat_comms::agent::types::CommsContent::Message { ref body, .. } = msg.content
            && let Some(cmd) = body.strip_prefix(CMD_PREFIX)
        {
                let cmd = cmd.trim();
                eprintln!("[target] command: {cmd}");

                let response = handle_command(
                    cmd,
                    &jsonl_store,
                    &llm,
                    &tools,
                    &store,
                    &model,
                    &system_prompt,
                    &mut agent,
                )
                .await;

                let _ = router
                    .send(
                        &sender,
                        MessageKind::Message { body: response, blocks: None },
                    )
                    .await;
                continue;
        }

        eprintln!("[target] received message from '{sender}'");

        // Normal message: run agent with streaming
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);
        let fwd_router = router.clone();
        let fwd_sender = sender.clone();
        let fwd_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if should_forward(&event) {
                    let Ok(json) = serde_json::to_string(&event) else {
                        continue;
                    };
                    let body = format!("{STREAM_PREFIX}{json}");
                    let _ = fwd_router
                        .send(&fwd_sender, MessageKind::Message { body, blocks: None })
                        .await;
                }
            }
        });

        let input = msg.to_user_message_text();
        if let Err(e) = agent.run_with_events(input.into(), event_tx).await {
            eprintln!("[target] agent error: {e}");
        }

        let _ = fwd_handle.await;
    }

    Ok(())
}

// ── Command handling ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_command(
    cmd: &str,
    jsonl_store: &JsonlStore,
    llm: &Arc<dyn AgentLlmClient>,
    tools: &Arc<impl meerkat_core::AgentToolDispatcher + 'static>,
    store: &Arc<StoreAdapter<JsonlStore>>,
    model: &str,
    system_prompt: &str,
    agent: &mut meerkat::DynAgent,
) -> String {
    if cmd == "NEW_SESSION" {
        *agent = AgentBuilder::new()
            .model(model)
            .system_prompt(system_prompt)
            .build(llm.clone(), tools.clone() as _, store.clone() as _)
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
                        *agent = AgentBuilder::new()
                            .model(model)
                            .system_prompt(system_prompt)
                            .resume_session(session)
                            .build(llm.clone(), tools.clone() as _, store.clone() as _)
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
    tools: &Arc<impl meerkat_core::AgentToolDispatcher + 'static>,
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
            .build(llm.clone(), tools.clone() as _, store.clone() as _)
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
                .build(llm.clone(), tools.clone() as _, store.clone() as _)
                .await;
        }
    }

    // Fresh session
    eprintln!("[target] starting fresh session");
    AgentBuilder::new()
        .model(model)
        .system_prompt(system_prompt)
        .build(llm.clone(), tools.clone() as _, store.clone() as _)
        .await
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
