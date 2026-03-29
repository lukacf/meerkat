//! # 035 — MDM TUX: Target Agent (Rust)
//!
//! This binary runs on a managed computer. It:
//! - Generates or loads a persistent Ed25519 keypair
//! - Prints its public key (for pairing with TUX)
//! - Listens for TCP comms messages from TUX
//! - Executes each incoming command with shell tools
//! - Sends the result back to the sender via the `send` comms tool
//!
//! The target goes straight into the inbox-wait loop — no LLM call is made
//! until the first command actually arrives.
//!
//! ## Supported providers
//! Detected from the model name prefix (see `detect_provider` in lib.rs).
//! Set the matching env var: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY`.
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --bin target -- target.toml.example
//! # or
//! OPENAI_API_KEY=... cargo run --bin target -- target.toml.example  # with model = "gpt-5.2"
//! ```
//! Copy the printed pubkey into `tux.toml` before starting TUX.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use meerkat::{AgentBuilder, AgentFactory, CompositeDispatcher, MemoryTaskStore};
use meerkat_comms::TrustedPeers;
use meerkat_comms::agent::{CommsManager, CommsManagerConfig, CommsToolDispatcher, spawn_tcp_listener};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::builtin::shell::ShellConfig;

use mdm_tux::{TargetConfig, build_llm_client, detect_provider, load_or_generate_keypair, peer_entry_to_trusted};

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

    let config_path = std::env::args().nth(1).unwrap_or_else(|| "target.toml".into());
    let raw = tokio::fs::read_to_string(&config_path)
        .await
        .with_context(|| format!("read config '{config_path}'"))?;
    let config: TargetConfig = toml::from_str(&raw)
        .with_context(|| format!("parse config '{config_path}'"))?;

    let provider = detect_provider(&config.model);
    let data_dir = PathBuf::from(&config.data_dir);

    // ── 1. Load or generate identity ─────────────────────────────────────────
    let keypair = load_or_generate_keypair(&data_dir.join("identity")).await?;
    let pubkey = keypair.public_key();
    println!("=== MDM Target: {} ===", config.name);
    println!("my pubkey : {}", pubkey.to_peer_id());
    println!("listening : tcp://{}", config.listen_addr);
    println!("provider  : {provider} ({})", config.model);
    println!("(add pubkey + addr to tux.toml, then start TUX)\n");

    // ── 2. Build trusted peers ────────────────────────────────────────────────
    let trusted_peers: Vec<_> = config.trusted_peers
        .iter()
        .map(peer_entry_to_trusted)
        .collect::<anyhow::Result<_>>()?;
    let trusted = TrustedPeers { peers: trusted_peers };

    // ── 3. Create CommsManager ────────────────────────────────────────────────
    let comms_cfg = CommsManagerConfig::with_keypair(keypair).trusted_peers(trusted);
    let mut comms = CommsManager::new(comms_cfg)?;

    // ── 4. Start TCP listener ─────────────────────────────────────────────────
    // Use the router's shared trusted peers so listener + router + dispatcher
    // all see the same live peer list.
    let trusted_shared = comms.router().shared_trusted_peers();
    let _listener = spawn_tcp_listener(
        &config.listen_addr,
        comms.keypair_arc(),
        trusted_shared.clone(),
        comms.inbox_sender().clone(),
    )
    .await
    .context("spawn TCP listener")?;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // ── 5. Build tool dispatcher (shell + comms) ──────────────────────────────
    let session_dir = data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_dir).await?;

    let task_store = Arc::new(MemoryTaskStore::new());
    let factory = AgentFactory::new(&session_dir);

    let shell_config = ShellConfig {
        restrict_to_project: false, // full system access for the managed machine
        ..ShellConfig::with_project_root(data_dir.clone())
    };

    // build_composite_dispatcher returns concrete CompositeDispatcher (Sized),
    // required for CommsToolDispatcher::with_inner<T: Sized>.
    let builtin: CompositeDispatcher = factory
        .build_composite_dispatcher(
            task_store,
            &Default::default(),
            None,
            Some(shell_config),
            None,
            None,
            None,
            false,
        )
        .await
        .context("build composite dispatcher")?;

    let tools = Arc::new(CommsToolDispatcher::with_inner(
        comms.router().clone(),
        trusted_shared,
        Arc::new(builtin),
    ));

    // ── 6. Build agent ────────────────────────────────────────────────────────
    let llm = build_llm_client(&factory, &config.model, provider).await?;

    let store = Arc::new(JsonlStore::new(session_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    let system_prompt = SYSTEM_PROMPT.replace("{name}", &config.name);
    let mut agent = AgentBuilder::new()
        .model(&config.model)
        .system_prompt(system_prompt)
        .build(llm, tools, store)
        .await;

    // ── 7. Stay alive — wake on inbox, execute, respond ───────────────────────
    // No initial LLM call: the target goes straight into the wait loop.
    // The agent only calls the LLM when a real command arrives in the inbox.
    println!("Ready. Waiting for commands from TUX...\n");
    loop {
        let Some(msg) = comms.recv_message().await else { break };
        if let Err(e) = agent.run(msg.to_user_message_text().into()).await {
            eprintln!("[target] agent error: {e}");
        }
    }

    Ok(())
}
