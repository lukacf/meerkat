//! Model discovery for the `/model` picker.
//!
//! [`list_models`] asks the session provider's list-models endpoint which
//! models the configured account can use; [`with_current`] puts the session's
//! current model in front. The list comes from the endpoint only — there is
//! no fallback to the curated `meerkat-models` catalog.
//!
//! The API key is only ever placed in request headers — never in a URL, log
//! line or error message.

use std::collections::HashSet;
use std::time::Duration;

use meerkat_core::Provider;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::credentials::ProviderCredentials;
use crate::endpoints::{Dialect, Endpoint};
use crate::pricing::ModelPrice;

/// Default Anthropic API base (mirrors `meerkat-client`).
const ANTHROPIC_DEFAULT_BASE: &str = "https://api.anthropic.com";
/// Anthropic API version header value (mirrors `meerkat-client`).
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Default OpenAI API base (mirrors `meerkat-client`).
const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com";
/// Default Gemini API base (mirrors `meerkat-client`).
const GEMINI_DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com";
/// Upper bound on followed pages, so a misbehaving endpoint cannot loop forever.
const MAX_PAGES: usize = 20;
/// Maximum number of characters of an error body kept in [`ListModelsError::Status`].
const MAX_ERROR_BODY_CHARS: usize = 300;

/// One model the picker can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// Model identifier as accepted by the provider (e.g. `claude-sonnet-4-6`).
    pub id: String,
    /// Human-readable name, when known.
    pub display_name: Option<String>,
    /// Where this entry came from.
    pub source: ModelSource,
    /// Maximum input context window in tokens, as reported by the endpoint.
    pub context_window: Option<u64>,
    /// Maximum output tokens per response, as reported by the endpoint.
    pub max_output_tokens: Option<u64>,
}

/// Origin of a [`ModelEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    /// Reported by the provider's list-models endpoint.
    Provider,
    /// The model the session is currently using.
    Current,
}

/// Failure to list models from a provider.
#[derive(Debug, thiserror::Error)]
pub enum ListModelsError {
    /// Transport failure (connect, timeout, TLS, …). The URL is stripped.
    #[error("network error: {0}")]
    Http(reqwest::Error),
    /// The endpoint answered with a non-success status.
    #[error("HTTP {status}: {body}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Truncated, single-line response body.
        body: String,
    },
    /// The response body did not match the expected shape.
    #[error("unexpected response: {0}")]
    Decode(String),
    /// The provider has no supported list-models endpoint.
    #[error("listing models is not supported for provider `{}`", .0.as_str())]
    Unsupported(Provider),
}

impl From<reqwest::Error> for ListModelsError {
    fn from(err: reqwest::Error) -> Self {
        Self::Http(err.without_url())
    }
}

/// Fetch the models available to `creds` from the provider's list endpoint.
///
/// `timeout` applies to each HTTP request (connect + response).
///
/// # Errors
///
/// Returns [`ListModelsError::Unsupported`] for [`Provider::SelfHosted`] and [`Provider::Other`], and
/// transport, status or decoding errors otherwise.
pub async fn list_models(
    creds: &ProviderCredentials,
    timeout: Duration,
) -> Result<Vec<ModelEntry>, ListModelsError> {
    let base = |default: &str| {
        creds
            .base_url
            .as_deref()
            .unwrap_or(default)
            .trim_end_matches('/')
            .to_string()
    };
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout);
    // Like `meerkat-client`: never send loopback endpoints through a proxy.
    if creds.base_url.as_deref().is_some_and(is_loopback_url) {
        builder = builder.no_proxy();
    }
    let client = builder.build()?;
    match creds.provider {
        Provider::Anthropic => {
            list_anthropic(&client, &base(ANTHROPIC_DEFAULT_BASE), &creds.api_key).await
        }
        Provider::OpenAI => list_openai(&client, &base(OPENAI_DEFAULT_BASE), &creds.api_key).await,
        Provider::Gemini => list_gemini(&client, &base(GEMINI_DEFAULT_BASE), &creds.api_key).await,
        other @ (Provider::SelfHosted | Provider::Other) => {
            Err(ListModelsError::Unsupported(other))
        }
    }
}

/// One endpoint's section of the `/model` picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelGroup {
    /// Endpoint name.
    pub endpoint: String,
    /// Its dialect.
    pub dialect: Dialect,
    /// Its base URL override.
    pub base_url: Option<String>,
    /// Models, in display order (the current one first in its group).
    pub models: Vec<ModelEntry>,
    /// Why the list is missing or incomplete (the fetch failed).
    pub warning: Option<String>,
}

/// List the models of `endpoint`. For OpenAI-dialect endpoints with a base
/// URL, LiteLLM's `GET /model/info` is also tried to fill in context windows
/// and output limits; any failure there is ignored.
///
/// # Errors
///
/// See [`list_models`].
pub async fn list_endpoint_models(
    endpoint: &Endpoint,
    timeout: Duration,
) -> Result<Vec<ModelEntry>, ListModelsError> {
    Ok(list_endpoint(endpoint, timeout).await?.models)
}

/// What listing one endpoint yields: its models and, for a LiteLLM proxy,
/// the per-model prices from `/model/info` (USD per million tokens).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EndpointListing {
    /// The endpoint's models.
    pub models: Vec<ModelEntry>,
    /// Prices by model id, first deployment first.
    pub prices: Vec<(String, ModelPrice)>,
}

/// [`list_endpoint_models`] plus the LiteLLM prices, when there are any.
///
/// # Errors
///
/// See [`list_models`].
pub async fn list_endpoint(
    endpoint: &Endpoint,
    timeout: Duration,
) -> Result<EndpointListing, ListModelsError> {
    let creds = endpoint.credentials();
    let mut listing = EndpointListing {
        models: list_models(&creds, timeout).await?,
        prices: Vec::new(),
    };
    if endpoint.dialect == Dialect::OpenAi
        && let Some(base) = &endpoint.base_url
    {
        match litellm_model_info(base, &creds.api_key, timeout).await {
            Ok(info) => apply_litellm_info(&mut listing, info),
            Err(err) => {
                tracing::debug!(endpoint = %endpoint.name, error = %err, "no LiteLLM /model/info");
            }
        }
    }
    Ok(listing)
}

/// [`list_endpoint`] for every endpoint in parallel; results in input order.
pub async fn list_all_endpoints(
    endpoints: &[Endpoint],
    timeout: Duration,
) -> Vec<(String, Result<EndpointListing, ListModelsError>)> {
    let fetches = endpoints.iter().map(|endpoint| async move {
        (
            endpoint.name.clone(),
            list_endpoint(endpoint, timeout).await,
        )
    });
    futures::future::join_all(fetches).await
}

/// Put `current` first (tagged [`ModelSource::Current`]) and otherwise keep
/// the endpoint order of `live`, dropping duplicate ids.
///
/// When `current` is reported by the endpoint, its display name and context
/// window are carried over. Pass an empty `live` when the endpoint call
/// failed: the picker then offers only the current model (show
/// [`fetch_warning`] alongside it).
pub fn with_current(current: &str, live: Vec<ModelEntry>) -> Vec<ModelEntry> {
    let mut seen: HashSet<String> = HashSet::with_capacity(live.len() + 1);
    let mut out = Vec::with_capacity(live.len() + 1);
    if !current.is_empty() {
        let known = live.iter().find(|entry| entry.id == current);
        out.push(ModelEntry {
            id: current.to_string(),
            display_name: known.and_then(|entry| entry.display_name.clone()),
            source: ModelSource::Current,
            context_window: known.and_then(|entry| entry.context_window),
            max_output_tokens: known.and_then(|entry| entry.max_output_tokens),
        });
        seen.insert(current.to_string());
    }
    for entry in live {
        if seen.insert(entry.id.clone()) {
            out.push(ModelEntry {
                source: ModelSource::Provider,
                ..entry
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// LiteLLM
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LiteLlmInfoPage {
    #[serde(default)]
    data: Vec<LiteLlmModel>,
}

#[derive(Debug, Deserialize)]
struct LiteLlmModel {
    model_name: String,
    #[serde(default)]
    model_info: Option<LiteLlmModelInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct LiteLlmModelInfo {
    #[serde(default)]
    max_input_tokens: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<u64>,
    #[serde(default, deserialize_with = "crate::pricing::lenient_f64")]
    input_cost_per_token: Option<f64>,
    #[serde(default, deserialize_with = "crate::pricing::lenient_f64")]
    output_cost_per_token: Option<f64>,
    #[serde(default, deserialize_with = "crate::pricing::lenient_f64")]
    cache_read_input_token_cost: Option<f64>,
    #[serde(default, deserialize_with = "crate::pricing::lenient_f64")]
    cache_creation_input_token_cost: Option<f64>,
}

impl LiteLlmModelInfo {
    /// Prices per million tokens, when input and output are both set.
    fn price(&self) -> Option<ModelPrice> {
        let price = ModelPrice::from_per_token(
            self.input_cost_per_token?,
            self.output_cost_per_token?,
            self.cache_read_input_token_cost,
            self.cache_creation_input_token_cost,
        );
        price.is_valid().then_some(price)
    }
}

async fn litellm_model_info(
    base: &str,
    api_key: &str,
    timeout: Duration,
) -> Result<LiteLlmInfoPage, ListModelsError> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout);
    if is_loopback_url(base) {
        builder = builder.no_proxy();
    }
    let client = builder.build()?;
    let url = format!("{}/model/info", base.trim_end_matches('/'));
    fetch_json(client.get(url).bearer_auth(api_key)).await
}

/// Fill missing windows from LiteLLM's model info (first deployment with a
/// value wins) and collect its prices.
fn apply_litellm_info(listing: &mut EndpointListing, info: LiteLlmInfoPage) {
    for model in info.data {
        let Some(details) = model.model_info else {
            continue;
        };
        if let Some(price) = details.price() {
            listing.prices.push((model.model_name.clone(), price));
        }
        for entry in listing
            .models
            .iter_mut()
            .filter(|e| e.id == model.model_name)
        {
            if entry.context_window.is_none() {
                entry.context_window = details.max_input_tokens;
            }
            if entry.max_output_tokens.is_none() {
                entry.max_output_tokens = details.max_output_tokens;
            }
        }
    }
}

/// One-line, secret-free warning shown when [`list_models`] failed.
pub fn fetch_warning(err: &ListModelsError) -> String {
    format!("could not list models from the provider: {err}")
}

// ---------------------------------------------------------------------------
// HTTP plumbing
// ---------------------------------------------------------------------------

/// Send `request`, map non-success statuses, and decode the body as `T`.
async fn fetch_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, ListModelsError> {
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(ListModelsError::Status {
            status: status.as_u16(),
            body: summarize_body(&body),
        });
    }
    decode(&body)
}

/// Whether `url` points at this machine (`localhost`, `127.0.0.0/8`, `::1`).
pub(crate) fn is_loopback_url(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn decode<T: DeserializeOwned>(body: &str) -> Result<T, ListModelsError> {
    serde_json::from_str(body).map_err(|err| ListModelsError::Decode(err.to_string()))
}

/// Collapse whitespace and truncate an error body for display.
fn summarize_body(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > MAX_ERROR_BODY_CHARS {
        let truncated: String = collapsed.chars().take(MAX_ERROR_BODY_CHARS).collect();
        format!("{truncated}…")
    } else {
        collapsed
    }
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AnthropicPage {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    last_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicModel {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    max_input_tokens: Option<u64>,
    #[serde(default)]
    max_tokens: Option<u64>,
}

impl From<AnthropicModel> for ModelEntry {
    fn from(model: AnthropicModel) -> Self {
        Self {
            id: model.id,
            display_name: model.display_name,
            source: ModelSource::Provider,
            context_window: model.max_input_tokens,
            max_output_tokens: model.max_tokens,
        }
    }
}

async fn list_anthropic(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
) -> Result<Vec<ModelEntry>, ListModelsError> {
    let url = format!("{base}/v1/models");
    let mut entries = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = client
            .get(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .query(&[("limit", "1000")]);
        if let Some(after_id) = &after {
            request = request.query(&[("after_id", after_id.as_str())]);
        }
        let page: AnthropicPage = fetch_json(request).await?;
        entries.extend(page.data.into_iter().map(ModelEntry::from));
        match (page.has_more, page.last_id) {
            (true, Some(last)) => after = Some(last),
            _ => break,
        }
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// OpenAI
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct OpenAiPage {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(default)]
    created: i64,
}

/// Whether an OpenAI model id belongs to an obviously non-chat family.
fn is_openai_non_chat(id: &str) -> bool {
    const CONTAINS: &[&str] = &["embedding", "tts", "moderation", "realtime", "transcribe"];
    const PREFIXES: &[&str] = &["whisper", "dall-e", "omni-moderation", "text-"];
    let id = id.to_ascii_lowercase();
    CONTAINS.iter().any(|needle| id.contains(needle))
        || PREFIXES.iter().any(|prefix| id.starts_with(prefix))
}

/// Keep chat-capable models, newest first (ties broken by id).
fn openai_entries(page: OpenAiPage) -> Vec<ModelEntry> {
    let mut models: Vec<OpenAiModel> = page
        .data
        .into_iter()
        .filter(|model| !is_openai_non_chat(&model.id))
        .collect();
    models.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| a.id.cmp(&b.id)));
    models
        .into_iter()
        .map(|model| ModelEntry {
            id: model.id,
            display_name: None,
            source: ModelSource::Provider,
            context_window: None,
            max_output_tokens: None,
        })
        .collect()
}

async fn list_openai(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
) -> Result<Vec<ModelEntry>, ListModelsError> {
    let request = client.get(format!("{base}/v1/models")).bearer_auth(api_key);
    let page: OpenAiPage = fetch_json(request).await?;
    Ok(openai_entries(page))
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPage {
    #[serde(default)]
    models: Vec<GeminiModel>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiModel {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
    #[serde(default)]
    input_token_limit: Option<u64>,
    #[serde(default)]
    output_token_limit: Option<u64>,
}

/// Convert a Gemini model, keeping only `generateContent`-capable ones.
fn gemini_entry(model: GeminiModel) -> Option<ModelEntry> {
    if !model
        .supported_generation_methods
        .iter()
        .any(|method| method == "generateContent")
    {
        return None;
    }
    let id = model
        .name
        .strip_prefix("models/")
        .unwrap_or(&model.name)
        .to_string();
    Some(ModelEntry {
        id,
        display_name: model.display_name,
        source: ModelSource::Provider,
        context_window: model.input_token_limit,
        max_output_tokens: model.output_token_limit,
    })
}

async fn list_gemini(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
) -> Result<Vec<ModelEntry>, ListModelsError> {
    let url = format!("{base}/v1beta/models");
    let mut entries = Vec::new();
    let mut page_token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = client
            .get(&url)
            .header("x-goog-api-key", api_key)
            .query(&[("pageSize", "200")]);
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token.as_str())]);
        }
        let page: GeminiPage = fetch_json(request).await?;
        entries.extend(page.models.into_iter().filter_map(gemini_entry));
        match page.next_page_token {
            Some(token) if !token.is_empty() => page_token = Some(token),
            _ => break,
        }
    }
    Ok(entries)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn entry(id: &str, source: ModelSource) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            display_name: None,
            source,
            context_window: None,
            max_output_tokens: None,
        }
    }

    #[test]
    fn decodes_anthropic_page_with_pagination() {
        let body = r#"{
            "data": [
                {"type": "model", "id": "claude-opus-4-6", "display_name": "Claude Opus 4.6", "created_at": "2026-02-01T00:00:00Z", "max_input_tokens": 1000000, "max_tokens": 128000, "capabilities": {"thinking": {"supported": true}}},
                {"type": "model", "id": "claude-sonnet-4-6", "display_name": "Claude Sonnet 4.6", "created_at": "2026-01-01T00:00:00Z"}
            ],
            "has_more": true,
            "first_id": "claude-opus-4-6",
            "last_id": "claude-sonnet-4-6"
        }"#;
        let page: AnthropicPage = decode(body).unwrap();
        assert!(page.has_more);
        assert_eq!(page.last_id.as_deref(), Some("claude-sonnet-4-6"));
        let entries: Vec<ModelEntry> = page.data.into_iter().map(ModelEntry::from).collect();
        assert_eq!(entries[0].id, "claude-opus-4-6");
        assert_eq!(entries[0].display_name.as_deref(), Some("Claude Opus 4.6"));
        assert_eq!(entries[0].context_window, Some(1_000_000));
        assert_eq!(entries[0].max_output_tokens, Some(128_000));
        assert_eq!(entries[1].source, ModelSource::Provider);
        assert_eq!(entries[1].context_window, None);
        assert_eq!(entries[1].max_output_tokens, None);
    }

    #[test]
    fn anthropic_last_page_defaults() {
        let page: AnthropicPage = decode(r#"{"data": []}"#).unwrap();
        assert!(!page.has_more);
        assert!(page.last_id.is_none());
    }

    #[test]
    fn decode_error_is_reported() {
        let err = decode::<AnthropicPage>(r#"{"models": []}"#).unwrap_err();
        assert!(matches!(err, ListModelsError::Decode(_)));
    }

    #[test]
    fn decodes_openai_and_filters_non_chat_newest_first() {
        let body = r#"{
            "object": "list",
            "data": [
                {"id": "gpt-5.2", "object": "model", "created": 200, "owned_by": "system"},
                {"id": "text-embedding-3-large", "object": "model", "created": 900, "owned_by": "system"},
                {"id": "gpt-5.4", "object": "model", "created": 300, "owned_by": "system"},
                {"id": "whisper-1", "object": "model", "created": 901, "owned_by": "openai-internal"},
                {"id": "tts-1-hd", "object": "model", "created": 902, "owned_by": "system"},
                {"id": "dall-e-3", "object": "model", "created": 903, "owned_by": "system"},
                {"id": "omni-moderation-latest", "object": "model", "created": 904, "owned_by": "system"},
                {"id": "gpt-4o-realtime-preview", "object": "model", "created": 905, "owned_by": "system"},
                {"id": "gpt-4o-transcribe", "object": "model", "created": 906, "owned_by": "system"},
                {"id": "text-davinci-003", "object": "model", "created": 907, "owned_by": "system"},
                {"id": "gpt-5.3-codex", "object": "model", "created": 300, "owned_by": "system"}
            ]
        }"#;
        let page: OpenAiPage = decode(body).unwrap();
        let ids: Vec<String> = openai_entries(page).into_iter().map(|e| e.id).collect();
        assert_eq!(ids, vec!["gpt-5.3-codex", "gpt-5.4", "gpt-5.2"]);
    }

    #[test]
    fn openai_filter_keeps_chat_models() {
        assert!(!is_openai_non_chat("gpt-5.4"));
        assert!(!is_openai_non_chat("o3-mini"));
        assert!(is_openai_non_chat("text-moderation-stable"));
        assert!(is_openai_non_chat("gpt-4o-mini-tts"));
    }

    #[test]
    fn decodes_gemini_page_strips_prefix_and_filters() {
        let body = r#"{
            "models": [
                {
                    "name": "models/gemini-3.1-pro-preview",
                    "version": "3.1",
                    "displayName": "Gemini 3.1 Pro Preview",
                    "inputTokenLimit": 1048576,
                    "outputTokenLimit": 65536,
                    "supportedGenerationMethods": ["generateContent", "countTokens"]
                },
                {
                    "name": "models/text-embedding-004",
                    "displayName": "Text Embedding 004",
                    "inputTokenLimit": 2048,
                    "supportedGenerationMethods": ["embedContent"]
                }
            ],
            "nextPageToken": "abc123"
        }"#;
        let page: GeminiPage = decode(body).unwrap();
        assert_eq!(page.next_page_token.as_deref(), Some("abc123"));
        let entries: Vec<ModelEntry> = page.models.into_iter().filter_map(gemini_entry).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "gemini-3.1-pro-preview");
        assert_eq!(
            entries[0].display_name.as_deref(),
            Some("Gemini 3.1 Pro Preview")
        );
        assert_eq!(entries[0].context_window, Some(1_048_576));
        assert_eq!(entries[0].max_output_tokens, Some(65_536));
    }

    #[test]
    fn gemini_last_page_has_no_token() {
        let page: GeminiPage = decode("{}").unwrap();
        assert!(page.models.is_empty());
        assert!(page.next_page_token.is_none());
    }

    #[test]
    fn with_current_puts_current_first_and_keeps_endpoint_order() {
        let mut current = entry("current-x", ModelSource::Provider);
        current.display_name = Some("Current X".to_string());
        current.context_window = Some(42);
        current.max_output_tokens = Some(7);
        let live = vec![
            entry("live-b", ModelSource::Provider),
            current,
            entry("live-a", ModelSource::Provider),
            entry("live-b", ModelSource::Provider),
        ];
        let merged = with_current("current-x", live);
        let pairs: Vec<(&str, ModelSource)> =
            merged.iter().map(|e| (e.id.as_str(), e.source)).collect();
        assert_eq!(
            pairs,
            vec![
                ("current-x", ModelSource::Current),
                ("live-b", ModelSource::Provider),
                ("live-a", ModelSource::Provider),
            ]
        );
        assert_eq!(merged[0].display_name.as_deref(), Some("Current X"));
        assert_eq!(merged[0].context_window, Some(42));
        assert_eq!(merged[0].max_output_tokens, Some(7));
    }

    #[test]
    fn with_current_unknown_to_endpoint_has_no_metadata() {
        let merged = with_current("custom", vec![entry("a", ModelSource::Provider)]);
        assert_eq!(merged[0].id, "custom");
        assert_eq!(merged[0].source, ModelSource::Current);
        assert!(merged[0].display_name.is_none());
        assert!(merged[0].context_window.is_none());
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn failure_shows_only_current_with_one_line_warning() {
        let err = ListModelsError::Status {
            status: 401,
            body: summarize_body("{\n  \"error\": \"invalid x-api-key\"\n}"),
        };
        let merged = with_current("cur", Vec::new());
        let ids: Vec<&str> = merged.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["cur"]);
        let warning = fetch_warning(&err);
        assert!(warning.contains("HTTP 401"));
        assert!(!warning.contains('\n'));
    }

    #[test]
    fn litellm_info_fills_missing_windows() {
        let info: LiteLlmInfoPage = decode(
            r#"{"data":[
                {"model_name":"gpt-x","model_info":{"max_input_tokens":200000,"max_output_tokens":8192,"other":1}},
                {"model_name":"gpt-x","model_info":{"max_input_tokens":1}},
                {"model_name":"no-info"},
                {"model_name":"null-info","model_info":{"max_input_tokens":null}}
            ]}"#,
        )
        .unwrap();
        let mut listing = EndpointListing {
            models: vec![
                entry("gpt-x", ModelSource::Provider),
                entry("no-info", ModelSource::Provider),
            ],
            prices: Vec::new(),
        };
        apply_litellm_info(&mut listing, info);
        let entries = &listing.models;
        assert_eq!(entries[0].context_window, Some(200_000));
        assert_eq!(entries[0].max_output_tokens, Some(8192));
        assert_eq!(entries[1].context_window, None);
        assert!(listing.prices.is_empty());
    }

    #[test]
    fn litellm_info_yields_prices_per_million() {
        let info: LiteLlmInfoPage = decode(
            r#"{"data":[
                {"model_name":"gpt-x","model_info":{"input_cost_per_token":2e-06,"output_cost_per_token":8e-06,"cache_read_input_token_cost":5e-07}},
                {"model_name":"half","model_info":{"input_cost_per_token":1e-06,"output_cost_per_token":null}},
                {"model_name":"odd","model_info":{"input_cost_per_token":"x","output_cost_per_token":1e-06,"max_input_tokens":42}}
            ]}"#,
        )
        .unwrap();
        let mut listing = EndpointListing {
            models: vec![entry("odd", ModelSource::Provider)],
            prices: Vec::new(),
        };
        apply_litellm_info(&mut listing, info);
        assert_eq!(listing.models[0].context_window, Some(42));
        assert_eq!(listing.prices.len(), 1);
        let (id, price) = &listing.prices[0];
        assert_eq!(id, "gpt-x");
        assert!((price.input - 2.0).abs() < 1e-9);
        assert!((price.output - 8.0).abs() < 1e-9);
        assert!((price.cached_input.unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn with_current_empty_current_is_skipped() {
        let merged = with_current("", vec![entry("a", ModelSource::Provider)]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, ModelSource::Provider);
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_url("http://127.0.0.1:8080/"));
        assert!(is_loopback_url("http://localhost:1"));
        assert!(is_loopback_url("http://[::1]:1"));
        assert!(!is_loopback_url("https://api.anthropic.com"));
        assert!(!is_loopback_url("not a url"));
    }

    #[test]
    fn summarize_body_truncates() {
        let long = "x".repeat(MAX_ERROR_BODY_CHARS + 50);
        assert_eq!(
            summarize_body(&long).chars().count(),
            MAX_ERROR_BODY_CHARS + 1
        );
    }

    #[tokio::test]
    async fn other_provider_is_unsupported() {
        let creds = ProviderCredentials {
            provider: Provider::Other,
            api_key: "k".to_string(),
            base_url: None,
        };
        let err = list_models(&creds, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(err, ListModelsError::Unsupported(Provider::Other)));
    }

    /// Serve each canned body to one request in turn; return the base URL and
    /// a handle yielding the raw request heads.
    async fn stub_server(
        bodies: Vec<(u16, &'static str)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut heads = Vec::new();
            for (status, body) in bodies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                heads.push(String::from_utf8_lossy(&buf).to_string());
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
            heads
        });
        (format!("http://{addr}/"), handle)
    }

    #[tokio::test]
    async fn anthropic_follows_pagination_with_headers() {
        let (base, server) = stub_server(vec![
            (
                200,
                r#"{"data":[{"id":"m1","display_name":"M1"}],"has_more":true,"last_id":"m1"}"#,
            ),
            (
                200,
                r#"{"data":[{"id":"m2"}],"has_more":false,"last_id":"m2"}"#,
            ),
        ])
        .await;
        let creds = ProviderCredentials {
            provider: Provider::Anthropic,
            api_key: "sk-test".to_string(),
            base_url: Some(base),
        };
        let entries = list_models(&creds, Duration::from_secs(5)).await.unwrap();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["m1", "m2"]);
        let heads = server.await.unwrap();
        assert!(heads[0].starts_with("GET /v1/models?limit=1000 "));
        assert!(heads[1].starts_with("GET /v1/models?limit=1000&after_id=m1 "));
        let lower = heads[0].to_ascii_lowercase();
        assert!(lower.contains("x-api-key: sk-test"));
        assert!(lower.contains("anthropic-version: 2023-06-01"));
    }

    #[tokio::test]
    async fn gemini_follows_page_token_and_uses_header_key() {
        let (base, server) = stub_server(vec![
            (
                200,
                r#"{"models":[{"name":"models/g1","supportedGenerationMethods":["generateContent"]}],"nextPageToken":"t2"}"#,
            ),
            (
                200,
                r#"{"models":[{"name":"models/g2","supportedGenerationMethods":["generateContent"]}]}"#,
            ),
        ])
        .await;
        let creds = ProviderCredentials {
            provider: Provider::Gemini,
            api_key: "g-key".to_string(),
            base_url: Some(base),
        };
        let entries = list_models(&creds, Duration::from_secs(5)).await.unwrap();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["g1", "g2"]);
        let heads = server.await.unwrap();
        assert!(heads[1].starts_with("GET /v1beta/models?pageSize=200&pageToken=t2 "));
        assert!(!heads[0].contains("key=g-key"));
        assert!(
            heads[0]
                .to_ascii_lowercase()
                .contains("x-goog-api-key: g-key")
        );
    }

    #[tokio::test]
    async fn openai_status_error_is_reported_without_key() {
        let (base, server) = stub_server(vec![(
            401,
            r#"{"error":{"message":"Incorrect API key provided"}}"#,
        )])
        .await;
        let creds = ProviderCredentials {
            provider: Provider::OpenAI,
            api_key: "sk-secret".to_string(),
            base_url: Some(base),
        };
        let err = list_models(&creds, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(matches!(err, ListModelsError::Status { status: 401, .. }));
        assert!(!err.to_string().contains("sk-secret"));
        let heads = server.await.unwrap();
        assert!(heads[0].contains("Bearer sk-secret"));
    }

    async fn live(provider: Provider) {
        let var = match provider {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
            _ => "GEMINI_API_KEY",
        };
        let Some(api_key) = std::env::var(var).ok().filter(|key| !key.is_empty()) else {
            eprintln!("skipping: no API key for {}", provider.as_str());
            return;
        };
        let creds = ProviderCredentials {
            provider,
            api_key,
            base_url: None,
        };
        let entries = list_models(&creds, Duration::from_secs(10)).await.unwrap();
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| e.source == ModelSource::Provider));
    }

    #[tokio::test]
    #[ignore = "e2e: live API, requires ANTHROPIC_API_KEY"]
    async fn e2e_list_models_anthropic() {
        live(Provider::Anthropic).await;
    }

    #[tokio::test]
    #[ignore = "e2e: live API, requires OPENAI_API_KEY"]
    async fn e2e_list_models_openai() {
        live(Provider::OpenAI).await;
    }

    #[tokio::test]
    #[ignore = "e2e: live API, requires GEMINI_API_KEY"]
    async fn e2e_list_models_gemini() {
        live(Provider::Gemini).await;
    }
}
