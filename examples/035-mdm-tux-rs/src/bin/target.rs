//! # 035 — MDM TUX: Target Agent
//!
//! Runs on managed machines. Registers with a TUX host automatically,
//! then executes shell commands received via comms.
//!
//! ```text
//! target <HOST:PORT> [--name NAME] [--model MODEL]
//! ```
//!
//! - `HOST:PORT` — the TUX host's comms port (registration runs on PORT+1)
//! - `--name`    — agent name (default: system hostname)
//! - `--model`   — LLM model (default: auto-detect from API key env vars)
//!
//! Set one of: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use meerkat::{AgentBuilder, AgentFactory, CompositeDispatcher, MemoryTaskStore};
use meerkat_comms::agent::CommsToolDispatcher;
use meerkat_comms::{PeerMeta, TrustedPeer};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::builtin::shell::ShellConfig;

use mdm_tux::{CommsNode, auto_detect, build_llm_client, detect_provider, load_or_generate_keypair, register_with_host};

const SYSTEM_PROMPT: &str = "\
You are a managed system agent named '{name}'. You receive commands from TUX (the controller).
For each incoming message:
1. Execute the requested task using your shell tools.
2. Collect the output.
3. Send the result back to the sender using the 'send' comms tool.
Always respond after completing a command. Keep responses concise.";

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
        Some(m) => { let p = detect_provider(&m); (m, p.to_string()) }
        None => match auto_detect() {
            Some((m, p, _)) => (m, p),
            None => bail!("set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY"),
        },
    };

    let data_dir = PathBuf::from(format!("/tmp/mdm-target-{name}"));

    // ── 1. Load or generate identity ──────────────────────────────────────────
    let keypair = load_or_generate_keypair(&data_dir.join("identity")).await?;

    // ── 2. Create comms node + bind a random free port ────────────────────────
    let mut node = CommsNode::new(keypair);
    let comms_listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let comms_port = comms_listener.local_addr()?.port();
    let comms_addr = format!("0.0.0.0:{comms_port}");
    drop(comms_listener);
    node.listen(&comms_addr).await?;

    println!("=== MDM Target: {name} ===");
    println!("comms     : tcp://0.0.0.0:{comms_port}");
    println!("provider  : {provider} ({model})");

    // ── 3. Register with TUX ──────────────────────────────────────────────────
    // Registration runs on host's comms port + 1.
    let host_comms_port: u16 = host_addr.rsplit(':').next()
        .and_then(|s| s.parse().ok())
        .context("invalid HOST:PORT")?;
    let host_base = host_addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_addr);
    let reg_addr = format!("{host_base}:{}", host_comms_port + 1);

    // The public comms_addr we advertise must be reachable from the host.
    // For same-machine testing: use the host's IP base.
    let advertised_addr = format!("tcp://{host_base}:{comms_port}");

    println!("registering with {reg_addr} ...");
    let resp = register_with_host(&reg_addr, &name, &node.pubkey_string(), &advertised_addr).await?;
    println!("paired with host '{}' ({})", resp.name, resp.pubkey);

    // Add host to our trusted peers so we accept its future messages.
    let host_pubkey = meerkat_comms::identity::PubKey::from_peer_id(&resp.pubkey)
        .context("bad host pubkey")?;
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
    let builtin: CompositeDispatcher = factory
        .build_composite_dispatcher(
            Arc::new(MemoryTaskStore::new()),
            &Default::default(),
            None,
            Some(shell_config),
            None, None, None, false,
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

    // ── 6. Listen for commands ────────────────────────────────────────────────
    // No initial LLM call — go straight into the inbox loop.
    // DISMISS from a peer exits the process (matches CommsAgent::run_stay_alive).
    println!("\nReady. Waiting for commands...\n");
    loop {
        let Some(msg) = node.recv_message().await else { break };
        if is_dismiss(&msg) {
            println!("[target] received DISMISS — shutting down");
            break;
        }
        if let Err(e) = agent.run(msg.to_user_message_text().into()).await {
            eprintln!("[target] agent error: {e}");
        }
    }

    Ok(())
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
