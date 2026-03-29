//! # 035 — MDM TUX: Target Agent
//!
//! Runs on managed machines. Registers with a TUX host automatically,
//! then serves as a directly-controlled agent with streaming output.
//!
//! Each incoming message is a new turn on the same agent session (history
//! accumulates). Agent events (text deltas, tool calls) are streamed back
//! to TUX as individual comms messages with the `__STREAM__` prefix.
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
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::builtin::shell::ShellConfig;
use tokio::sync::mpsc;

use mdm_tux::{
    CommsNode, STREAM_PREFIX, auto_detect, build_llm_client, detect_provider,
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

    // Provide a session ID + ops lifecycle registry so the JobManager
    // is session-bound and supports background shell execution.
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

    let tools = Arc::new(CommsToolDispatcher::with_inner(
        node.router.clone(),
        node.trusted.clone(),
        Arc::new(builtin),
    ));

    // ── 5. Build agent ────────────────────────────────────────────────────────
    let llm = build_llm_client(&factory, &model, &provider).await?;

    let store = Arc::new(JsonlStore::new(session_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    let mut agent = AgentBuilder::new()
        .model(&model)
        .system_prompt(SYSTEM_PROMPT.replace("{name}", &name))
        .build(llm, tools, store)
        .await;

    // ── 6. Listen for commands with streaming ─────────────────────────────────
    // Each message is a new turn on the same session (history accumulates).
    // Agent events are forwarded to the sender as __STREAM__-prefixed messages.
    // DISMISS from a peer exits the process.
    println!("\nReady. Waiting for commands...\n");

    // We need a mutable reference to node for recv_message, but also need
    // the router for the event forwarder. Clone the router Arc before the loop.
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
        eprintln!("[target] received message from '{sender}'");

        // Create a per-turn event channel + forwarder
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);
        let fwd_router = router.clone();
        let fwd_sender = sender.clone();
        let fwd_handle = tokio::spawn(async move {
            let mut count = 0u32;
            while let Some(event) = event_rx.recv().await {
                if should_forward(&event) {
                    let Ok(json) = serde_json::to_string(&event) else {
                        continue;
                    };
                    let body = format!("{STREAM_PREFIX}{json}");
                    if let Err(e) = fwd_router
                        .send(&fwd_sender, MessageKind::Message { body, blocks: None })
                        .await
                    {
                        eprintln!("[target] failed to send event to '{fwd_sender}': {e}");
                    } else {
                        count += 1;
                    }
                }
            }
            eprintln!("[target] forwarded {count} events to '{fwd_sender}'");
        });

        let input = msg.to_user_message_text();
        eprintln!("[target] running agent...");
        match agent.run_with_events(input.into(), event_tx).await {
            Ok(result) => eprintln!("[target] agent done ({} chars)", result.text.len()),
            Err(e) => eprintln!("[target] agent error: {e}"),
        }

        // event_tx is dropped here → fwd_handle will finish draining
        let _ = fwd_handle.await;
    }

    Ok(())
}

/// Select which events to stream back to TUX.
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
