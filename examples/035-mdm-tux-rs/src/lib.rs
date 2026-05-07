//! Shared infrastructure for the MDM TUX example.
//!
//! Provides provider auto-detection, direct control payloads for comms-based
//! target attach/stream, and the kennel protocol types.

use anyhow::Context as _;
use meerkat_comms::agent::types::{CommsContent, CommsMessage};
use meerkat_comms::identity::Keypair;
use meerkat_core::AgentEvent;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub mod kennel;
pub mod machines;
pub mod rpc_client;
pub use kennel::*;

// ── Provider auto-detection ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Openai,
    Anthropic,
    Gemini,
}

pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5.5";
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-3.1-flash-lite-preview";

/// Probe env vars in priority order and return `(model, provider, api_key)`.
///
/// Checks `OPENAI_API_KEY` → `ANTHROPIC_API_KEY` → `GEMINI_API_KEY`.
pub fn auto_detect() -> Option<(String, ProviderKind, String)> {
    if let Ok(k) = std::env::var("OPENAI_API_KEY") {
        return Some((DEFAULT_OPENAI_MODEL.into(), ProviderKind::Openai, k));
    }
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        return Some((DEFAULT_ANTHROPIC_MODEL.into(), ProviderKind::Anthropic, k));
    }
    if let Ok(k) = std::env::var("GEMINI_API_KEY") {
        return Some((DEFAULT_GEMINI_MODEL.into(), ProviderKind::Gemini, k));
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
        blocks: None,
        handling_mode: None,
    })
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
