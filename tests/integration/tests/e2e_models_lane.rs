//! Live per-model catalog validation lane.
//!
//! For every model in [`meerkat_models::catalog`], this lane exercises the
//! capabilities advertised by that model's [`ModelCapabilities`] row against
//! the real provider API. Each test iterates the catalog, skips models whose
//! provider key is absent, and reports per-model pass/fail.
//!
//! Lane: `e2e-models` (ignored by default). Invoke with
//! `./scripts/repo-cargo e2e-models` / `make e2e-models` with provider API
//! keys in the environment. The lane is **not** in the CI required-gate list —
//! it is intended as a pre-release / on-demand check.
//!
//! The lane is intentionally narrower than `e2e-live`: there is no agent
//! loop, session store, or runtime harness here — we talk directly to the
//! `LlmClient` per provider, keeping failures localized to a single
//! (model, capability) pair.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use futures::StreamExt;
use meerkat_client::{
    AnthropicClient, GeminiClient, LlmDoneOutcome, LlmEvent, LlmRequest, OpenAiClient,
    types::LlmClient,
};
use meerkat_core::{Message, UserMessage};
use meerkat_models::{
    CatalogEntry, ModelCapabilities, ThinkingSupport, all_capabilities, capabilities_for, catalog,
};

// ---------------------------------------------------------------------------
// API key resolution
// ---------------------------------------------------------------------------

fn first_env(vars: &[&str]) -> Option<String> {
    vars.iter().find_map(|name| std::env::var(name).ok())
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

fn provider_api_key(provider: &str) -> Option<String> {
    match provider {
        "anthropic" => anthropic_api_key(),
        "openai" => openai_api_key(),
        "gemini" => gemini_api_key(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Provider client construction
// ---------------------------------------------------------------------------

fn build_client(provider: &str, api_key: String) -> Result<Arc<dyn LlmClient>, String> {
    match provider {
        "anthropic" => Ok(Arc::new(
            AnthropicClient::new(api_key).map_err(|e| format!("AnthropicClient::new: {e:?}"))?,
        )),
        "openai" => Ok(Arc::new(OpenAiClient::new(api_key))),
        "gemini" => Ok(Arc::new(GeminiClient::new(api_key))),
        other => Err(format!("unknown provider '{other}'")),
    }
}

// ---------------------------------------------------------------------------
// Catalog iteration helper
// ---------------------------------------------------------------------------

/// Iterate every catalog entry, skipping models whose provider API key is
/// absent. Reports skipped models via `eprintln!` so the lane output records
/// missing coverage.
fn for_each_catalog_model_with_key() -> impl Iterator<Item = (&'static CatalogEntry, String)> {
    catalog()
        .iter()
        .filter_map(|entry| match provider_api_key(entry.provider) {
            Some(key) => Some((entry, key)),
            None => {
                eprintln!(
                    "SKIP model={} provider={} reason=missing_api_key",
                    entry.id, entry.provider
                );
                None
            }
        })
}

fn caps_for(entry: &CatalogEntry) -> &'static ModelCapabilities {
    capabilities_for(entry.provider, entry.id)
        .unwrap_or_else(|| panic!("no capability row for {}", entry.id))
}

// ---------------------------------------------------------------------------
// Chat roundtrip helper
// ---------------------------------------------------------------------------

/// Run a single chat turn against the given client and return accumulated text
/// or a diagnostic error.
async fn run_chat(
    client: &dyn LlmClient,
    model: &str,
    prompt: &str,
    provider_params: Option<serde_json::Value>,
    max_tokens: u32,
) -> Result<String, String> {
    let messages = vec![Message::User(UserMessage::text(prompt.to_string()))];
    let mut request = LlmRequest::new(model, messages).with_max_tokens(max_tokens);
    if let Some(params) = provider_params {
        // Test helper: project untyped JSON into the per-provider tag at the
        // adapter boundary so legacy e2e fixtures keep working against the
        // typed-provider-params surface.
        use meerkat_core::lifecycle::run_primitive::{
            AnthropicProviderTag, GeminiProviderTag, OpenAiProviderTag, ProviderTag,
        };
        let tag = match client.provider() {
            "anthropic" => AnthropicProviderTag::from_legacy_value(&params)
                .ok()
                .map(ProviderTag::Anthropic),
            "openai" => OpenAiProviderTag::from_legacy_value(&params)
                .ok()
                .map(ProviderTag::OpenAi),
            "gemini" | "google" => GeminiProviderTag::from_legacy_value(&params)
                .ok()
                .map(ProviderTag::Gemini),
            _ => None,
        };
        if let Some(tag) = tag {
            request = request.with_provider_params(tag);
        }
    }

    let mut stream = client.stream(&request);
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::TextDelta { delta, .. }) => text.push_str(&delta),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            }) => return Ok(text),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("provider Done(Error): {error:?}")),
            Ok(_) => {}
            Err(e) => return Err(format!("stream error: {e:?}")),
        }
    }
    Err("stream ended without Done".into())
}

// ---------------------------------------------------------------------------
// Test: chat roundtrip for every catalog model
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[ignore = "lane:e2e-models"]
async fn chat_roundtrip_all_models() {
    let mut failures: Vec<String> = Vec::new();
    let mut ran = 0_usize;

    for (entry, api_key) in for_each_catalog_model_with_key() {
        eprintln!(
            "-- chat_roundtrip model={} provider={}",
            entry.id, entry.provider
        );
        let client = match build_client(entry.provider, api_key) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{}: build_client: {e}", entry.id));
                continue;
            }
        };
        match run_chat(
            client.as_ref(),
            entry.id,
            "Respond with exactly the word: ready",
            None,
            256,
        )
        .await
        {
            Ok(text) if !text.trim().is_empty() => {
                ran += 1;
            }
            Ok(_) => failures.push(format!("{}: empty text", entry.id)),
            Err(e) => failures.push(format!("{}: {e}", entry.id)),
        }
    }

    assert!(
        failures.is_empty(),
        "chat_roundtrip failures ({} ran):\n{}",
        ran,
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Test: each advertised effort level is accepted
// ---------------------------------------------------------------------------

/// Build the provider_params block that targets the given effort level.
///
/// Anthropic: `{ effort: <level> }` → client forwards into `output_config.effort`.
/// OpenAI: `{ reasoning_effort: <level> }` → client forwards into
/// `reasoning.effort` on the Responses API.
/// Gemini: effort levels are not a concept; this test is skipped for Gemini.
fn effort_provider_params(provider: &str, level: &str) -> Option<serde_json::Value> {
    match provider {
        "anthropic" => Some(serde_json::json!({ "effort": level })),
        "openai" => Some(serde_json::json!({ "reasoning_effort": level })),
        _ => None,
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "lane:e2e-models"]
async fn effort_levels_accepted() {
    let mut failures: Vec<String> = Vec::new();
    let mut tried = 0_usize;

    for (entry, api_key) in for_each_catalog_model_with_key() {
        let caps = caps_for(entry);
        if caps.effort_levels.is_empty() {
            continue;
        }
        // Skip `none` — sending reasoning_effort=none is redundant with the
        // default on GPT-5.4 and doesn't prove the enum advertises correctly.
        let levels: Vec<&&str> = caps
            .effort_levels
            .iter()
            .filter(|lvl| **lvl != "none")
            .collect();
        if levels.is_empty() {
            continue;
        }
        let client = match build_client(entry.provider, api_key) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{}: build_client: {e}", entry.id));
                continue;
            }
        };
        for level in levels {
            let params = match effort_provider_params(entry.provider, level) {
                Some(p) => p,
                None => continue,
            };
            eprintln!(
                "-- effort model={} provider={} effort={}",
                entry.id, entry.provider, level
            );
            tried += 1;
            match run_chat(
                client.as_ref(),
                entry.id,
                "Respond with exactly the word: ready",
                Some(params),
                256,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => failures.push(format!("{} effort={level}: {e}", entry.id)),
            }
        }
    }

    assert!(
        failures.is_empty(),
        "effort_levels_accepted failures ({tried} tried):\n{}",
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Test: advertised thinking modes are accepted
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[ignore = "lane:e2e-models"]
async fn thinking_modes_per_capability() {
    let mut failures: Vec<String> = Vec::new();
    let mut tried = 0_usize;

    for (entry, api_key) in for_each_catalog_model_with_key() {
        let caps = caps_for(entry);
        let configs: Vec<(&'static str, serde_json::Value)> = match caps.thinking {
            ThinkingSupport::None | ThinkingSupport::GeminiThinkingLevel => {
                // Gemini thinking goes through a separate knob (thinking_level)
                // that our client does not forward today — skip here to keep
                // the lane narrow.
                continue;
            }
            ThinkingSupport::AnthropicEnabledOnly => vec![(
                "enabled",
                serde_json::json!({
                    "thinking": { "type": "enabled", "budget_tokens": 2048 }
                }),
            )],
            ThinkingSupport::AnthropicAdaptiveOnly => vec![(
                "adaptive",
                serde_json::json!({
                    "thinking": { "type": "adaptive" }
                }),
            )],
            ThinkingSupport::AnthropicAdaptiveAndEnabled => vec![
                (
                    "adaptive",
                    serde_json::json!({
                        "thinking": { "type": "adaptive" }
                    }),
                ),
                (
                    "enabled",
                    serde_json::json!({
                        "thinking": { "type": "enabled", "budget_tokens": 2048 }
                    }),
                ),
            ],
        };

        let client = match build_client(entry.provider, api_key) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{}: build_client: {e}", entry.id));
                continue;
            }
        };
        for (mode, params) in configs {
            eprintln!(
                "-- thinking model={} provider={} mode={}",
                entry.id, entry.provider, mode
            );
            tried += 1;
            // Thinking requires max_tokens > budget_tokens. The enabled mode
            // asks for budget_tokens: 2048, so allow some headroom for the
            // actual response.
            match run_chat(
                client.as_ref(),
                entry.id,
                "Think briefly, then answer 'ready'.",
                Some(params),
                4096,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => failures.push(format!("{} thinking={mode}: {e}", entry.id)),
            }
        }
    }

    assert!(
        failures.is_empty(),
        "thinking_modes_per_capability failures ({tried} tried):\n{}",
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Meta test: ensure every catalog model has a capability row.
//
// This is the cheap, always-runnable half of the lane. It runs even without
// any provider API keys because it is a pure metadata check. It stays
// `#[ignore]`-gated with the rest of the lane so running `cargo test` on the
// integration crate doesn't drag a live-credentialed lane into regular CI.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "lane:e2e-models"]
fn every_catalog_model_has_capability_row() {
    for entry in catalog() {
        let caps = capabilities_for(entry.provider, entry.id);
        assert!(
            caps.is_some(),
            "catalog model {} has no capability row",
            entry.id
        );
    }
    // Every capability row also maps back to a catalog entry.
    for caps in all_capabilities() {
        let entry = catalog()
            .iter()
            .find(|e| e.provider == caps.provider && e.id == caps.id);
        assert!(
            entry.is_some(),
            "capability row {} has no catalog entry",
            caps.id
        );
    }
}
