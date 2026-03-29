//! Shared types for the MDM TUX example.
//!
//! Config structs use plain `String` for pubkeys (e.g. `"ed25519:..."`)
//! because `PubKey` serialises as CBOR bytes, which is incompatible with
//! TOML. Conversion to `TrustedPeer` happens via `PubKey::from_peer_id`
//! at startup.

use anyhow::Context as _;
use meerkat::{AgentFactory, AnthropicClient, GeminiClient, OpenAiClient};
use meerkat_comms::identity::{Keypair, PubKey};
use meerkat_comms::{PeerMeta, TrustedPeer, TrustedPeers};
use meerkat_core::AgentLlmClient;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;

// ── Config structs ────────────────────────────────────────────────────────────

/// Configuration for the `target` binary (managed machine).
#[derive(Debug, Deserialize)]
pub struct TargetConfig {
    /// Agent name (used in system prompt and peer discovery).
    pub name: String,
    /// TCP address to listen on, e.g. `"0.0.0.0:4748"`.
    pub listen_addr: String,
    /// LLM model name, e.g. `"claude-sonnet-4-6"`, `"gpt-5.2"`, `"gemini-3.1-flash-lite"`.
    pub model: String,
    /// Directory for keypair + session store persistence.
    pub data_dir: String,
    /// Peers trusted to send this agent commands (typically just TUX).
    #[serde(default)]
    pub trusted_peers: Vec<PeerEntry>,
}

/// Configuration for the `tux` binary (controller machine).
#[derive(Debug, Deserialize)]
pub struct TuxConfig {
    /// TCP address TUX listens on for incoming replies.
    pub listen_addr: String,
    /// LLM model for the hive agent (only used in Hive mode).
    pub model: String,
    /// Directory for keypair persistence.
    pub data_dir: String,
    /// Known managed targets.
    #[serde(default)]
    pub targets: Vec<TargetEntry>,
}

/// A peer entry in `target.toml` (trusted controllers).
#[derive(Debug, Deserialize, Clone)]
pub struct PeerEntry {
    pub name: String,
    /// `"ed25519:..."` string as printed by the remote binary at startup.
    pub pubkey: String,
    /// Transport address, e.g. `"tcp://192.168.1.50:4747"`.
    pub addr: String,
}

/// A target entry in `tux.toml` (managed machines).
#[derive(Debug, Deserialize, Clone)]
pub struct TargetEntry {
    pub name: String,
    /// `"ed25519:..."` string as printed by `target` at startup.
    pub pubkey: String,
    /// Transport address, e.g. `"tcp://192.168.1.100:4748"`.
    pub addr: String,
}

// ── Provider detection ────────────────────────────────────────────────────────

/// Infer the provider from a model name prefix.
///
/// - `gpt-*`, `o1-*`, `o3-*`, `o4-*` → `"openai"`
/// - `gemini-*` → `"gemini"`
/// - anything else → `"anthropic"`
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

/// Return the environment variable name that holds the API key for a provider.
pub fn api_key_env_var(provider: &str) -> &'static str {
    match provider {
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        _ => "ANTHROPIC_API_KEY",
    }
}

/// Build an `Arc<dyn AgentLlmClient>` for the given model + provider.
///
/// Reads the appropriate API key from the environment.
/// Returns an error (with a clear message) if the key is absent.
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
                .build_llm_adapter(Arc::new(OpenAiClient::new(key)?), model)
                .await,
        ),
        "gemini" => Arc::new(
            factory
                .build_llm_adapter(Arc::new(GeminiClient::new(key)?), model)
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

// ── Keypair helpers ───────────────────────────────────────────────────────────

/// Load or generate a persistent Ed25519 keypair from `dir`.
///
/// The keypair is stored as `<dir>/ed25519_secret_key` (CBOR binary).
/// On first run a new key is generated; subsequent runs load the same key.
pub async fn load_or_generate_keypair(dir: &Path) -> anyhow::Result<Keypair> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("create keypair dir {}", dir.display()))?;
    Keypair::load_or_generate(dir)
        .await
        .with_context(|| format!("load_or_generate keypair in {}", dir.display()))
}

// ── Peer conversion helpers ───────────────────────────────────────────────────

/// Convert a `PeerEntry` (from `target.toml`) to a `TrustedPeer`.
pub fn peer_entry_to_trusted(e: &PeerEntry) -> anyhow::Result<TrustedPeer> {
    let pubkey = PubKey::from_peer_id(&e.pubkey)
        .with_context(|| format!("invalid pubkey '{}' for peer '{}'", e.pubkey, e.name))?;
    Ok(TrustedPeer { name: e.name.clone(), pubkey, addr: e.addr.clone(), meta: PeerMeta::default() })
}

/// Convert a `TargetEntry` (from `tux.toml`) to a `TrustedPeer`.
pub fn target_entry_to_trusted(e: &TargetEntry) -> anyhow::Result<TrustedPeer> {
    let pubkey = PubKey::from_peer_id(&e.pubkey)
        .with_context(|| format!("invalid pubkey '{}' for target '{}'", e.pubkey, e.name))?;
    Ok(TrustedPeer { name: e.name.clone(), pubkey, addr: e.addr.clone(), meta: PeerMeta::default() })
}

/// Build a `TrustedPeers` list from a `TargetEntry` slice.
pub fn targets_to_trusted_peers(targets: &[TargetEntry]) -> anyhow::Result<TrustedPeers> {
    let peers = targets
        .iter()
        .map(target_entry_to_trusted)
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(TrustedPeers { peers })
}
