//! Shared infrastructure for the MDM TUX example.
//!
//! Provides provider auto-detection, a lightweight JSON registration protocol
//! (so target + tux can pair without manual key exchange), and a comms inbox
//! receiver that uses live trusted peers for dynamic registration.

use anyhow::Context as _;
use meerkat_comms::agent::types::{CommsContent, CommsMessage};
use meerkat_comms::identity::Keypair;
use meerkat_comms::router::CommsConfig;
use meerkat_comms::{Inbox, InboxItem, InboxSender, PeerMeta, Router, TrustedPeer, TrustedPeers};
use meerkat_core::AgentEvent;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub mod kennel;
pub mod machines;
pub use kennel::*;

// ── Provider auto-detection ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    Gemini,
}

/// Probe env vars in priority order and return `(model, provider, api_key)`.
///
/// Checks `ANTHROPIC_API_KEY` → `OPENAI_API_KEY` → `GEMINI_API_KEY`.
pub fn auto_detect() -> Option<(String, ProviderKind, String)> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        return Some(("claude-sonnet-4-6".into(), ProviderKind::Anthropic, k));
    }
    if let Ok(k) = std::env::var("OPENAI_API_KEY") {
        return Some(("gpt-5.2".into(), ProviderKind::Openai, k));
    }
    if let Ok(k) = std::env::var("GEMINI_API_KEY") {
        return Some(("gemini-3.1-flash-lite".into(), ProviderKind::Gemini, k));
    }
    None
}

/// Return the env-var name that holds the API key for `provider`.
pub fn api_key_env_var(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Openai => "OPENAI_API_KEY",
        ProviderKind::Gemini => "GEMINI_API_KEY",
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
    }
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
        Self {
            router,
            trusted,
            inbox,
            inbox_sender,
            keypair,
        }
    }

    /// Start the TCP comms listener on `addr`. Returns the actual bound address
    /// (useful when binding to port 0 for a random free port).
    pub async fn listen(&self, addr: &str) -> anyhow::Result<std::net::SocketAddr> {
        // Bind first so the caller can read the actual port before any TOCTOU gap.
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .context("bind TCP comms listener")?;
        let local_addr = listener.local_addr().context("read bound address")?;

        // Spawn the accept loop using the already-bound listener.
        let keypair = self.keypair.clone();
        let trusted = self.trusted.clone();
        let inbox_sender = self.inbox_sender.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _peer)) => {
                        let keypair = keypair.clone();
                        let trusted = trusted.clone();
                        let inbox_sender = inbox_sender.clone();
                        tokio::spawn(async move {
                            let trusted_snapshot = trusted.read().clone();
                            if let Err(e) = meerkat_comms::handle_connection(
                                stream,
                                true, // require_peer_auth
                                keypair.as_ref(),
                                &trusted_snapshot,
                                &inbox_sender,
                            )
                            .await
                            {
                                tracing::debug!("comms connection error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("comms accept error: {e}");
                    }
                }
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Ok(local_addr)
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DirectRegistrationRejectReason {
    NameConflict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RegResponse {
    Accepted {
        name: String,
        pubkey: String,
    },
    Rejected {
        name: String,
        pubkey: String,
        reason: DirectRegistrationRejectReason,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct DirectRegistrationNotice {
    pub target_id: String,
    pub name: String,
    pub direct_addr: String,
}

/// Host: listen for client registrations on `reg_addr` (typically comms port + 1).
/// Each accepted client is added to `node.trusted` and appended to `targets`.
pub async fn run_registration_server(
    reg_addr: String,
    node_pubkey: String,
    trusted: Arc<RwLock<TrustedPeers>>,
    new_target_tx: tokio::sync::mpsc::Sender<DirectRegistrationNotice>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(&reg_addr)
        .await
        .with_context(|| format!("bind reg {reg_addr}"))?;
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
    new_target_tx: &tokio::sync::mpsc::Sender<DirectRegistrationNotice>,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let req: RegRequest = serde_json::from_str(line.trim())?;

    let pubkey = meerkat_comms::identity::PubKey::from_peer_id(&req.pubkey)
        .with_context(|| format!("bad pubkey from '{}'", req.name))?;

    let registration = {
        let peers = trusted.read();
        let existing = peers.peers.iter().find(|p| p.name == req.name);
        crate::machines::direct_registration::transition(
            crate::machines::direct_registration::Event::Register {
                requested_name: req.name.clone(),
                requested_pubkey: req.pubkey.clone(),
                existing_peer: existing.map(|peer| {
                    crate::machines::direct_registration::ExistingPeer {
                        name: peer.name.clone(),
                        pubkey: peer.pubkey.to_peer_id(),
                    }
                }),
            },
        )?
    };

    if let crate::machines::direct_registration::Effect::Reject { reason, message } = registration {
        let resp = RegResponse::Rejected {
            name: "tux".into(),
            pubkey: host_pubkey.into(),
            reason: match reason {
                crate::machines::direct_registration::RejectReason::NameConflict => {
                    DirectRegistrationRejectReason::NameConflict
                }
            },
            message,
        };
        let mut resp_json = serde_json::to_string(&resp)?;
        resp_json.push('\n');
        writer.write_all(resp_json.as_bytes()).await?;
        writer.flush().await?;
        eprintln!("[reg] rejected '{}': name conflict", req.name);
        return Ok(());
    }

    {
        let mut peers = trusted.write();
        peers.upsert(TrustedPeer {
            name: req.name.clone(),
            pubkey,
            addr: req.comms_addr.clone(),
            meta: PeerMeta::default(),
        });
    }

    let resp = RegResponse::Accepted {
        name: "tux".into(),
        pubkey: host_pubkey.into(),
    };
    let mut resp_json = serde_json::to_string(&resp)?;
    resp_json.push('\n');
    writer.write_all(resp_json.as_bytes()).await?;
    writer.flush().await?;

    eprintln!(
        "[reg] registered target '{}' at {}",
        req.name, req.comms_addr
    );
    let _ = new_target_tx
        .send(DirectRegistrationNotice {
            target_id: req.pubkey,
            name: req.name,
            direct_addr: req.comms_addr,
        })
        .await;
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

pub const DIRECT_CONTROL_INTENT: &str = "mdm.direct";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DirectControlPayload {
    AttachRequest { lease_id: String, target_id: String },
    AttachAck { lease_id: String },
    StreamEvent { event: AgentEvent },
}

pub fn parse_direct_control_message(
    msg: &CommsMessage,
) -> anyhow::Result<Option<DirectControlPayload>> {
    match &msg.content {
        CommsContent::Request { intent, params, .. }
            if intent.as_str() == DIRECT_CONTROL_INTENT =>
        {
            let payload =
                serde_json::from_value(params.clone()).context("decode direct control payload")?;
            Ok(Some(payload))
        }
        _ => Ok(None),
    }
}

pub fn direct_control_request(
    payload: &DirectControlPayload,
) -> anyhow::Result<meerkat_comms::MessageKind> {
    let params = serde_json::to_value(payload).context("encode direct control payload")?;
    Ok(meerkat_comms::MessageKind::Request {
        intent: DIRECT_CONTROL_INTENT.into(),
        params,
    })
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Openai => "openai",
            ProviderKind::Gemini => "gemini",
        };
        f.write_str(text)
    }
}

impl FromStr for ProviderKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::Openai),
            "gemini" => Ok(Self::Gemini),
            other => anyhow::bail!("unsupported provider '{other}'"),
        }
    }
}

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
        // Timeout the entire registration attempt (connect + write + read_line)
        // so a black-holed or unresponsive server doesn't wedge the retry loop.
        let attempt = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            register_with_host(reg_addr, name, pubkey, comms_addr),
        )
        .await;
        match attempt {
            Ok(Ok(resp)) => {
                if let RegResponse::Rejected { message, .. } = &resp {
                    anyhow::bail!("registration rejected: {message}");
                }
                return Ok(resp);
            }
            Ok(Err(e)) => {
                eprintln!("[target] registration failed: {e} — retrying in {delay}s");
            }
            Err(_) => {
                eprintln!("[target] registration timed out — retrying in {delay}s");
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_rejection_roundtrips_with_typed_reason() {
        let response = RegResponse::Rejected {
            name: "tux".into(),
            pubkey: "ed25519:tux".into(),
            reason: DirectRegistrationRejectReason::NameConflict,
            message: "duplicate".into(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let decoded: RegResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, response);
    }
}
