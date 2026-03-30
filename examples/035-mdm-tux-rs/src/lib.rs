//! Shared infrastructure for the MDM TUX example.
//!
//! Provides provider auto-detection, a lightweight JSON registration protocol
//! (so target + tux can pair without manual key exchange), and a comms inbox
//! receiver that uses live trusted peers for dynamic registration.

use anyhow::Context as _;
use meerkat::{AgentFactory, AnthropicClient, GeminiClient, OpenAiClient};
use meerkat_comms::agent::spawn_tcp_listener;
use meerkat_comms::agent::types::CommsMessage;
use meerkat_comms::identity::Keypair;
use meerkat_comms::router::CommsConfig;
use meerkat_comms::{
    Inbox, InboxItem, InboxSender, PeerMeta, Router, TrustedPeer, TrustedPeers,
};
use meerkat_core::AgentLlmClient;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

// ── Provider auto-detection ───────────────────────────────────────────────────

/// Probe env vars in priority order and return `(model, provider, api_key)`.
///
/// Checks `ANTHROPIC_API_KEY` → `OPENAI_API_KEY` → `GEMINI_API_KEY`.
pub fn auto_detect() -> Option<(String, String, String)> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        return Some(("claude-sonnet-4-6".into(), "anthropic".into(), k));
    }
    if let Ok(k) = std::env::var("OPENAI_API_KEY") {
        return Some(("gpt-5.2".into(), "openai".into(), k));
    }
    if let Ok(k) = std::env::var("GEMINI_API_KEY") {
        return Some(("gemini-3.1-flash-lite".into(), "gemini".into(), k));
    }
    None
}

/// Infer provider from model name prefix.
pub fn detect_provider(model: &str) -> &'static str {
    if model.starts_with("gpt-")
        || model.starts_with("o1-")
        || model.starts_with("o3-")
        || model.starts_with("o4-")
    {
        "openai"
    } else if model.starts_with("gemini-") {
        "gemini"
    } else {
        "anthropic"
    }
}

/// Return the env-var name that holds the API key for `provider`.
pub fn api_key_env_var(provider: &str) -> &'static str {
    match provider {
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        _ => "ANTHROPIC_API_KEY",
    }
}

/// Build an `Arc<dyn AgentLlmClient>` for the given model + provider.
pub async fn build_llm_client(
    factory: &AgentFactory,
    model: &str,
    provider: &str,
) -> anyhow::Result<Arc<dyn AgentLlmClient>> {
    let key_var = api_key_env_var(provider);
    let key = std::env::var(key_var)
        .with_context(|| format!("{key_var} not set (required for provider '{provider}')"))?;

    let adapter: Arc<dyn AgentLlmClient> = match provider {
        "openai" => Arc::new(
            factory
                .build_llm_adapter(Arc::new(OpenAiClient::new(key)), model)
                .await,
        ),
        "gemini" => Arc::new(
            factory
                .build_llm_adapter(Arc::new(GeminiClient::new(key)), model)
                .await,
        ),
        _ => Arc::new(
            factory
                .build_llm_adapter(Arc::new(AnthropicClient::new(key)?), model)
                .await,
        ),
    };
    Ok(adapter)
}

// ── Comms node (Router + Inbox, supports dynamic peers) ───────────────────────

/// A lightweight comms node that supports dynamic peer registration.
///
/// Unlike `CommsManager` (which snapshots trusted peers at construction and
/// never updates the inbox filter), `CommsNode` uses the router's live
/// `Arc<RwLock<TrustedPeers>>` for every `recv_message()` call.
pub struct CommsNode {
    pub router: Arc<Router>,
    pub trusted: Arc<RwLock<TrustedPeers>>,
    inbox: Inbox,
    inbox_sender: InboxSender,
    keypair: Arc<Keypair>,
}

impl CommsNode {
    pub fn new(keypair: Keypair) -> Self {
        let (inbox, inbox_sender) = Inbox::new();
        let router = Arc::new(Router::new(
            keypair.clone(),
            TrustedPeers::default(),
            CommsConfig::default(),
            inbox_sender.clone(),
            true,
        ));
        let trusted = router.shared_trusted_peers();
        let keypair = router.keypair_arc();
        Self { router, trusted, inbox, inbox_sender, keypair }
    }

    /// Start the TCP comms listener on `addr`.
    pub async fn listen(&self, addr: &str) -> anyhow::Result<()> {
        spawn_tcp_listener(addr, self.keypair.clone(), self.trusted.clone(), self.inbox_sender.clone())
            .await
            .context("spawn TCP comms listener")?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Ok(())
    }

    /// Add a trusted peer (both send-side and receive-side updated immediately).
    pub fn add_peer(&self, peer: TrustedPeer) {
        self.router.add_trusted_peer(peer);
    }

    /// Receive the next message from a trusted peer. Blocks until one arrives.
    /// Uses the live trusted-peer list, so dynamically added peers are accepted.
    pub async fn recv_message(&mut self) -> Option<CommsMessage> {
        loop {
            let item = self.inbox.recv().await?;
            let peers = self.trusted.read();
            if let Some(msg) = CommsMessage::from_inbox_item(&item, &peers, true) {
                return Some(msg);
            }
            // Log dropped messages for debugging
            match &item {
                InboxItem::External { envelope } => {
                    let peer_id = envelope.from.to_peer_id();
                    let known = peers.peers.iter().any(|p| p.pubkey == envelope.from);
                    eprintln!(
                        "[comms] dropped item from {peer_id} (known={known}, \
                         kind={}, trusted_count={})",
                        match &envelope.kind {
                            meerkat_comms::MessageKind::Message { .. } => "message",
                            meerkat_comms::MessageKind::Request { .. } => "request",
                            meerkat_comms::MessageKind::Response { .. } => "response",
                            meerkat_comms::MessageKind::Ack { .. } => "ack",
                        },
                        peers.peers.len()
                    );
                }
                _ => eprintln!("[comms] dropped non-external item"),
            }
        }
    }

    /// Receive the next raw inbox item (including from untrusted senders).
    /// Used during the registration handshake.
    pub async fn recv_raw(&mut self) -> Option<InboxItem> {
        self.inbox.recv().await
    }

    pub fn pubkey_string(&self) -> String {
        self.keypair.public_key().to_peer_id()
    }
}

// ── Registration protocol ─────────────────────────────────────────────────────
//
// A simple JSON-over-TCP handshake on port+1 so that clients can pair with the
// host without manual pubkey exchange.
//
// Client → Host: {"name":"mac","pubkey":"ed25519:...","comms_addr":"tcp://1.2.3.4:9200"}\n
// Host → Client: {"name":"tux","pubkey":"ed25519:..."}\n

#[derive(Serialize, Deserialize)]
pub struct RegRequest {
    pub name: String,
    pub pubkey: String,
    pub comms_addr: String,
}

#[derive(Serialize, Deserialize)]
pub struct RegResponse {
    pub name: String,
    pub pubkey: String,
    /// If set, registration was permanently rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Host: listen for client registrations on `reg_addr` (typically comms port + 1).
/// Each accepted client is added to `node.trusted` and appended to `targets`.
pub async fn run_registration_server(
    reg_addr: String,
    node_pubkey: String,
    trusted: Arc<RwLock<TrustedPeers>>,
    new_target_tx: tokio::sync::mpsc::Sender<(String, String)>, // (name, addr) for TUI
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(&reg_addr).await.with_context(|| format!("bind reg {reg_addr}"))?;
    loop {
        let (stream, _peer) = listener.accept().await?;
        let pubkey = node_pubkey.clone();
        let trusted = trusted.clone();
        let tx = new_target_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_registration(stream, &pubkey, &trusted, &tx).await {
                eprintln!("[reg] error: {e}");
            }
        });
    }
}

async fn handle_registration(
    mut stream: TcpStream,
    host_pubkey: &str,
    trusted: &Arc<RwLock<TrustedPeers>>,
    new_target_tx: &tokio::sync::mpsc::Sender<(String, String)>,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let req: RegRequest = serde_json::from_str(line.trim())?;

    let pubkey = meerkat_comms::identity::PubKey::from_peer_id(&req.pubkey)
        .with_context(|| format!("bad pubkey from '{}'", req.name))?;

    // Atomic check-then-insert under a single write lock. The lock is
    // dropped before any async I/O (parking_lot guards are !Send).
    let conflict = {
        let mut peers = trusted.write();
        let existing = peers.peers.iter().find(|p| p.name == req.name);
        if let Some(old) = existing
            && old.pubkey != pubkey
        {
            true
        } else {
            peers.upsert(TrustedPeer {
                name: req.name.clone(),
                pubkey,
                addr: req.comms_addr.clone(),
                meta: PeerMeta::default(),
            });
            false
        }
    }; // lock dropped here

    if conflict {
        let resp = RegResponse {
            name: "tux".into(),
            pubkey: host_pubkey.into(),
            error: Some(format!(
                "name '{}' already registered by a different machine — \
                 use --name to pick a unique name",
                req.name
            )),
        };
        let mut resp_json = serde_json::to_string(&resp)?;
        resp_json.push('\n');
        writer.write_all(resp_json.as_bytes()).await?;
        writer.flush().await?;
        eprintln!("[reg] rejected '{}': name conflict", req.name);
        return Ok(());
    }

    let resp = RegResponse { name: "tux".into(), pubkey: host_pubkey.into(), error: None };
    let mut resp_json = serde_json::to_string(&resp)?;
    resp_json.push('\n');
    writer.write_all(resp_json.as_bytes()).await?;
    writer.flush().await?;

    eprintln!("[reg] registered target '{}' at {}", req.name, req.comms_addr);
    let _ = new_target_tx.send((req.name, req.comms_addr)).await;
    Ok(())
}

/// Client: register with the host. Returns the host's pubkey.
pub async fn register_with_host(
    reg_addr: &str,
    name: &str,
    our_pubkey: &str,
    our_comms_addr: &str,
) -> anyhow::Result<RegResponse> {
    let mut stream = TcpStream::connect(reg_addr)
        .await
        .with_context(|| format!("connect to registration server at {reg_addr}"))?;

    let req = RegRequest {
        name: name.into(),
        pubkey: our_pubkey.into(),
        comms_addr: our_comms_addr.into(),
    };
    let mut req_json = serde_json::to_string(&req)?;
    req_json.push('\n');
    stream.write_all(req_json.as_bytes()).await?;
    stream.flush().await?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let resp: RegResponse = serde_json::from_str(line.trim())?;
    Ok(resp)
}

// ── Streaming protocol ────────────────────────────────────────────────────────

/// Prefix for streaming event messages.
/// Body format: `__STREAM__\n<json-serialized AgentEvent>`.
pub const STREAM_PREFIX: &str = "__STREAM__\n";

/// Prefix for slash-command messages (TUX → target).
/// Body format: `__CMD__\n<COMMAND> [args...]`.
pub const CMD_PREFIX: &str = "__CMD__\n";

/// Prefix for heartbeat pings (target → TUX).
pub const HEARTBEAT_PREFIX: &str = "__HEARTBEAT__\n";

/// Register with backoff retry. Retries forever with exponential backoff
/// (1s → 2s → 4s → ... → 30s cap) until the host accepts the registration.
/// Register with backoff retry. Retries transient errors forever.
/// Returns `Err` for permanent rejections (e.g. name conflict).
pub async fn register_with_backoff(
    reg_addr: &str,
    name: &str,
    pubkey: &str,
    comms_addr: &str,
) -> anyhow::Result<RegResponse> {
    let mut delay = 1u64;
    loop {
        match register_with_host(reg_addr, name, pubkey, comms_addr).await {
            Ok(resp) => {
                // Check for permanent rejection from the server
                if let Some(ref err) = resp.error {
                    anyhow::bail!("registration rejected: {err}");
                }
                return Ok(resp);
            }
            Err(e) => {
                eprintln!("[target] registration failed: {e} — retrying in {delay}s");
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                delay = (delay * 2).min(30);
            }
        }
    }
}

// ── Keypair helper ────────────────────────────────────────────────────────────

/// Load or generate a persistent Ed25519 keypair from `dir`.
pub async fn load_or_generate_keypair(dir: &std::path::Path) -> anyhow::Result<Keypair> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("create keypair dir {}", dir.display()))?;
    Keypair::load_or_generate(dir)
        .await
        .with_context(|| format!("load_or_generate keypair in {}", dir.display()))
}
