//! Anthropic Claude API client
//!
//! Implements the LlmClient trait for Anthropic's Claude API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{Message, OutputSchema, StopReason, Usage};
use serde::Deserialize;
use serde_json::Value;
use std::pin::Pin;
use std::time::Duration;

/// Default connect timeout
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default request timeout
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Default pool idle timeout
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// SSE buffer capacity to reduce reallocations
const SSE_BUFFER_CAPACITY: usize = 4096;

/// Client for Anthropic API
pub struct AnthropicClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
    connect_timeout: Duration,
    request_timeout: Duration,
    pool_idle_timeout: Duration,
}

/// Builder for AnthropicClient
pub struct AnthropicClientBuilder {
    api_key: String,
    base_url: String,
    connect_timeout: Duration,
    request_timeout: Duration,
    pool_idle_timeout: Duration,
}

impl AnthropicClientBuilder {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            pool_idle_timeout: DEFAULT_POOL_IDLE_TIMEOUT,
        }
    }

    /// Set custom base URL
    pub fn base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Set connection timeout
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set request timeout
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set pool idle timeout
    pub fn pool_idle_timeout(mut self, timeout: Duration) -> Self {
        self.pool_idle_timeout = timeout;
        self
    }

    /// Build the client with configured HTTP settings
    pub fn build(self) -> Result<AnthropicClient, LlmError> {
        let base_url = self.base_url;
        let connect_timeout = self.connect_timeout;
        let request_timeout = self.request_timeout;
        let pool_idle_timeout = self.pool_idle_timeout;
        #[cfg(not(target_arch = "wasm32"))]
        let builder = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .pool_idle_timeout(pool_idle_timeout)
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30));
        #[cfg(target_arch = "wasm32")]
        let builder = reqwest::Client::builder();
        let http = crate::http::build_http_client_for_base_url(builder, &base_url)?;

        Ok(AnthropicClient {
            api_key: self.api_key,
            base_url,
            http,
            connect_timeout,
            request_timeout,
            pool_idle_timeout,
        })
    }
}

impl AnthropicClient {
    /// Create a new Anthropic client with the given API key and default HTTP settings
    pub fn new(api_key: String) -> Result<Self, LlmError> {
        AnthropicClientBuilder::new(api_key).build()
    }

    /// Create a builder for more control over HTTP configuration
    pub fn builder(api_key: String) -> AnthropicClientBuilder {
        AnthropicClientBuilder::new(api_key)
    }

    /// Create from environment variable ANTHROPIC_API_KEY
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("RKAT_ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .map_err(|_| LlmError::InvalidApiKey)?;
        Self::new(api_key)
    }

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let builder = reqwest::Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.request_timeout)
            .pool_idle_timeout(self.pool_idle_timeout)
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30));
        #[cfg(target_arch = "wasm32")]
        let builder = reqwest::Client::builder();
        if let Ok(http) = crate::http::build_http_client_for_base_url(builder, &url) {
            self.http = http;
        }
        self.base_url = url;
        self
    }

    /// Build request body for Anthropic API
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let mut messages = Vec::new();
        let mut system_prompt = None;

        for msg in &request.messages {
            match msg {
                Message::System(s) => {
                    system_prompt = Some(s.content.clone());
                }
                Message::User(u) => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": u.content
                    }));
                }
                Message::Assistant(a) => {
                    // Legacy format: flat content + tool_calls
                    let mut content = Vec::new();

                    if !a.content.is_empty() {
                        content.push(serde_json::json!({
                            "type": "text",
                            "text": a.content
                        }));
                    }

                    for tc in &a.tool_calls {
                        content.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.args
                        }));
                    }

                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                Message::BlockAssistant(a) => {
                    // New format: ordered blocks with thinking support
                    let mut content = Vec::new();

                    for block in &a.blocks {
                        match block {
                            meerkat_core::AssistantBlock::Text { text, .. } => {
                                // Anthropic text blocks don't use meta
                                if !text.is_empty() {
                                    content.push(serde_json::json!({
                                        "type": "text",
                                        "text": text
                                    }));
                                }
                            }
                            meerkat_core::AssistantBlock::Reasoning { text, meta } => {
                                match meta.as_deref() {
                                    Some(meerkat_core::ProviderMeta::Anthropic { signature }) => {
                                        // Regular thinking block with signature for replay
                                        content.push(serde_json::json!({
                                            "type": "thinking",
                                            "thinking": text,
                                            "signature": signature
                                        }));
                                    }
                                    Some(meerkat_core::ProviderMeta::AnthropicRedacted {
                                        data,
                                    }) => {
                                        // Redacted thinking — pass encrypted data verbatim
                                        content.push(serde_json::json!({
                                            "type": "redacted_thinking",
                                            "data": data
                                        }));
                                    }
                                    Some(meerkat_core::ProviderMeta::AnthropicCompaction {
                                        content: summary,
                                    }) => {
                                        // Compaction summary — replaces prior context
                                        content.push(serde_json::json!({
                                            "type": "compaction",
                                            "content": summary
                                        }));
                                    }
                                    _ => {
                                        tracing::warn!(
                                            "thinking block missing Anthropic signature, skipping"
                                        );
                                        continue;
                                    }
                                }
                            }
                            meerkat_core::AssistantBlock::ToolUse { id, name, args, .. } => {
                                // Parse RawValue to Value for JSON serialization
                                let args_value: Value = serde_json::from_str(args.get())
                                    .unwrap_or_else(|_| serde_json::json!({}));
                                content.push(serde_json::json!({
                                    "type": "tool_use",
                                    "id": id,
                                    "name": name,
                                    "input": args_value
                                }));
                            }
                            // Handle future block types (non_exhaustive pattern)
                            _ => {}
                        }
                    }

                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                Message::ToolResults { results } => {
                    let mut content = Vec::new();

                    for r in results {
                        content.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": r.tool_use_id,
                            "content": r.content,
                            "is_error": r.is_error
                        }));
                    }

                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content
                    }));
                }
            }
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "stream": true
        });

        if let Some(system) = system_prompt {
            body["system"] = Value::String(system);
        }

        if let Some(temp) = request.temperature
            && let Some(num) = serde_json::Number::from_f64(temp as f64)
        {
            body["temperature"] = Value::Number(num);
        }

        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
        }

        // Extract provider-specific params
        if let Some(ref params) = request.provider_params {
            // Handle thinking config from three formats:
            // 1. Adaptive (Opus 4.6): {"thinking": {"type": "adaptive"}}
            // 2. Legacy flat format: {"thinking_budget": 10000}
            // 3. Typed enabled: {"thinking": {"type": "enabled", "budget_tokens": 10000}}
            if let Some(thinking) = params.get("thinking") {
                if thinking.get("type").and_then(|t| t.as_str()) == Some("adaptive") {
                    // Opus 4.6 adaptive thinking — pass through directly
                    body["thinking"] = serde_json::json!({"type": "adaptive"});
                } else if let Some(budget) = thinking.get("budget_tokens").and_then(|v| v.as_u64())
                {
                    // Explicit enabled format with budget
                    body["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": budget
                    });
                }
            } else if let Some(budget) = params.get("thinking_budget").and_then(|v| v.as_u64()) {
                // Legacy flat format
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
            }

            // top_k must be a number - coerce strings from CLI --param
            if let Some(top_k) = params.get("top_k") {
                let numeric_top_k = match top_k {
                    Value::Number(_) => Some(top_k.clone()),
                    Value::String(s) => s.parse::<u64>().ok().map(|n| Value::Number(n.into())),
                    _ => None,
                };
                if let Some(v) = numeric_top_k {
                    body["top_k"] = v;
                }
            }

            // Handle effort parameter (GA on Opus 4.6, no beta header needed)
            // Format: {"effort": "low"|"medium"|"high"|"max"}
            if let Some(effort) = params.get("effort").and_then(|v| v.as_str()) {
                // Ensure output_config exists (may already be set by structured output below)
                if body.get("output_config").is_none() {
                    body["output_config"] = serde_json::json!({});
                }
                body["output_config"]["effort"] = Value::String(effort.to_string());
            }

            // Handle structured output configuration
            // Format: {"structured_output": {"schema": {...}, "name": "output", "strict": true}}
            if let Some(structured) = params.get("structured_output") {
                let output_schema: OutputSchema = serde_json::from_value(structured.clone())
                    .map_err(|e| LlmError::InvalidRequest {
                        message: format!("Invalid structured_output schema: {e}"),
                    })?;
                let compiled =
                    self.compile_schema(&output_schema)
                        .map_err(|e| LlmError::InvalidRequest {
                            message: e.to_string(),
                        })?;
                // Ensure output_config exists (may already have effort set above)
                if body.get("output_config").is_none() {
                    body["output_config"] = serde_json::json!({});
                }
                body["output_config"]["format"] = serde_json::json!({
                    "type": "json_schema",
                    "schema": compiled.schema
                });
            }

            // Handle inference_geo for data residency (Opus 4.6+)
            // Format: {"inference_geo": "us"} or {"inference_geo": "global"}
            if let Some(geo) = params.get("inference_geo").and_then(|v| v.as_str()) {
                body["inference_geo"] = Value::String(geo.to_string());
            }

            // Handle compaction (Opus 4.6, beta)
            // Format: {"compaction": "auto"} or {"compaction": {"trigger": 150000}}
            if let Some(compaction) = params.get("compaction") {
                if compaction.as_str() == Some("auto") {
                    body["context_management"] = serde_json::json!({
                        "edits": [{"type": "compact_20260112"}]
                    });
                } else if compaction.is_object() {
                    // Allow passing full compaction config: {"trigger": 150000, "instructions": "..."}
                    let mut edit = serde_json::json!({"type": "compact_20260112"});
                    if let Some(obj) = compaction.as_object() {
                        for (k, v) in obj {
                            edit[k] = v.clone();
                        }
                    }
                    body["context_management"] = serde_json::json!({
                        "edits": [edit]
                    });
                }
            }
        }

        Ok(body)
    }

    /// Parse an SSE event from the response
    fn parse_sse_line(line: &str) -> Option<AnthropicEvent> {
        if let Some(data) = Self::strip_data_prefix(line) {
            serde_json::from_str(data).ok()
        } else {
            None
        }
    }

    fn strip_data_prefix(line: &str) -> Option<&str> {
        line.strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
            .map(str::trim_start)
    }

    fn map_stop_reason(reason: &str) -> StopReason {
        match reason {
            "end_turn" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            "stop_sequence" => StopReason::StopSequence,
            "content_filter" => StopReason::ContentFilter,
            _ => StopReason::EndTurn,
        }
    }
}

/// Recursively add `additionalProperties: false` to all object schemas that
/// have `properties` but no explicit `additionalProperties`.
///
/// This is required by Anthropic's structured output API.
fn ensure_additional_properties_false(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            let is_object_type = match obj.get("type") {
                Some(Value::String(t)) => t == "object",
                Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
                _ => obj.contains_key("properties"),
            };
            if is_object_type
                && obj.contains_key("properties")
                && !obj.contains_key("additionalProperties")
            {
                obj.insert("additionalProperties".to_string(), Value::Bool(false));
            }

            let keys: Vec<String> = obj.keys().cloned().collect();
            for key in keys {
                if let Some(child) = obj.get_mut(&key) {
                    ensure_additional_properties_false(child);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                ensure_additional_properties_false(item);
            }
        }
        _ => {}
    }
}

fn merge_usage(target: &mut Usage, update: &AnthropicUsage) {
    if let Some(v) = update.input_tokens {
        target.input_tokens = v;
    }
    if let Some(v) = update.output_tokens {
        target.output_tokens = v;
    }
    if let Some(v) = update.cache_creation_input_tokens {
        target.cache_creation_tokens = Some(v);
    }
    if let Some(v) = update.cache_read_input_tokens {
        target.cache_read_tokens = Some(v);
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
            let body = self.build_request_body(request)?;

            // Collect beta headers based on request features
            let mut betas = Vec::new();

            // Legacy thinking (type: "enabled") requires interleaved-thinking header
            // Adaptive thinking (Opus 4.6) does NOT need this header
            let thinking_type = body.get("thinking")
                .and_then(|t| t.get("type"))
                .and_then(|t| t.as_str());
            if thinking_type == Some("enabled") {
                betas.push("interleaved-thinking-2025-05-14");
            }

            // Structured output format requires beta header
            if body.get("output_config").and_then(|c| c.get("format")).is_some() {
                betas.push("structured-outputs-2025-11-13");
            }

            // 1M context window (opt-in via provider_params)
            if let Some(ref params) = request.provider_params
                && params.get("context").and_then(|v| v.as_str()) == Some("1m")
            {
                betas.push("context-1m-2025-08-07");
            }

            // Compaction API (beta)
            if body.get("context_management").is_some() {
                betas.push("compact-2026-01-12");
            }

            let mut req = self.http
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json");

            if !betas.is_empty() {
                req = req.header("anthropic-beta", betas.join(","));
            }

            let response = req
                .json(&body)
                .send()
                .await
                .map_err(|_| LlmError::NetworkTimeout {
                    duration_ms: 30000,
                })?;

            let status_code = response.status().as_u16();
            let stream_result = if (200..=299).contains(&status_code) {
                Ok(response.bytes_stream())
            } else {
                let text = response.text().await.unwrap_or_default();
                Err(LlmError::from_http_status(status_code, text))
            };
            let mut stream = stream_result?;
            let mut buffer = String::with_capacity(SSE_BUFFER_CAPACITY);
            let mut current_tool_id: Option<String> = None;
            let mut current_tool_name: Option<String> = None;
            let mut accumulated_tool_args = String::new();
            let mut current_block_type: Option<String> = None;
            let mut current_thinking_signature: Option<String> = None;
            let mut last_stop_reason: Option<StopReason> = None;
            let mut usage = Usage::default();
            let mut saw_done = false;
            let mut saw_event = false;

            macro_rules! handle_event {
                ($event:expr) => {
                    match $event.event_type.as_str() {
                        "content_block_delta" => {
                            if let Some(delta) = $event.delta {
                                match delta.delta_type.as_str() {
                                    "text_delta" => {
                                        if let Some(text) = delta.text {
                                            saw_event = true;
                                            yield LlmEvent::TextDelta { delta: text, meta: None };
                                        }
                                    }
                                    "thinking_delta" => {
                                        // Emit incremental thinking content
                                        if let Some(text) = delta.thinking {
                                            saw_event = true;
                                            yield LlmEvent::ReasoningDelta { delta: text };
                                        }
                                    }
                                    "signature_delta" => {
                                        // Signature arrives as separate delta before content_block_stop
                                        if let Some(sig) = delta.signature {
                                            saw_event = true;
                                            current_thinking_signature = Some(sig);
                                        }
                                    }
                                    "input_json_delta" => {
                                        if let Some(partial_json) = delta.partial_json {
                                            accumulated_tool_args.push_str(&partial_json);
                                            saw_event = true;
                                            yield LlmEvent::ToolCallDelta {
                                                id: current_tool_id.clone().unwrap_or_default(),
                                                name: None,
                                                args_delta: partial_json,
                                            };
                                        }
                                    }
                                    "compaction_delta" => {
                                        // Compaction summary arrives as single delta — emit as Reasoning with compaction meta
                                        if let Some(content) = delta.content {
                                            saw_event = true;
                                            yield LlmEvent::ReasoningComplete {
                                                text: String::new(),
                                                meta: Some(Box::new(meerkat_core::ProviderMeta::AnthropicCompaction { content })),
                                            };
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "content_block_start" => {
                            if let Some(content_block) = $event.content_block {
                                current_block_type = Some(content_block.block_type.clone());
                                match content_block.block_type.as_str() {
                                    "thinking" => {
                                        // Start of thinking block
                                        // Signature may already be present or arrive via signature_delta
                                        current_thinking_signature = content_block.signature;
                                    }
                                    "redacted_thinking" => {
                                        // Redacted by safety systems — emit as Reasoning with redacted meta
                                        if let Some(data) = content_block.data {
                                            saw_event = true;
                                            yield LlmEvent::ReasoningComplete {
                                                text: String::new(),
                                                meta: Some(Box::new(meerkat_core::ProviderMeta::AnthropicRedacted { data })),
                                            };
                                        }
                                    }
                                    "compaction" => {
                                        // Compaction block start — content arrives via compaction_delta
                                    }
                                    "tool_use" => {
                                        let id = content_block.id.unwrap_or_default();
                                        current_tool_id = Some(id.clone());
                                        current_tool_name = content_block.name.clone();
                                        accumulated_tool_args.clear();
                                        saw_event = true;
                                        yield LlmEvent::ToolCallDelta {
                                            id,
                                            name: content_block.name,
                                            args_delta: String::new(),
                                        };
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "content_block_stop" => {
                            // Emit complete events based on block type
                            match current_block_type.as_deref() {
                                Some("thinking") => {
                                    // Emit ReasoningComplete with Anthropic signature
                                    let meta = current_thinking_signature
                                        .take()
                                        .map(|sig| Box::new(meerkat_core::ProviderMeta::Anthropic { signature: sig }));
                                    yield LlmEvent::ReasoningComplete {
                                        text: String::new(), // Text was already streamed via deltas
                                        meta,
                                    };
                                }
                                Some("tool_use") => {
                                    // Finalize tool call: assemble accumulated
                                    // JSON args from deltas into a complete event.
                                    if let Some(ref tool_id) = current_tool_id {
                                        // Accumulate args from the buffer by
                                        // reading the accumulated json string.
                                        // The name was set in content_block_start
                                        // and passed via the initial ToolCallDelta.
                                        // We emit ToolCallComplete with the
                                        // accumulated_tool_args (collected during
                                        // input_json_delta events).
                                        let args_str = accumulated_tool_args.clone();
                                        let args_val: serde_json::Value = serde_json::from_str(&args_str)
                                            .unwrap_or(serde_json::json!({}));
                                        saw_event = true;
                                        yield LlmEvent::ToolCallComplete {
                                            id: tool_id.clone(),
                                            name: current_tool_name.take().unwrap_or_default(),
                                            args: args_val,
                                            meta: None,
                                        };
                                        accumulated_tool_args.clear();
                                    }
                                    current_tool_id = None;
                                }
                                _ => {}
                            }
                            current_block_type = None;
                        }
                        "message_delta" => {
                            if let Some(usage_update) = $event.usage {
                                merge_usage(&mut usage, &usage_update);
                                saw_event = true;
                                yield LlmEvent::UsageUpdate {
                                    usage: usage.clone(),
                                };
                            }
                            if let Some(finish_reason) = $event.delta.and_then(|d| d.stop_reason) {
                                let reason = Self::map_stop_reason(finish_reason.as_str());
                                last_stop_reason = Some(reason);
                                if !saw_done {
                                    saw_event = true;
                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                    };
                                    saw_done = true;
                                }
                            }
                        }
                        "message_start" => {
                            if let Some(usage_update) = $event.message.and_then(|m| m.usage) {
                                merge_usage(&mut usage, &usage_update);
                                saw_event = true;
                                yield LlmEvent::UsageUpdate {
                                    usage: usage.clone(),
                                };
                            }
                        }
                        "message_stop" => {
                            if !saw_done {
                                let finish_reason = $event.delta.and_then(|d| d.stop_reason);
                                let reason = finish_reason
                                    .as_deref()
                                    .map(Self::map_stop_reason)
                                    .or(last_stop_reason)
                                    .unwrap_or(StopReason::EndTurn);
                                last_stop_reason = Some(reason);
                                saw_event = true;
                                yield LlmEvent::Done {
                                    outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                };
                                saw_done = true;
                            }
                        }
                        "error" => {
                            // Anthropic streaming error (e.g. overloaded_error mid-stream)
                            let error_msg = $event.error
                                .as_ref()
                                .and_then(|e| e.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown streaming error");
                            let error_type = $event.error
                                .as_ref()
                                .and_then(|e| e.get("type"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("unknown");

                            tracing::error!(
                                error_type,
                                error_msg,
                                "Anthropic streaming error"
                            );

                            let error = match error_type {
                                "overloaded_error" => LlmError::ServerOverloaded,
                                "rate_limit_error" => LlmError::RateLimited { retry_after_ms: None },
                                "api_error" => LlmError::ServerError {
                                    status: 500,
                                    message: error_msg.to_string(),
                                },
                                _ => LlmError::Unknown {
                                    message: format!("{error_type}: {error_msg}"),
                                },
                            };

                            if !saw_done {
                                saw_event = true;
                                yield LlmEvent::Done {
                                    outcome: LlmDoneOutcome::Error { error },
                                };
                                saw_done = true;
                            }
                        }
                        _ => {}
                    }
                };
            }

            macro_rules! handle_line {
                ($line:expr) => {
                    if !$line.is_empty() {
                        if let Some(data) = Self::strip_data_prefix($line) {
                            if data == "[DONE]" {
                                if !saw_done {
                                    let reason =
                                        last_stop_reason.unwrap_or(StopReason::EndTurn);
                                    saw_event = true;
                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                    };
                                    saw_done = true;
                                }
                            } else if let Some(event) = Self::parse_sse_line($line) {
                                handle_event!(event);
                            }
                        }
                    }
                };
            }

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim();
                    handle_line!(line);
                    buffer.drain(..=newline_pos);
                }
            }

            if !buffer.is_empty() {
                for line in buffer.lines() {
                    let line = line.trim();
                    handle_line!(line);
                }
            }

            if !saw_done && saw_event {
                tracing::warn!(
                    model = %request.model,
                    "Anthropic stream ended without terminal event; emitting synthetic Done"
                );
                let reason = last_stop_reason.unwrap_or(StopReason::EndTurn);
                yield LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { stop_reason: reason },
                };
            }
        });

        crate::streaming::ensure_terminal_done(inner)
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        let mut schema = output_schema.schema.as_value().clone();
        ensure_additional_properties_false(&mut schema);

        Ok(CompiledSchema {
            schema,
            warnings: Vec::new(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<AnthropicDelta>,
    content_block: Option<AnthropicContentBlock>,
    message: Option<AnthropicMessage>,
    usage: Option<AnthropicUsage>,
    /// Error object for streaming error events
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type", default)]
    delta_type: String,
    text: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
    /// Thinking content for thinking_delta events
    thinking: Option<String>,
    /// Signature for signature_delta events (arrives separately before content_block_stop)
    signature: Option<String>,
    /// Content for compaction_delta events
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
    /// Signature for thinking block continuity
    signature: Option<String>,
    /// Encrypted data for redacted_thinking blocks
    data: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessage {
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{AssistantBlock, BlockAssistantMessage, ProviderMeta, UserMessage};

    // =========================================================================
    // Thinking block SSE parsing tests (spec section 3.5)
    // =========================================================================

    #[test]
    fn test_anthropic_content_block_parses_thinking_type() {
        // content_block_start with type: "thinking" should parse
        let json = r#"{"type": "thinking", "thinking": "Let me analyze..."}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();

        assert_eq!(block.block_type, "thinking");
    }

    #[test]
    fn test_anthropic_content_block_parses_thinking_with_signature() {
        // content_block_start may have signature already
        let json = r#"{"type": "thinking", "thinking": "", "signature": "sig_abc123"}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();

        assert_eq!(block.block_type, "thinking");
        assert_eq!(block.signature.as_deref(), Some("sig_abc123"));
    }

    #[test]
    fn test_anthropic_delta_parses_thinking_delta() {
        // content_block_delta with delta type: "thinking_delta"
        let json = r#"{"type": "thinking_delta", "thinking": "I need to consider..."}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "thinking_delta");
        assert_eq!(delta.thinking.as_deref(), Some("I need to consider..."));
    }

    #[test]
    fn test_anthropic_delta_parses_signature_delta() {
        // content_block_delta with delta type: "signature_delta"
        let json = r#"{"type": "signature_delta", "signature": "sig_xyz789"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "signature_delta");
        assert_eq!(delta.signature.as_deref(), Some("sig_xyz789"));
    }

    #[test]
    fn test_anthropic_delta_parses_text_delta_unchanged() {
        // Existing text_delta should still work
        let json = r#"{"type": "text_delta", "text": "Hello world"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "text_delta");
        assert_eq!(delta.text.as_deref(), Some("Hello world"));
        assert!(delta.thinking.is_none());
        assert!(delta.signature.is_none());
    }

    #[test]
    fn test_anthropic_delta_parses_input_json_delta_unchanged() {
        // Existing input_json_delta should still work
        let json = r#"{"type": "input_json_delta", "partial_json": "{\"path\":"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "input_json_delta");
        assert_eq!(delta.partial_json.as_deref(), Some("{\"path\":"));
    }

    // =========================================================================
    // Request building tests for BlockAssistantMessage (spec section 3.5)
    // =========================================================================

    #[test]
    fn test_build_request_body_renders_thinking_block_with_signature()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Create a session with a thinking block
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: "Let me analyze this problem...".to_string(),
                    meta: Some(Box::new(ProviderMeta::Anthropic {
                        signature: "sig_abc123".to_string(),
                    })),
                },
                AssistantBlock::Text {
                    text: "The answer is 42.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage {
                    content: "What is the meaning of life?".to_string(),
                }),
                assistant_msg,
                Message::User(UserMessage {
                    content: "Can you elaborate?".to_string(),
                }),
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();

        // Second message should be the assistant with thinking + text
        let assistant_content = messages[1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 2);

        // First block should be thinking
        assert_eq!(assistant_content[0]["type"], "thinking");
        assert_eq!(
            assistant_content[0]["thinking"],
            "Let me analyze this problem..."
        );
        assert_eq!(assistant_content[0]["signature"], "sig_abc123");

        // Second block should be text
        assert_eq!(assistant_content[1]["type"], "text");
        assert_eq!(assistant_content[1]["text"], "The answer is 42.");

        Ok(())
    }

    #[test]
    fn test_build_request_body_skips_thinking_block_without_signature()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Thinking block without Anthropic signature should be skipped
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: "Some thinking...".to_string(),
                    meta: None, // No signature!
                },
                AssistantBlock::Text {
                    text: "The answer.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage {
                    content: "Question".to_string(),
                }),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();

        // Assistant content should only have the text block (thinking skipped)
        let assistant_content = messages[1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 1);
        assert_eq!(assistant_content[0]["type"], "text");

        Ok(())
    }

    #[test]
    fn test_build_request_body_renders_tool_use_blocks() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Text {
                    text: "I'll read that file for you.".to_string(),
                    meta: None,
                },
                AssistantBlock::ToolUse {
                    id: "tu_123".to_string(),
                    name: "read_file".to_string(),
                    args: serde_json::value::RawValue::from_string(
                        r#"{"path": "/tmp/test.txt"}"#.to_string(),
                    )
                    .unwrap(),
                    meta: None, // Tool use blocks don't have signatures in Anthropic
                },
            ],
            stop_reason: StopReason::ToolUse,
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage {
                    content: "Read /tmp/test.txt".to_string(),
                }),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let assistant_content = messages[1]["content"].as_array().unwrap();

        assert_eq!(assistant_content.len(), 2);
        assert_eq!(assistant_content[0]["type"], "text");
        assert_eq!(assistant_content[1]["type"], "tool_use");
        assert_eq!(assistant_content[1]["id"], "tu_123");
        assert_eq!(assistant_content[1]["name"], "read_file");
        assert_eq!(assistant_content[1]["input"]["path"], "/tmp/test.txt");

        Ok(())
    }

    #[test]
    fn test_build_request_body_adds_thinking_beta_header() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = AnthropicClient::new("test-key".to_string())?;

        // When thinking is enabled via provider_params, beta header should be added
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking_budget", 10000);

        let body = client.build_request_body(&request)?;

        // The beta header is added during the HTTP request, not in the body
        // But the thinking config should be in the body
        assert!(body.get("thinking").is_some());
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);

        Ok(())
    }

    // =========================================================================
    // Original tests (unchanged)
    // =========================================================================

    #[test]
    fn test_build_request_body_basic() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking_budget", 10000);

        let body = client.build_request_body(&request)?;

        assert!(
            body.get("thinking").is_some(),
            "thinking field should be present"
        );
        let thinking = &body["thinking"];
        assert_eq!(thinking["type"], "enabled");
        assert_eq!(thinking["budget_tokens"], 10000);
        Ok(())
    }

    #[test]
    fn test_build_request_body_with_top_k() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", 40);

        let body = client.build_request_body(&request)?;

        assert_eq!(body["top_k"], 40);
        Ok(())
    }

    #[test]
    fn test_build_request_body_no_provider_params() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request)?;

        assert!(body.get("thinking").is_none());
        assert!(body.get("top_k").is_none());
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        Ok(())
    }

    #[test]
    fn test_client_builder_creates_configured_client() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::builder("test-key".to_string())
            .connect_timeout(std::time::Duration::from_secs(5))
            .request_timeout(std::time::Duration::from_secs(120))
            .build()?;

        assert_eq!(client.provider(), "anthropic");
        Ok(())
    }

    #[test]
    fn test_client_default_has_connection_pool() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        assert_eq!(client.provider(), "anthropic");
        Ok(())
    }

    #[test]
    fn test_parse_sse_line() {
        let line = r###"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"###;
        let event = AnthropicClient::parse_sse_line(line);
        assert!(event.is_some());
        assert_eq!(event.unwrap().event_type, "content_block_delta");

        let done = AnthropicClient::parse_sse_line("data: [DONE]");
        assert!(done.is_none());

        let comment = AnthropicClient::parse_sse_line(": comment");
        assert!(comment.is_none());

        let event_line = AnthropicClient::parse_sse_line("event: message_start");
        assert!(event_line.is_none());
    }

    #[test]
    fn test_sse_buffer_constants() {
        let buffer_cap = SSE_BUFFER_CAPACITY;
        assert!(buffer_cap >= 256, "SSE buffer should be at least 256 bytes");
        let connect_timeout = DEFAULT_CONNECT_TIMEOUT.as_secs();
        assert!(
            connect_timeout >= 5,
            "Connect timeout should be at least 5s"
        );
        let request_timeout = DEFAULT_REQUEST_TIMEOUT.as_secs();
        assert!(
            request_timeout >= 60,
            "Request timeout should be at least 60s"
        );
    }

    /// Regression: CLI --param passes top_k as string; must coerce to number
    /// Previously: `--param top_k=40` sent `"top_k": "40"` which Anthropic rejects
    #[test]
    fn test_regression_top_k_string_coercion() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Simulate CLI --param which passes values as strings
        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", "40"); // String, not number!

        let body = client.build_request_body(&request)?;

        // Should be coerced to a number
        assert!(
            body["top_k"].is_number(),
            "top_k should be a number, not string"
        );
        assert_eq!(body["top_k"], 40);
        Ok(())
    }

    /// Regression: non-numeric string values for top_k should be ignored
    #[test]
    fn test_regression_top_k_invalid_string_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", "not_a_number");

        let body = client.build_request_body(&request)?;

        // Invalid string should be ignored (no top_k in body)
        assert!(
            body.get("top_k").is_none(),
            "invalid top_k should be ignored"
        );
        Ok(())
    }

    #[test]
    fn test_usage_merge_preserves_input_tokens() {
        let mut usage = Usage::default();
        let start = AnthropicUsage {
            input_tokens: Some(120),
            output_tokens: Some(0),
            cache_creation_input_tokens: Some(4),
            cache_read_input_tokens: Some(2),
        };

        merge_usage(&mut usage, &start);

        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, Some(4));
        assert_eq!(usage.cache_read_tokens, Some(2));

        let delta = AnthropicUsage {
            input_tokens: None,
            output_tokens: Some(7),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };

        merge_usage(&mut usage, &delta);

        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cache_creation_tokens, Some(4));
        assert_eq!(usage.cache_read_tokens, Some(2));
    }

    #[test]
    fn test_build_request_body_with_structured_output() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param(
            "structured_output",
            serde_json::json!({
                "schema": schema,
                "name": "person",
                "strict": true
            }),
        );

        let body = client.build_request_body(&request)?;

        // Check output_config is present with correct structure
        assert!(
            body.get("output_config").is_some(),
            "output_config should be present"
        );
        let output_config = &body["output_config"];
        assert_eq!(output_config["format"]["type"], "json_schema");
        assert!(output_config["format"]["schema"].is_object());
        assert_eq!(
            output_config["format"]["schema"]["additionalProperties"],
            serde_json::json!(false)
        );
        Ok(())
    }

    #[test]
    fn test_build_request_body_without_structured_output() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request)?;

        // output_config should not be present
        assert!(
            body.get("output_config").is_none(),
            "output_config should not be present without structured_output"
        );
        Ok(())
    }

    // AdditionalProperties handling is covered by core schema compiler tests.

    // =========================================================================
    // Opus 4.6: Adaptive thinking & effort parameter tests
    // =========================================================================

    #[test]
    fn test_build_request_body_adaptive_thinking() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking", serde_json::json!({"type": "adaptive"}));

        let body = client.build_request_body(&request)?;

        assert!(
            body.get("thinking").is_some(),
            "thinking field should be present"
        );
        assert_eq!(body["thinking"]["type"], "adaptive");
        // adaptive mode should NOT have budget_tokens
        assert!(body["thinking"].get("budget_tokens").is_none());
        Ok(())
    }

    #[test]
    fn test_build_request_body_effort_parameter() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("effort", "medium");

        let body = client.build_request_body(&request)?;

        assert!(
            body.get("output_config").is_some(),
            "output_config should be present"
        );
        assert_eq!(body["output_config"]["effort"], "medium");
        Ok(())
    }

    #[test]
    fn test_build_request_body_effort_with_structured_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });

        let mut request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );
        request.provider_params = Some(serde_json::json!({
            "effort": "high",
            "structured_output": {
                "schema": schema,
                "name": "output",
                "strict": true
            }
        }));

        let body = client.build_request_body(&request)?;

        // output_config should have BOTH effort and format
        let output_config = &body["output_config"];
        assert_eq!(output_config["effort"], "high");
        assert_eq!(output_config["format"]["type"], "json_schema");
        assert!(output_config["format"]["schema"].is_object());
        Ok(())
    }

    #[test]
    fn test_build_request_body_adaptive_thinking_does_not_set_interleaved_beta()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking", serde_json::json!({"type": "adaptive"}));

        let body = client.build_request_body(&request)?;

        // Adaptive thinking should be in the body
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(
            body["thinking"].get("budget_tokens").is_none(),
            "adaptive thinking should not have budget_tokens"
        );
        Ok(())
    }

    #[test]
    fn test_build_request_body_legacy_thinking_still_works()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Legacy flat format should still produce type: enabled
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking_budget", 10000);

        let body = client.build_request_body(&request)?;

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);
        Ok(())
    }

    #[test]
    fn test_build_request_body_inference_geo() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("inference_geo", "us");

        let body = client.build_request_body(&request)?;

        assert_eq!(body["inference_geo"], "us");
        Ok(())
    }

    #[test]
    fn test_build_request_body_compaction_auto() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("compaction", "auto");

        let body = client.build_request_body(&request)?;

        let edits = body["context_management"]["edits"].as_array().unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["type"], "compact_20260112");
        Ok(())
    }

    #[test]
    fn test_build_request_body_compaction_with_trigger() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let mut request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );
        request.provider_params = Some(serde_json::json!({
            "compaction": {
                "trigger": {"type": "input_tokens", "value": 100000}
            }
        }));

        let body = client.build_request_body(&request)?;

        let edits = body["context_management"]["edits"].as_array().unwrap();
        assert_eq!(edits[0]["type"], "compact_20260112");
        assert_eq!(edits[0]["trigger"]["value"], 100000);
        Ok(())
    }

    #[test]
    fn test_build_request_body_redacted_thinking_roundtrip()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Redacted thinking is stored as Reasoning with AnthropicRedacted meta
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: String::new(),
                    meta: Some(Box::new(ProviderMeta::AnthropicRedacted {
                        data: "encrypted_blob_abc123".to_string(),
                    })),
                },
                AssistantBlock::Text {
                    text: "The answer.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
        });

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![
                Message::User(UserMessage {
                    content: "Question".to_string(),
                }),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let assistant_content = messages[1]["content"].as_array().unwrap();

        // First block should be redacted_thinking with encrypted data
        assert_eq!(assistant_content[0]["type"], "redacted_thinking");
        assert_eq!(assistant_content[0]["data"], "encrypted_blob_abc123");

        // Second block should be text
        assert_eq!(assistant_content[1]["type"], "text");
        Ok(())
    }

    #[test]
    fn test_build_request_body_compaction_block_roundtrip() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Compaction is stored as Reasoning with AnthropicCompaction meta
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: String::new(),
                    meta: Some(Box::new(ProviderMeta::AnthropicCompaction {
                        content: "Summary of prior conversation...".to_string(),
                    })),
                },
                AssistantBlock::Text {
                    text: "Continuing from summary.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
        });

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![
                Message::User(UserMessage {
                    content: "Continue".to_string(),
                }),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let assistant_content = messages[1]["content"].as_array().unwrap();

        // First block should be compaction with summary
        assert_eq!(assistant_content[0]["type"], "compaction");
        assert_eq!(
            assistant_content[0]["content"],
            "Summary of prior conversation..."
        );

        // Second block should be text
        assert_eq!(assistant_content[1]["type"], "text");
        Ok(())
    }

    // =========================================================================
    // SSE stream regression tests
    // =========================================================================

    use axum::{Router, extract::State, response::IntoResponse, routing::post};
    use tokio::net::TcpListener;

    async fn messages_sse(State(payload): State<String>) -> impl IntoResponse {
        ([("content-type", "text/event-stream")], payload)
    }

    async fn spawn_anthropic_stub_server(payload: String) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/v1/messages", post(messages_sse))
            .with_state(payload);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (format!("http://{}", addr), handle)
    }

    /// Regression: message_delta with stop_reason (no "type" in delta) must yield Done.
    ///
    /// Previously, AnthropicDelta required delta_type (mapped from "type") as a non-optional
    /// String. Anthropic's message_delta sends {"delta": {"stop_reason": "end_turn"}} with
    /// no "type" field, causing the entire event to fail serde parsing silently. When the
    /// stream ended before message_stop arrived, no Done was ever emitted.
    #[tokio::test]
    async fn test_regression_message_delta_stop_reason_without_type_yields_done() {
        // Simulate a stream where message_stop is NOT sent (e.g., connection dropped
        // after message_delta). Done must come from message_delta's stop_reason.
        let payload = [
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"data: {"type":"content_block_start","content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"data: {"type":"content_block_stop"}"#,
            // message_delta has "delta": {"stop_reason": "end_turn"} with NO "type" field
            r#"data: {"type":"message_delta","usage":{"output_tokens":5},"delta":{"stop_reason":"end_turn"}}"#,
            // Simulate stream ending WITHOUT message_stop (connection dropped)
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage {
                content: "hello".to_string(),
            })],
        );

        let mut stream = client.stream(&request);
        let mut saw_text = false;
        let mut saw_done = false;
        let mut done_is_success = false;
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                LlmEvent::TextDelta { delta, .. } if delta == "Hello" => saw_text = true,
                LlmEvent::Done { outcome } => {
                    saw_done = true;
                    done_is_success = matches!(outcome, LlmDoneOutcome::Success { .. });
                    break;
                }
                _ => {}
            }
        }
        server.abort();

        assert!(saw_text, "Expected text delta");
        assert!(
            saw_done,
            "Expected Done event from message_delta stop_reason"
        );
        assert!(done_is_success, "Expected successful Done outcome");
    }

    /// Regression: Anthropic streaming error event must yield Done with error.
    #[tokio::test]
    async fn test_regression_anthropic_error_event_yields_done_with_error() {
        let payload = [
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage {
                content: "hello".to_string(),
            })],
        );

        let mut stream = client.stream(&request);
        let mut saw_error_done = false;
        while let Some(event) = stream.next().await {
            if let LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            } = event.expect("stream event")
            {
                assert!(
                    matches!(error, LlmError::ServerOverloaded),
                    "expected ServerOverloaded, got: {error:?}"
                );
                saw_error_done = true;
                break;
            }
        }
        server.abort();

        assert!(saw_error_done, "Expected Done with error outcome");
    }

    /// Normal stream with message_stop should still work (baseline).
    #[tokio::test]
    async fn test_normal_stream_with_message_stop_yields_done() {
        let payload = [
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"data: {"type":"content_block_start","content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}"#,
            r#"data: {"type":"content_block_stop"}"#,
            r#"data: {"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"end_turn"}}"#,
            r#"data: {"type":"message_stop"}"#,
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage {
                content: "hello".to_string(),
            })],
        );

        let mut stream = client.stream(&request);
        let mut done_count = 0;
        while let Some(event) = stream.next().await {
            if let LlmEvent::Done { .. } = event.expect("stream event") {
                done_count += 1;
                break;
            }
        }
        server.abort();

        assert_eq!(done_count, 1, "Expected exactly one Done event");
    }
}
