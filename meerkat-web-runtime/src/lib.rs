//! Meerkat WASM runtime — a real meerkat surface in the browser.
//!
//! Routes through `AgentFactory::build_agent()` with override-first resource
//! injection, same pipeline as CLI/RPC/REST/MCP. Uses real agent loop, real LLM
//! providers (browser fetch), real comms (inproc), and real tool dispatch.
//!
//! ## Platform API
//!
//! ### Bootstrap
//! - `init_runtime(mobpack_bytes, credentials_json)` — primary mobpack bootstrap
//! - `init_runtime_from_config(config_json)` — bare-bones bootstrap without mobpack
//! - `runtime_version()` → crate version string (for JS/WASM version mismatch detection)
//!
//! ### Session (runtime-backed session-handle façades)
//! - `create_session(mobpack_bytes, config_json)` → handle
//! - `start_turn(handle, prompt, options_json)` → JSON result
//! - `get_session_state(handle)` → JSON
//! - `destroy_session(handle)`
//! - `inspect_mobpack(mobpack_bytes)` → JSON
//! - `poll_events(handle)` → JSON
//!
//! ### Mob lifecycle (delegates to `MobMcpState`)
//! - `mob_create(definition_json)` → mob_id
//! - `mob_status(mob_id)` → JSON
//! - `mob_list()` → JSON
//! - `mob_lifecycle(mob_id, action)` — stop/resume/complete/reset/destroy
//! - `mob_events(mob_id, after_cursor, limit)` → JSON
//! - `mob_spawn(mob_id, specs_json)` → JSON
//! - `mob_retire(mob_id, meerkat_id)`
//! - `mob_wire_peer(mob_id, member, peer_json)` / `mob_unwire_peer(mob_id, member, peer_json)` — canonical member/peer wiring
//! - `mob_wire(mob_id, a, b)` / `mob_unwire(mob_id, a, b)` — low-level local-local wiring
//! - `mob_wire_target(mob_id, local, target_json)` / `mob_unwire_target(mob_id, local, target_json)` — low-level aliases
//! - `mob_list_members(mob_id)` → JSON
//! - `mob_append_system_context(mob_id, meerkat_id, request_json)` → JSON
//! - `mob_member_send(mob_id, meerkat_id, request_json)` → JSON delivery receipt
//! - `mob_member_status(mob_id, meerkat_id)` → JSON member snapshot
//! - `mob_respawn(mob_id, meerkat_id, initial_message?)` → JSON result envelope
//! - `mob_force_cancel(mob_id, meerkat_id)`
//! - `mob_spawn_helper(mob_id, request_json)` → JSON helper result
//! - `mob_fork_helper(mob_id, request_json)` → JSON helper result
//! - `mob_run_flow(mob_id, flow_id, params_json)` → run_id
//! - `mob_flow_status(mob_id, run_id)` → JSON
//! - `mob_cancel_flow(mob_id, run_id)`
//!
//! ### Event Streaming
//! - `mob_member_subscribe(mob_id, meerkat_id)` → handle (per-member)
//! - `mob_subscribe_events(mob_id)` → handle (mob-wide)
//! - `poll_subscription(handle)` → JSON events
//! - `close_subscription(handle)`
//!
//! ### Comms (placeholder)
//! - `comms_peers(session_id)` → JSON
//! - `comms_send(session_id, params_json)` → JSON

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

#[cfg(not(target_arch = "wasm32"))]
pub use ::tokio;

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::sync::Arc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;

use meerkat::{AgentBuildConfig, SessionServiceControlExt};
use meerkat_core::{Config, SessionService};
use meerkat_mob::{FlowId, MeerkatId, MobDefinition, MobId, RunId};
use meerkat_mob_mcp::MobMcpState;

// ═══════════════════════════════════════════════════════════
// Tracing → browser console (wasm32 only)
// ═══════════════════════════════════════════════════════════

#[cfg(target_arch = "wasm32")]
fn init_tracing() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            tracing::error!("wasm panic: {info}");
        }));
        tracing_wasm::set_as_global_default();
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn init_tracing() {}

// ═══════════════════════════════════════════════════════════
// reqwest wasm32 workaround — Response.url backfill
// ═══════════════════════════════════════════════════════════

/// Wrap `globalThis.fetch` so that `Response.url` is never empty.
///
/// reqwest on wasm32 calls `Url::parse(response.url)` and panics when the
/// string is empty (`RelativeUrlWithoutBase`). Browser-native fetch always
/// fills `Response.url`, but constructed responses (`new Response(body, init)`)
/// have `.url === ""` because the property is read-only per spec. This breaks
/// any host that proxies fetch (reverse proxy, service worker, test harness).
///
/// Workaround for <https://github.com/seanmonstar/reqwest/issues/2489>.
/// Remove once reqwest ships a fix upstream.
#[cfg(target_arch = "wasm32")]
fn patch_fetch_response_url() {
    use std::sync::Once;
    static PATCH: Once = Once::new();
    PATCH.call_once(|| {
        if let Err(e) = js_sys::eval(
            r"(function(){
  var f=globalThis.fetch;
  globalThis.fetch=function(i,o){
    var u=typeof i==='string'?i:i instanceof Request?i.url:String(i);
    return f.call(globalThis,i,o).then(function(r){
      if(!r.url&&u)Object.defineProperty(r,'url',{value:u});
      return r;
    });
  };
})()",
        ) {
            tracing::warn!("fetch response-url patch failed: {:?}", e);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn patch_fetch_response_url() {}

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

const MAX_SYSTEM_PROMPT_BYTES: usize = 100 * 1024;
const FORBIDDEN_CAPABILITIES: &[&str] = &["shell", "mcp_stdio", "process_spawn"];
const SKILL_SEPARATOR: &str = "\n\n---\n\n";
const MAX_SESSIONS: usize = 64;

// ═══════════════════════════════════════════════════════════
// Mobpack Types
// ═══════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct WebManifest {
    mobpack: MobpackSection,
    #[serde(default)]
    requires: Option<RequiresSection>,
}

#[derive(Debug, Deserialize)]
struct MobpackSection {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RequiresSection {
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MobDefinitionHeader {
    id: String,
}

// ═══════════════════════════════════════════════════════════
// Session Config (from JavaScript)
// ═══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionConfig {
    model: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    /// Backward-compat single base URL — mapped to the inferred provider.
    #[serde(default)]
    base_url: Option<String>,
    /// Per-provider base URLs — take precedence over `base_url`.
    #[serde(default)]
    anthropic_base_url: Option<String>,
    #[serde(default)]
    openai_base_url: Option<String>,
    #[serde(default)]
    gemini_base_url: Option<String>,
    /// Enable comms for this session (registers in InprocRegistry).
    #[serde(default)]
    comms_name: Option<String>,
    /// Whether this session stays alive after the initial turn (enables comms drain loop).
    #[serde(default)]
    keep_alive: bool,
    /// Application-defined labels (flow through mob spawn, not used at create_session level).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Additional instruction sections appended to the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    /// Opaque application context passed through to the agent build pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
}

fn default_max_tokens() -> u32 {
    4096
}

// ═══════════════════════════════════════════════════════════
// Credentials / Config for init_runtime
// ═══════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct Credentials {
    /// Backward-compat single key (treated as anthropic fallback).
    #[serde(default)]
    api_key: Option<String>,
    /// Per-provider API keys — preferred over `api_key`.
    #[serde(default)]
    anthropic_api_key: Option<String>,
    #[serde(default)]
    openai_api_key: Option<String>,
    #[serde(default)]
    gemini_api_key: Option<String>,
    #[serde(default = "default_model")]
    model: Option<String>,
    /// Backward-compat single base URL — mapped to the default model's provider.
    #[serde(default)]
    base_url: Option<String>,
    /// Per-provider base URLs — take precedence over `base_url`.
    #[serde(default)]
    anthropic_base_url: Option<String>,
    #[serde(default)]
    openai_base_url: Option<String>,
    #[serde(default)]
    gemini_base_url: Option<String>,
}

fn default_model() -> Option<String> {
    Some(
        meerkat_models::default_model("anthropic")
            .unwrap_or("claude-sonnet-4-5")
            .to_string(),
    )
}

#[derive(Debug, Deserialize)]
struct RuntimeConfig {
    /// Backward-compat single key (treated as anthropic fallback).
    #[serde(default)]
    api_key: Option<String>,
    /// Per-provider API keys — preferred over `api_key`.
    #[serde(default)]
    anthropic_api_key: Option<String>,
    #[serde(default)]
    openai_api_key: Option<String>,
    #[serde(default)]
    gemini_api_key: Option<String>,
    #[serde(default = "default_model")]
    model: Option<String>,
    /// Backward-compat single base URL — mapped to the default model's provider.
    #[serde(default)]
    base_url: Option<String>,
    /// Per-provider base URLs — take precedence over `base_url`.
    #[serde(default)]
    anthropic_base_url: Option<String>,
    #[serde(default)]
    openai_base_url: Option<String>,
    #[serde(default)]
    gemini_base_url: Option<String>,
    #[serde(default = "default_max_sessions")]
    max_sessions: usize,
}

fn default_max_sessions() -> usize {
    MAX_SESSIONS
}

// ═══════════════════════════════════════════════════════════
// Event Model
// ═══════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════
// Runtime-backed browser session-handle façade
// ═══════════════════════════════════════════════════════════

type WasmSessionEventReceiver = crate::tokio::sync::broadcast::Receiver<
    meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
>;

struct RuntimeHandleSession {
    session_id: meerkat_core::SessionId,
    mob_id: String,
    model: String,
    keep_alive: bool,
    run_counter: u64,
    event_rx: WasmSessionEventReceiver,
}

// ═══════════════════════════════════════════════════════════
// RuntimeState — service-based infrastructure
// ═══════════════════════════════════════════════════════════

type WasmSessionService = meerkat::EphemeralSessionService<meerkat::FactoryAgentBuilder>;

struct RuntimeState {
    mob_state: Arc<MobMcpState>,
    /// Concrete session service — needed for subscribe_session_events_raw.
    session_service: Arc<WasmSessionService>,
    /// Opaque browser-local handles mapped to runtime-owned sessions.
    sessions: BTreeMap<u32, RuntimeHandleSession>,
    next_handle: u32,
    #[allow(dead_code)]
    model: String,
    #[cfg(target_arch = "wasm32")]
    js_tools: Vec<JsToolEntry>,
}

/// Resolve per-provider API keys into a `Config.providers.api_keys` map.
///
/// `api_key` is the backward-compat single key (treated as anthropic fallback).
/// Per-provider fields take precedence when set. Empty/whitespace-only keys are
/// treated as missing so callers get a fast failure instead of a deferred auth error.
fn build_provider_api_keys(
    api_key: Option<&str>,
    anthropic_api_key: Option<&str>,
    openai_api_key: Option<&str>,
    gemini_api_key: Option<&str>,
) -> Result<HashMap<String, String>, JsValue> {
    // Treat blank keys as absent so misconfigured init fails fast.
    fn non_blank(s: Option<&str>) -> Option<&str> {
        match s {
            Some(v) if !v.trim().is_empty() => Some(v),
            _ => None,
        }
    }

    let mut keys = HashMap::new();
    // Per-provider keys take precedence; api_key is anthropic fallback.
    let anthropic = non_blank(anthropic_api_key).or(non_blank(api_key));
    if let Some(k) = anthropic {
        keys.insert("anthropic".into(), k.to_string());
    }
    if let Some(k) = non_blank(openai_api_key) {
        keys.insert("openai".into(), k.to_string());
    }
    if let Some(k) = non_blank(gemini_api_key) {
        keys.insert("gemini".into(), k.to_string());
    }
    if keys.is_empty() {
        return Err(err_js(
            "invalid_config",
            "at least one API key must be provided (api_key, anthropic_api_key, openai_api_key, or gemini_api_key)",
        ));
    }
    Ok(keys)
}

/// Resolve per-provider base URLs into a `Config.providers.base_urls` map.
///
/// Per-provider fields (`anthropic_base_url`, etc.) take precedence. The
/// backward-compat single `base_url` is mapped to the default model's inferred
/// provider as a fallback.
fn build_provider_base_urls(
    base_url: Option<&str>,
    anthropic_base_url: Option<&str>,
    openai_base_url: Option<&str>,
    gemini_base_url: Option<&str>,
    model: &str,
) -> Option<HashMap<String, String>> {
    fn non_blank(s: Option<&str>) -> Option<&str> {
        match s {
            Some(v) if !v.trim().is_empty() => Some(v),
            _ => None,
        }
    }

    let mut urls = HashMap::new();
    if let Some(url) = non_blank(anthropic_base_url) {
        urls.insert("anthropic".into(), url.to_string());
    }
    if let Some(url) = non_blank(openai_base_url) {
        urls.insert("openai".into(), url.to_string());
    }
    if let Some(url) = non_blank(gemini_base_url) {
        urls.insert("gemini".into(), url.to_string());
    }
    // Backward-compat: single base_url maps to the default model's provider,
    // but only if no per-provider URL was set for that provider.
    if let Some(url) = non_blank(base_url) {
        if let Some(provider) = infer_provider_name(model) {
            urls.entry(provider.to_string())
                .or_insert_with(|| url.to_string());
        } else {
            tracing::warn!(model, "base_url ignored: cannot infer provider from model");
        }
    }
    if urls.is_empty() { None } else { Some(urls) }
}

/// Resolve the effective base URL for a single session's LLM client.
///
/// Per-provider fields take precedence; `base_url` is the backward-compat fallback
/// for the model's inferred provider.
fn resolve_session_base_url(
    base_url: Option<&str>,
    anthropic_base_url: Option<&str>,
    openai_base_url: Option<&str>,
    gemini_base_url: Option<&str>,
    model: &str,
) -> Option<String> {
    let provider = infer_provider_name(model)?;
    let per_provider = match provider {
        "anthropic" => anthropic_base_url,
        "openai" => openai_base_url,
        "gemini" => gemini_base_url,
        _ => None,
    };
    per_provider
        .or(base_url)
        .filter(|s| !s.trim().is_empty())
        .map(ToString::to_string)
}

/// Infer the provider name from a model string.
///
/// Delegates to the canonical `Provider::infer_from_model` in meerkat-core.
/// Returns `None` for unrecognized models (caller should error, not default).
fn infer_provider_name(model: &str) -> Option<&'static str> {
    meerkat_core::Provider::infer_from_model(model).map(|p| p.as_str())
}

/// Build the shared service infrastructure from a Config populated with provider keys.
fn build_service_infrastructure(
    config: Config,
    max_sessions: usize,
) -> Result<(Arc<WasmSessionService>, Arc<MobMcpState>), JsValue> {
    let factory = meerkat::AgentFactory::minimal();
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);

    // NO default_llm_client — build_agent() resolves the correct provider per-model
    // from Config.providers.api_keys. This is architecturally correct: per-agent
    // provider agnosticity works the same way on WASM as on all other surfaces.

    let tools = build_wasm_tool_dispatcher().map_err(|e| err_str("tool_error", e))?;
    builder.default_tool_dispatcher = Some(tools);
    let store: Arc<dyn meerkat_core::AgentSessionStore> = Arc::new(
        meerkat_store::StoreAdapter::new(Arc::new(meerkat_store::MemoryStore::new())),
    );
    builder.default_session_store = Some(store);

    let service = Arc::new(meerkat::EphemeralSessionService::new(builder, max_sessions));
    let session_service = service.clone();
    let mob_state = Arc::new(MobMcpState::new(
        service as Arc<dyn meerkat_mob::MobSessionService>,
    ));
    Ok((session_service, mob_state))
}

thread_local! {
    static RUNTIME_STATE: RefCell<Option<RuntimeState>> = const { RefCell::new(None) };
}

fn clear_subscription_registry() {
    SUBSCRIPTIONS.with(|cell| {
        let mut registry = cell.borrow_mut();
        registry.subscriptions.clear();
        registry.next_handle = 1;
    });
}

fn install_runtime_state(state: RuntimeState) {
    clear_subscription_registry();
    RUNTIME_STATE.with(|cell| {
        *cell.borrow_mut() = Some(state);
    });
}

/// Tear down the embedded runtime and release all local handles/subscriptions.
///
/// Existing `Session`, `Mob`, and subscription handles become invalid after
/// this call.
#[wasm_bindgen]
pub fn destroy_runtime() -> Result<(), JsValue> {
    clear_subscription_registry();
    RUNTIME_STATE.with(|cell| {
        cell.borrow_mut().take();
    });
    Ok(())
}

fn with_runtime_state<F, R>(f: F) -> Result<R, JsValue>
where
    F: FnOnce(&RuntimeState) -> Result<R, JsValue>,
{
    RUNTIME_STATE.with(|cell| {
        let borrow = cell.borrow();
        let state = borrow.as_ref().ok_or_else(|| {
            err_js(
                "not_initialized",
                "runtime not initialized — call init_runtime or init_runtime_from_config first",
            )
        })?;
        f(state)
    })
}

fn with_runtime_state_mut<F, R>(f: F) -> Result<R, JsValue>
where
    F: FnOnce(&mut RuntimeState) -> Result<R, JsValue>,
{
    RUNTIME_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let state = borrow.as_mut().ok_or_else(|| {
            err_js(
                "not_initialized",
                "runtime not initialized — call init_runtime or init_runtime_from_config first",
            )
        })?;
        f(state)
    })
}

fn with_mob_state<F, R>(f: F) -> Result<R, JsValue>
where
    F: FnOnce(Arc<MobMcpState>) -> Result<R, JsValue>,
{
    with_runtime_state(|state| f(state.mob_state.clone()))
}

// ═══════════════════════════════════════════════════════════
// Error helpers
// ═══════════════════════════════════════════════════════════

fn err_js(code: &str, message: &str) -> JsValue {
    let json = serde_json::json!({ "code": code, "message": message });
    JsValue::from_str(&json.to_string())
}

fn err_str(code: &str, msg: impl std::fmt::Display) -> JsValue {
    err_js(code, &msg.to_string())
}

fn err_mob(e: meerkat_mob::MobError) -> JsValue {
    err_str("mob_error", e)
}

fn err_session(e: meerkat_core::SessionError) -> JsValue {
    match e {
        meerkat_core::SessionError::NotFound { .. } => {
            err_js("SESSION_NOT_FOUND", "session not found")
        }
        meerkat_core::SessionError::Busy { .. } => err_js("SESSION_BUSY", "session is busy"),
        meerkat_core::SessionError::PersistenceDisabled => err_js(
            "SESSION_PERSISTENCE_DISABLED",
            "session persistence is disabled",
        ),
        meerkat_core::SessionError::CompactionDisabled => err_js(
            "SESSION_COMPACTION_DISABLED",
            "session compaction is disabled",
        ),
        meerkat_core::SessionError::NotRunning { .. } => {
            err_js("SESSION_NOT_RUNNING", "session is not running")
        }
        meerkat_core::SessionError::Unsupported(message) => err_js("SESSION_UNSUPPORTED", &message),
        meerkat_core::SessionError::Store(other) => err_str("internal_error", other),
        meerkat_core::SessionError::Agent(other) => err_str("internal_error", other),
    }
}

fn err_session_control(e: meerkat_core::SessionControlError) -> JsValue {
    match e {
        meerkat_core::SessionControlError::Session(meerkat_core::SessionError::NotFound {
            ..
        }) => err_js("SESSION_NOT_FOUND", "session not found"),
        meerkat_core::SessionControlError::Session(other) => err_str("internal_error", other),
        meerkat_core::SessionControlError::InvalidRequest { message } => {
            err_js("INVALID_PARAMS", &message)
        }
        meerkat_core::SessionControlError::Conflict { key, .. } => err_js(
            "SESSION_SYSTEM_CONTEXT_CONFLICT",
            &format!("system-context idempotency conflict for key '{key}'"),
        ),
    }
}

async fn resolve_mob_member_session_id(
    mob_state: &MobMcpState,
    mob_id: &MobId,
    meerkat_id: &MeerkatId,
) -> Result<meerkat_core::SessionId, JsValue> {
    let members = mob_state.mob_list_members(mob_id).await.map_err(err_mob)?;
    let entry = members
        .iter()
        .find(|m| m.meerkat_id == *meerkat_id)
        .ok_or_else(|| {
            err_js(
                "member_not_found",
                &format!("meerkat '{meerkat_id}' not in mob '{mob_id}'"),
            )
        })?;

    entry
        .member_ref
        .session_id()
        .ok_or_else(|| {
            err_js(
                "no_session",
                &format!("meerkat '{meerkat_id}' has no session"),
            )
        })
        .cloned()
}

// ═══════════════════════════════════════════════════════════
// Mobpack Parsing
// ═══════════════════════════════════════════════════════════

#[derive(Debug)]
struct ParsedMobpack {
    manifest: WebManifest,
    definition: MobDefinitionHeader,
    skills: BTreeMap<String, String>,
}

fn parse_mobpack(bytes: &[u8]) -> Result<ParsedMobpack, String> {
    let files =
        extract_targz_safe(bytes).map_err(|e| format!("failed to parse mobpack archive: {e}"))?;

    let manifest_text = std::str::from_utf8(
        files
            .get("manifest.toml")
            .ok_or_else(|| "manifest.toml is missing".to_string())?,
    )
    .map_err(|e| format!("manifest.toml is not valid UTF-8: {e}"))?;

    let manifest: WebManifest =
        toml::from_str(manifest_text).map_err(|e| format!("invalid manifest.toml: {e}"))?;

    let definition: MobDefinitionHeader = serde_json::from_slice(
        files
            .get("definition.json")
            .ok_or_else(|| "definition.json is missing".to_string())?,
    )
    .map_err(|e| format!("invalid definition.json: {e}"))?;

    if let Some(requires) = &manifest.requires {
        for cap in &requires.capabilities {
            if FORBIDDEN_CAPABILITIES.contains(&cap.as_str()) {
                return Err(format!(
                    "forbidden capability '{cap}' is not allowed in browser-safe mode"
                ));
            }
        }
    }

    let mut skills = BTreeMap::new();
    for (path, content) in &files {
        if let Some(name) = path.strip_prefix("skills/") {
            let text = std::str::from_utf8(content)
                .map_err(|e| format!("skill file '{path}' is not valid UTF-8: {e}"))?;
            let key = name
                .strip_suffix(".md")
                .or_else(|| name.strip_suffix(".txt"))
                .unwrap_or(name);
            skills.insert(key.to_string(), text.to_string());
        }
    }

    Ok(ParsedMobpack {
        manifest,
        definition,
        skills,
    })
}

fn compile_system_prompt(
    mob_id: &str,
    mob_name: &str,
    skills: &BTreeMap<String, String>,
    user_system_prompt: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    parts.push(format!("# Mob: {mob_name} ({mob_id})"));
    for (name, content) in skills {
        parts.push(format!("## Skill: {name}\n\n{content}"));
    }
    if let Some(extra) = user_system_prompt {
        parts.push(extra.to_string());
    }
    let joined = parts.join(SKILL_SEPARATOR);
    if joined.len() > MAX_SYSTEM_PROMPT_BYTES {
        let mut i = MAX_SYSTEM_PROMPT_BYTES.min(joined.len());
        while i > 0 && !joined.is_char_boundary(i) {
            i -= 1;
        }
        joined[..i].to_string()
    } else {
        joined
    }
}

// ═══════════════════════════════════════════════════════════
// Archive Extraction
// ═══════════════════════════════════════════════════════════

fn extract_targz_safe(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let decoder = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(decoder);
    let mut files = BTreeMap::new();
    let entries = archive
        .entries()
        .map_err(|e| format!("failed to read archive entries: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("failed reading archive entry: {e}"))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err("archive contains unsupported entry type".to_string());
        }
        let path = entry
            .path()
            .map_err(|e| format!("invalid archive path: {e}"))?;
        if !kind.is_file() {
            continue;
        }
        let normalized = normalize_for_archive(path.to_string_lossy().as_ref())?;
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .map_err(|e| format!("failed reading archive file '{normalized}': {e}"))?;
        files.insert(normalized, contents);
    }
    Ok(files)
}

fn normalize_for_archive(path: &str) -> Result<String, String> {
    let replaced = path.replace('\\', "/");
    if replaced.starts_with('/') || looks_like_windows_absolute(&replaced) {
        return Err("archive contains absolute path entry".to_string());
    }
    let mut parts = Vec::new();
    for segment in replaced.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err("archive contains parent directory traversal entry".to_string());
        }
        parts.push(segment);
    }
    if parts.is_empty() {
        return Err("archive contains empty path entry".to_string());
    }
    Ok(parts.join("/"))
}

fn looks_like_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

// ═══════════════════════════════════════════════════════════
// Provider Resolution
// ═══════════════════════════════════════════════════════════

fn create_llm_client(
    model: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> Result<Arc<dyn meerkat_client::types::LlmClient>, String> {
    if model.starts_with("claude") {
        let mut builder = meerkat_client::anthropic::AnthropicClientBuilder::new(api_key.into());
        if let Some(url) = base_url {
            builder = builder.base_url(url.into());
        }
        let client = builder
            .build()
            .map_err(|e| format!("failed to create Anthropic client: {e}"))?;
        Ok(Arc::new(client))
    } else if model.starts_with("gpt") || model.starts_with("chatgpt") {
        let client = if let Some(url) = base_url {
            meerkat_client::openai::OpenAiClient::new_with_base_url(api_key.into(), url.into())
        } else {
            meerkat_client::openai::OpenAiClient::new(api_key.into())
        };
        Ok(Arc::new(client))
    } else {
        let client = if let Some(url) = base_url {
            meerkat_client::gemini::GeminiClient::new_with_base_url(api_key.into(), url.into())
        } else {
            meerkat_client::gemini::GeminiClient::new(api_key.into())
        };
        Ok(Arc::new(client))
    }
}

// ═══════════════════════════════════════════════════════════
// JS Tool Callbacks — register tool implementations from JavaScript
// ═══════════════════════════════════════════════════════════

/// How a JS-registered tool is dispatched.
#[cfg(target_arch = "wasm32")]
enum JsToolMode {
    /// Calls a JS callback and awaits its Promise result.
    Callback(js_sys::Function),
    /// Returns `"acknowledged"` immediately. The tool call appears in the event
    /// stream (`ToolCallRequested`) for the host to act on asynchronously.
    FireAndForget,
}

#[cfg(target_arch = "wasm32")]
struct JsToolEntry {
    def: Arc<meerkat_core::ToolDef>,
    mode: JsToolMode,
}

/// Register a tool implementation from JavaScript.
///
/// Requires initialized runtime state.
/// The `callback` receives a JSON string of tool arguments and must return
/// a `Promise<string>` resolving to JSON `{"content": "...", "is_error": false}`.
///
/// Example (JS):
/// ```js
/// register_tool_callback("shell", '{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}', shellFn);
/// ```
#[wasm_bindgen]
pub fn register_tool_callback(
    name: String,
    description: String,
    schema_json: String,
    callback: JsValue,
) -> Result<(), JsValue> {
    #[cfg(target_arch = "wasm32")]
    {
        let func: js_sys::Function = callback
            .dyn_into()
            .map_err(|_| err_js("invalid_callback", "callback must be a function"))?;

        let schema: serde_json::Value = serde_json::from_str(&schema_json)
            .map_err(|e| err_str("invalid_schema", format!("invalid JSON schema: {e}")))?;

        let def = Arc::new(meerkat_core::ToolDef {
            name,
            description,
            input_schema: schema,
        });

        with_runtime_state_mut(|state| {
            state.js_tools.retain(|e| e.def.name != def.name);
            state.js_tools.push(JsToolEntry {
                def,
                mode: JsToolMode::Callback(func),
            });
            Ok(())
        })?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (name, description, schema_json, callback);
    }
    Ok(())
}

/// Register a fire-and-forget tool from JavaScript.
///
/// Requires initialized runtime state.
/// When the agent calls this tool, `dispatch()` returns `"acknowledged"`
/// immediately and the agent loop continues without waiting.
///
/// The host should watch `ToolCallRequested` events in the event stream to
/// capture the tool call arguments and act on them asynchronously. Any
/// response (e.g. human approval) should be sent back via `mob_member_send`.
///
/// Example (JS):
/// ```js
/// register_js_tool(
///   "request_human_approval",
///   "Escalate a high-risk action for human sign-off",
///   JSON.stringify({
///     type: "object",
///     properties: {
///       summary: { type: "string" },
///       risk_level: { type: "string", enum: ["low", "medium", "high"] },
///     },
///     required: ["summary", "risk_level"],
///   }),
/// );
/// ```
#[wasm_bindgen]
pub fn register_js_tool(
    name: String,
    description: String,
    schema_json: String,
) -> Result<(), JsValue> {
    #[cfg(target_arch = "wasm32")]
    {
        let schema: serde_json::Value = serde_json::from_str(&schema_json)
            .map_err(|e| err_str("invalid_schema", format!("invalid JSON schema: {e}")))?;

        let def = Arc::new(meerkat_core::ToolDef {
            name,
            description,
            input_schema: schema,
        });

        with_runtime_state_mut(|state| {
            state.js_tools.retain(|e| e.def.name != def.name);
            state.js_tools.push(JsToolEntry {
                def,
                mode: JsToolMode::FireAndForget,
            });
            Ok(())
        })?;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (name, description, schema_json);
    }
    Ok(())
}

/// Clear all registered tool callbacks.
#[wasm_bindgen]
pub fn clear_tool_callbacks() {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = with_runtime_state_mut(|state| {
            state.js_tools.clear();
            Ok(())
        });
    }
}

/// Return the crate version embedded at compile time.
///
/// Used by the `@rkat/web` TypeScript wrapper to validate that the JS glue
/// and the WASM binary were built from the same version.
#[wasm_bindgen]
pub fn runtime_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Tool dispatcher that delegates to JS callbacks stored in the thread-local registry.
///
/// The dispatcher itself stores no JS state. It resolves tool definitions and
/// callbacks from the initialized runtime's tool registry on each access,
/// avoiding `!Send` callback state inside the dispatcher.
#[cfg(target_arch = "wasm32")]
struct JsToolDispatcher;

#[cfg(target_arch = "wasm32")]
impl JsToolDispatcher {
    fn tools_from_runtime() -> Arc<[Arc<meerkat_core::ToolDef>]> {
        RUNTIME_STATE.with(|cell| {
            let borrow = cell.borrow();
            let Some(state) = borrow.as_ref() else {
                return Vec::<Arc<meerkat_core::ToolDef>>::new().into();
            };
            let defs: Vec<_> = state
                .js_tools
                .iter()
                .map(|entry| entry.def.clone())
                .collect();
            defs.into()
        })
    }

    /// Check whether a tool is registered as fire-and-forget.
    fn is_fire_and_forget(name: &str) -> bool {
        RUNTIME_STATE.with(|cell| {
            let borrow = cell.borrow();
            let Some(state) = borrow.as_ref() else {
                return false;
            };
            state
                .js_tools
                .iter()
                .find(|e| e.def.name == name)
                .map(|e| matches!(e.mode, JsToolMode::FireAndForget))
                .unwrap_or(false)
        })
    }

    /// Look up the JS callback for a tool by name from initialized runtime state.
    fn get_callback(name: &str) -> Option<js_sys::Function> {
        RUNTIME_STATE.with(|cell| {
            let borrow = cell.borrow();
            let state = borrow.as_ref()?;
            state.js_tools.iter().find_map(|e| {
                if e.def.name == name {
                    match &e.mode {
                        JsToolMode::Callback(f) => Some(f.clone()),
                        JsToolMode::FireAndForget => None,
                    }
                } else {
                    None
                }
            })
        })
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait::async_trait(?Send)]
impl meerkat_core::AgentToolDispatcher for JsToolDispatcher {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Self::tools_from_runtime()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::ToolError> {
        // Fire-and-forget tools return immediately. The host watches
        // ToolCallRequested events in the stream to act on the call.
        if Self::is_fire_and_forget(call.name) {
            return Ok(meerkat_core::ToolResult::new(
                call.id.to_string(),
                "acknowledged".to_string(),
                false,
            )
            .into());
        }

        let callback = Self::get_callback(call.name)
            .ok_or_else(|| meerkat_core::error::ToolError::not_found(call.name))?;

        let args_str = call.args.get();
        let js_args = JsValue::from_str(args_str);
        let promise_val = callback.call1(&JsValue::NULL, &js_args).map_err(|e| {
            meerkat_core::error::ToolError::execution_failed(format!("JS callback threw: {e:?}"))
        })?;
        let promise = js_sys::Promise::from(promise_val);
        let result_val = wasm_bindgen_futures::JsFuture::from(promise)
            .await
            .map_err(|e| {
                meerkat_core::error::ToolError::execution_failed(format!(
                    "JS promise rejected: {e:?}"
                ))
            })?;
        let result_str = result_val.as_string().unwrap_or_default();

        // Parse JSON result: {"content": "...", "is_error": false}
        #[derive(Deserialize)]
        struct JsToolResult {
            content: String,
            #[serde(default)]
            is_error: bool,
        }
        let parsed: JsToolResult = serde_json::from_str(&result_str).map_err(|e| {
            meerkat_core::error::ToolError::execution_failed(format!(
                "invalid tool result JSON: {e}"
            ))
        })?;

        Ok(
            meerkat_core::ToolResult::new(call.id.to_string(), parsed.content, parsed.is_error)
                .into(),
        )
    }
}

// ═══════════════════════════════════════════════════════════
// WASM Tool Dispatcher
// ═══════════════════════════════════════════════════════════

#[cfg(target_arch = "wasm32")]
fn build_wasm_tool_dispatcher() -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, String> {
    let task_store: Arc<dyn meerkat_tools::builtin::TaskStore> =
        Arc::new(meerkat_tools::builtin::MemoryTaskStore::new());
    let config = meerkat_tools::builtin::BuiltinToolConfig::default();
    let external = Some(Arc::new(JsToolDispatcher) as Arc<dyn meerkat_core::AgentToolDispatcher>);
    let composite =
        meerkat_tools::builtin::CompositeDispatcher::new_wasm(task_store, &config, external, None)
            .map_err(|e| format!("failed to create tool dispatcher: {e}"))?;
    Ok(Arc::new(composite))
}

#[cfg(not(target_arch = "wasm32"))]
fn build_wasm_tool_dispatcher() -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, String> {
    Ok(Arc::new(meerkat_tools::EmptyToolDispatcher))
}

// ═══════════════════════════════════════════════════════════
// Bootstrap: init_runtime
// ═══════════════════════════════════════════════════════════

/// Primary bootstrap: parse a mobpack and create service infrastructure.
///
/// `mobpack_bytes`: tar.gz mobpack archive.
/// `credentials_json`: `{ "api_key": "sk-...", "anthropic_api_key"?: "...", "openai_api_key"?: "...", "gemini_api_key"?: "...", "model"?: "claude-sonnet-4-5" }`
///
/// Stores an `EphemeralSessionService<FactoryAgentBuilder>` and a `MobMcpState`
/// in a `thread_local! RuntimeState` for subsequent mob/comms calls.
#[wasm_bindgen]
pub fn init_runtime(mobpack_bytes: &[u8], credentials_json: &str) -> Result<JsValue, JsValue> {
    init_tracing();
    patch_fetch_response_url();
    let _parsed = parse_mobpack(mobpack_bytes).map_err(|e| err_str("invalid_mobpack", e))?;
    let creds: Credentials =
        serde_json::from_str(credentials_json).map_err(|e| err_str("invalid_credentials", e))?;

    let api_keys = build_provider_api_keys(
        creds.api_key.as_deref(),
        creds.anthropic_api_key.as_deref(),
        creds.openai_api_key.as_deref(),
        creds.gemini_api_key.as_deref(),
    )?;

    let model = creds.model.unwrap_or_else(|| {
        meerkat_models::default_model("anthropic")
            .unwrap_or("claude-sonnet-4-5")
            .to_string()
    });

    let providers: Vec<String> = api_keys.keys().cloned().collect();
    let mut config = Config::default();
    config.agent.model.clone_from(&model);
    config.providers.api_keys = Some(api_keys);
    config.providers.base_urls = build_provider_base_urls(
        creds.base_url.as_deref(),
        creds.anthropic_base_url.as_deref(),
        creds.openai_base_url.as_deref(),
        creds.gemini_base_url.as_deref(),
        &model,
    );

    let (session_service, mob_state) = build_service_infrastructure(config, MAX_SESSIONS)?;

    install_runtime_state(RuntimeState {
        mob_state,
        session_service,
        sessions: BTreeMap::new(),
        next_handle: 1,
        model: model.clone(),
        #[cfg(target_arch = "wasm32")]
        js_tools: Vec::new(),
    });

    let result = serde_json::json!({
        "status": "initialized",
        "model": model,
        "providers": providers,
    });
    Ok(JsValue::from_str(&result.to_string()))
}

// ═══════════════════════════════════════════════════════════
// Bootstrap: init_runtime_from_config
// ═══════════════════════════════════════════════════════════

/// Advanced bare-bones bootstrap without a mobpack.
///
/// `config_json`: `{ "api_key"?: "sk-...", "anthropic_api_key"?: "...", "openai_api_key"?: "...", "gemini_api_key"?: "...", "model"?: "claude-sonnet-4-5", "max_sessions"?: 64 }`
#[wasm_bindgen]
pub fn init_runtime_from_config(config_json: &str) -> Result<JsValue, JsValue> {
    init_tracing();
    patch_fetch_response_url();
    let rt_config: RuntimeConfig =
        serde_json::from_str(config_json).map_err(|e| err_str("invalid_config", e))?;

    let api_keys = build_provider_api_keys(
        rt_config.api_key.as_deref(),
        rt_config.anthropic_api_key.as_deref(),
        rt_config.openai_api_key.as_deref(),
        rt_config.gemini_api_key.as_deref(),
    )?;

    let model = rt_config.model.unwrap_or_else(|| {
        meerkat_models::default_model("anthropic")
            .unwrap_or("claude-sonnet-4-5")
            .to_string()
    });
    let max_sessions = rt_config.max_sessions;

    let providers: Vec<String> = api_keys.keys().cloned().collect();
    let mut config = Config::default();
    config.agent.model.clone_from(&model);
    config.providers.api_keys = Some(api_keys);
    config.providers.base_urls = build_provider_base_urls(
        rt_config.base_url.as_deref(),
        rt_config.anthropic_base_url.as_deref(),
        rt_config.openai_base_url.as_deref(),
        rt_config.gemini_base_url.as_deref(),
        &model,
    );

    let (session_service, mob_state) = build_service_infrastructure(config, max_sessions)?;

    install_runtime_state(RuntimeState {
        mob_state,
        session_service,
        sessions: BTreeMap::new(),
        next_handle: 1,
        model: model.clone(),
        #[cfg(target_arch = "wasm32")]
        js_tools: Vec::new(),
    });

    let result = serde_json::json!({
        "status": "initialized",
        "model": model,
        "max_sessions": max_sessions,
        "providers": providers,
    });
    Ok(JsValue::from_str(&result.to_string()))
}

// ═══════════════════════════════════════════════════════════
// Exported WASM API — Session (runtime-backed session-handle façades)
// ═══════════════════════════════════════════════════════════

fn next_runtime_handle(state: &mut RuntimeState) -> u32 {
    let handle = state.next_handle.max(1);
    state.next_handle = handle.wrapping_add(1);
    while state.next_handle == 0 || state.sessions.contains_key(&state.next_handle) {
        state.next_handle = state.next_handle.wrapping_add(1);
    }
    handle
}

fn build_direct_session_request(
    config: &SessionConfig,
    system_prompt: Option<String>,
) -> Result<meerkat_core::service::CreateSessionRequest, JsValue> {
    if config.model.trim().is_empty() {
        return Err(err_js("invalid_config", "model must not be empty"));
    }
    let api_key = config.api_key.as_deref().unwrap_or("");
    if api_key.is_empty() {
        return Err(err_js("invalid_config", "api_key must not be empty"));
    }

    // Resolve effective base URL: per-provider fields take precedence over single base_url.
    let effective_base_url = resolve_session_base_url(
        config.base_url.as_deref(),
        config.anthropic_base_url.as_deref(),
        config.openai_base_url.as_deref(),
        config.gemini_base_url.as_deref(),
        &config.model,
    );

    // Create LLM client.
    let llm_client = create_llm_client(&config.model, api_key, effective_base_url.as_deref())
        .map_err(|e| err_str("provider_error", e))?;

    let mut build_config = AgentBuildConfig::new(&config.model);
    build_config.llm_client_override = Some(llm_client);
    if let Some(name) = config.comms_name.clone() {
        build_config.comms_name = Some(name);
        build_config.keep_alive = config.keep_alive;
    }
    build_config.additional_instructions = config.additional_instructions.clone();
    build_config.app_context = config.app_context.clone();

    Ok(meerkat_core::service::CreateSessionRequest {
        model: config.model.clone(),
        prompt: "".into(),
        render_metadata: None,
        system_prompt,
        max_tokens: Some(config.max_tokens),
        event_tx: None,

        skill_references: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        build: Some(build_config.to_session_build_options()),
        labels: config.labels.clone(),
    })
}

fn create_runtime_backed_session(
    config: SessionConfig,
    system_prompt: Option<String>,
    mob_id: String,
) -> Result<u32, JsValue> {
    let session_service = with_runtime_state(|state| Ok(state.session_service.clone()))?;
    let keep_alive = config.comms_name.is_some() && config.keep_alive;
    let request = build_direct_session_request(&config, system_prompt)?;

    let created = futures::executor::block_on(session_service.create_session(request))
        .map_err(err_session)?;
    let session_id = created.session_id;
    let event_rx = match futures::executor::block_on(
        session_service.subscribe_session_events_raw(&session_id),
    ) {
        Ok(rx) => rx,
        Err(err) => {
            let _ = futures::executor::block_on(session_service.archive(&session_id));
            return Err(err_str("session_event_stream_error", err));
        }
    };

    with_runtime_state_mut(|state| {
        let handle = next_runtime_handle(state);
        state.sessions.insert(
            handle,
            RuntimeHandleSession {
                session_id,
                mob_id,
                model: config.model,
                keep_alive,
                run_counter: 0,
                event_rx,
            },
        );
        Ok(handle)
    })
}

/// Create a session from a mobpack + config.
///
/// Allocates a real session through the initialized runtime-backed session
/// service, then returns a browser-local session handle for convenience.
#[wasm_bindgen]
pub fn create_session(mobpack_bytes: &[u8], config_json: &str) -> Result<u32, JsValue> {
    let parsed = parse_mobpack(mobpack_bytes).map_err(|e| err_str("invalid_mobpack", e))?;
    let config: SessionConfig =
        serde_json::from_str(config_json).map_err(|e| err_str("invalid_config", e))?;
    let system_prompt = compile_system_prompt(
        &parsed.definition.id,
        &parsed.manifest.mobpack.name,
        &parsed.skills,
        config.system_prompt.as_deref(),
    );
    create_runtime_backed_session(config, Some(system_prompt), parsed.definition.id)
}

/// Create a standalone session through initialized runtime state.
#[wasm_bindgen]
pub fn create_session_simple(config_json: &str) -> Result<u32, JsValue> {
    let config: SessionConfig =
        serde_json::from_str(config_json).map_err(|e| err_str("invalid_config", e))?;
    let system_prompt = config.system_prompt.clone();
    create_runtime_backed_session(config, system_prompt, String::new())
}

/// Per-turn options parsed from `options_json`.
#[derive(Debug, Default, Deserialize)]
struct TurnOptions {
    /// Additional instruction sections appended for this turn only.
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct AppendSystemContextOptions {
    text: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[cfg(test)]
fn merge_runtime_system_context_state(
    mut agent_state: meerkat_core::SessionSystemContextState,
    starting_state: &meerkat_core::SessionSystemContextState,
    current_state: &meerkat_core::SessionSystemContextState,
) -> meerkat_core::SessionSystemContextState {
    for pending in &current_state.pending {
        if !starting_state.pending.contains(pending) {
            agent_state.pending.push(pending.clone());
        }
    }

    for (key, seen) in &current_state.seen {
        if !starting_state.seen.contains_key(key) {
            agent_state.seen.insert(key.clone(), seen.clone());
        }
    }

    agent_state
}

/// Append runtime system context to a browser session handle.
#[wasm_bindgen]
pub fn append_system_context(handle: u32, request_json: &str) -> Result<JsValue, JsValue> {
    let req: AppendSystemContextOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;

    let (session_service, session_id) = with_runtime_state(|state| {
        let session = state
            .sessions
            .get(&handle)
            .ok_or_else(|| err_js("SESSION_NOT_FOUND", &format!("unknown handle: {handle}")))?;
        Ok((state.session_service.clone(), session.session_id.clone()))
    })?;
    let status = futures::executor::block_on(session_service.append_system_context(
        &session_id,
        meerkat_core::AppendSystemContextRequest {
            text: req.text,
            source: req.source,
            idempotency_key: req.idempotency_key,
        },
    ))
    .map_err(err_session_control)?
    .status;

    Ok(JsValue::from_str(
        &serde_json::json!({
            "handle": handle,
            "session_id": session_id.to_string(),
            "status": status,
        })
        .to_string(),
    ))
}

/// Run a turn through the runtime-backed session service.
///
/// Returns JSON: `{ "text", "usage", "status", "session_id", "turns", "tool_calls" }`
///
/// Convention: always resolves (Ok). Check `status` field for "completed" vs "failed".
/// Only rejects (Err) for infrastructure errors (session not found, busy, etc).
/// Agent-level errors (LLM failure, timeout) resolve with `status: "failed"` + `error` field.
#[wasm_bindgen]
pub async fn start_turn(handle: u32, prompt: &str, options_json: &str) -> Result<JsValue, JsValue> {
    let options: TurnOptions = if options_json.is_empty() || options_json == "{}" {
        TurnOptions::default()
    } else {
        serde_json::from_str(options_json).map_err(|e| err_str("invalid_options", e))?
    };
    let (session_service, session_id, run_id, _keep_alive) = with_runtime_state_mut(|state| {
        let session = state
            .sessions
            .get_mut(&handle)
            .ok_or_else(|| err_js("SESSION_NOT_FOUND", &format!("unknown handle: {handle}")))?;
        session.run_counter = session.run_counter.saturating_add(1);
        Ok((
            state.session_service.clone(),
            session.session_id.clone(),
            session.run_counter,
            session.keep_alive,
        ))
    })?;

    // Parse the prompt as structured ContentInput (supports both plain strings
    // and JSON-serialized content blocks from the Web SDK). Falls back to plain
    // text when the prompt is not valid ContentInput JSON.
    let content_input: meerkat_core::types::ContentInput =
        serde_json::from_str(prompt).unwrap_or_else(|_| prompt.into());

    let run_result = session_service
        .start_turn(
            &session_id,
            meerkat_core::service::StartTurnRequest {
                prompt: content_input,
                system_prompt: None,
                render_metadata: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                event_tx: None,

                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: options.additional_instructions,
            },
        )
        .await;

    match run_result {
        Ok(result) => {
            let result_json = serde_json::json!({
                "run_id": run_id,
                "text": result.text,
                "usage": {
                    "input_tokens": result.usage.input_tokens,
                    "output_tokens": result.usage.output_tokens,
                },
                "session_id": result.session_id.to_string(),
                "status": "completed",
                "turns": result.turns,
                "tool_calls": result.tool_calls,
            });
            Ok(JsValue::from_str(&result_json.to_string()))
        }
        Err(meerkat_core::SessionError::Agent(err)) => {
            let error_msg = format!("{err}");
            let result_json = serde_json::json!({
                "run_id": run_id,
                "text": "",
                "usage": { "input_tokens": 0, "output_tokens": 0 },
                "session_id": session_id.to_string(),
                "status": "failed",
                "error": error_msg,
            });
            Ok(JsValue::from_str(&result_json.to_string()))
        }
        Err(err) => Err(err_session(err)),
    }
}

/// Get current session state.
#[wasm_bindgen]
pub fn get_session_state(handle: u32) -> Result<String, JsValue> {
    let (session_service, session_id, mob_id, model, run_counter) = with_runtime_state(|state| {
        let session = state
            .sessions
            .get(&handle)
            .ok_or_else(|| err_js("SESSION_NOT_FOUND", &format!("unknown handle: {handle}")))?;
        Ok((
            state.session_service.clone(),
            session.session_id.clone(),
            session.mob_id.clone(),
            session.model.clone(),
            session.run_counter,
        ))
    })?;
    let view =
        futures::executor::block_on(session_service.read(&session_id)).map_err(err_session)?;
    let state = serde_json::json!({
        "handle": handle,
        "session_id": session_id.to_string(),
        "mob_id": mob_id,
        "model": model,
        "usage": view.billing.usage,
        "run_counter": run_counter,
        "message_count": view.state.message_count,
        "is_active": view.state.is_active,
        "last_assistant_text": view.state.last_assistant_text,
    });
    Ok(state.to_string())
}

/// Inspect a mobpack without creating a session.
#[wasm_bindgen]
pub fn inspect_mobpack(mobpack_bytes: &[u8]) -> Result<String, JsValue> {
    let parsed = parse_mobpack(mobpack_bytes).map_err(|e| err_str("invalid_mobpack", e))?;
    let skills: Vec<serde_json::Value> = parsed
        .skills
        .iter()
        .map(|(name, content)| {
            serde_json::json!({
                "name": name,
                "size_bytes": content.len(),
            })
        })
        .collect();
    let result = serde_json::json!({
        "manifest": {
            "name": parsed.manifest.mobpack.name,
            "version": parsed.manifest.mobpack.version,
            "description": parsed.manifest.mobpack.description,
        },
        "definition": { "id": parsed.definition.id },
        "skills": skills,
        "capabilities": parsed.manifest.requires.as_ref().map(|r| &r.capabilities),
    });
    serde_json::to_string(&result).map_err(|e| err_str("serialize_error", e))
}

/// Remove a session.
#[wasm_bindgen]
pub fn destroy_session(handle: u32) -> Result<(), JsValue> {
    let (session_service, session_id) = with_runtime_state(|state| {
        let session = state
            .sessions
            .get(&handle)
            .ok_or_else(|| err_js("SESSION_NOT_FOUND", &format!("unknown handle: {handle}")))?;
        Ok((state.session_service.clone(), session.session_id.clone()))
    })?;
    futures::executor::block_on(session_service.archive(&session_id)).map_err(err_session)?;
    with_runtime_state_mut(|state| {
        state.sessions.remove(&handle);
        Ok(())
    })
}

/// Drain and return all pending agent events from the last turn(s).
///
/// Returns a JSON array of `AgentEvent` objects. Each call drains the buffer;
/// subsequent calls return `[]` until the next `start_turn` produces more events.
#[wasm_bindgen]
pub fn poll_events(handle: u32) -> Result<String, JsValue> {
    with_runtime_state_mut(|state| {
        let session = state
            .sessions
            .get_mut(&handle)
            .ok_or_else(|| err_js("SESSION_NOT_FOUND", &format!("unknown handle: {handle}")))?;
        let mut events = Vec::new();
        loop {
            match session.event_rx.try_recv() {
                Ok(envelope) => events.push(
                    serde_json::to_value(&envelope.payload)
                        .map_err(|err| err_str("serialize_error", err))?,
                ),
                Err(crate::tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(crate::tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                Err(crate::tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                    events.push(serde_json::json!({
                        "type": "lagged",
                        "skipped": skipped,
                    }));
                }
            }
        }
        serde_json::to_string(&events).map_err(|err| err_str("serialize_error", err))
    })
}

// ═══════════════════════════════════════════════════════════
// Mob Lifecycle Exports (delegates to MobMcpState)
// ═══════════════════════════════════════════════════════════

/// Create a new mob from a definition JSON.
///
/// Returns the mob_id as a string.
#[wasm_bindgen]
pub async fn mob_create(definition_json: &str) -> Result<JsValue, JsValue> {
    let definition: MobDefinition =
        serde_json::from_str(definition_json).map_err(|e| err_str("invalid_definition", e))?;
    let mob_state = with_mob_state(Ok)?;
    let mob_id = mob_state
        .mob_create_definition(definition)
        .await
        .map_err(err_mob)?;
    Ok(JsValue::from_str(mob_id.as_ref()))
}

/// Get the status of a mob.
///
/// Returns JSON with the mob state.
#[wasm_bindgen]
pub async fn mob_status(mob_id: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let state = mob_state.mob_status(&id).await.map_err(err_mob)?;
    let result = serde_json::json!({
        "mob_id": mob_id,
        "state": state.as_str(),
    });
    Ok(JsValue::from_str(&result.to_string()))
}

/// List all mobs.
///
/// Returns JSON array of `{ mob_id, state }`.
#[wasm_bindgen]
pub async fn mob_list() -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let mobs = mob_state.mob_list().await;
    let result: Vec<serde_json::Value> = mobs
        .into_iter()
        .map(|(id, state)| {
            serde_json::json!({
                "mob_id": id.to_string(),
                "state": state.as_str(),
            })
        })
        .collect();
    let json = serde_json::to_string(&result).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
}

/// Perform a lifecycle action on a mob.
///
/// `action`: one of "stop", "resume", "complete", "reset", "destroy".
#[wasm_bindgen]
pub async fn mob_lifecycle(mob_id: &str, action: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    match action {
        "stop" => mob_state.mob_stop(&id).await.map_err(err_mob)?,
        "resume" => mob_state.mob_resume(&id).await.map_err(err_mob)?,
        "complete" => mob_state.mob_complete(&id).await.map_err(err_mob)?,
        "reset" => mob_state.mob_reset(&id).await.map_err(err_mob)?,
        "destroy" => mob_state.mob_destroy(&id).await.map_err(err_mob)?,
        _ => {
            return Err(err_js(
                "invalid_action",
                &format!(
                    "unknown lifecycle action: {action} (expected stop/resume/complete/reset/destroy)"
                ),
            ));
        }
    }
    Ok(())
}

/// Fetch mob events.
///
/// Returns JSON array of mob events.
///
/// Note: `after_cursor` is u32 at the JS boundary (wasm_bindgen limitation),
/// internally widened to u64. Cursors beyond 4B are not supported via this export.
#[wasm_bindgen]
pub async fn mob_events(mob_id: &str, after_cursor: u32, limit: u32) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let events = mob_state
        .mob_events(&id, after_cursor as u64, limit as usize)
        .await
        .map_err(err_mob)?;
    let json = serde_json::to_string(&events).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
}

/// Spawn one or more meerkats in a mob.
///
/// `specs_json`: JSON array of `{ "profile": "...", "meerkat_id": "...", "initial_message"?: "...",
///                "runtime_mode"?: "autonomous_host"|"turn_driven", "backend"?: "session"|"external" }`
///
/// Returns JSON array of results per spec.
#[wasm_bindgen]
pub async fn mob_spawn(mob_id: &str, specs_json: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let specs: Vec<SpawnSpecInput> =
        serde_json::from_str(specs_json).map_err(|e| err_str("invalid_specs", e))?;

    let mut spawn_specs = Vec::with_capacity(specs.len());
    for s in specs {
        let mut spec = meerkat_mob::SpawnMemberSpec::new(s.profile.as_str(), s.meerkat_id.as_str());
        spec.initial_message = s.initial_message;
        spec.runtime_mode = s.runtime_mode;
        spec.backend = s.backend;
        spec.context = s.context;
        spec.labels = s.labels;
        if let Some(id) = s.resume_session_id {
            let sid = meerkat_core::SessionId::parse(&id).map_err(|_| {
                err_js(
                    "invalid_resume_session_id",
                    &format!("invalid session ID: {id}"),
                )
            })?;
            spec = spec.with_resume_session_id(sid);
        }
        spec.additional_instructions = s.additional_instructions;
        spawn_specs.push(spec);
    }

    let results = mob_state
        .mob_spawn_many(&id, spawn_specs)
        .await
        .map_err(err_mob)?;

    let result_json: Vec<serde_json::Value> = results
        .into_iter()
        .map(|r| match r {
            Ok(member_ref) => serde_json::json!({
                "status": "ok",
                "member_ref": match serde_json::to_value(&member_ref) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to serialize member_ref");
                        serde_json::Value::Null
                    }
                },
            }),
            Err(e) => serde_json::json!({
                "status": "error",
                "error": e.to_string(),
            }),
        })
        .collect();

    let json = serde_json::to_string(&result_json).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
}

#[derive(Debug, Deserialize)]
struct SpawnSpecInput {
    profile: String,
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<meerkat_core::types::ContentInput>,
    #[serde(default)]
    runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    backend: Option<meerkat_mob::MobBackendKind>,
    /// Resume an existing session instead of creating a new one.
    #[serde(default)]
    resume_session_id: Option<String>,
    /// Application-defined labels for this member.
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    /// Opaque application context passed through to the agent build pipeline.
    #[serde(default)]
    context: Option<serde_json::Value>,
    /// Additional instruction sections appended to the system prompt.
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
}

/// Retire a meerkat from a mob.
#[wasm_bindgen]
pub async fn mob_retire(mob_id: &str, meerkat_id: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);
    mob_state.mob_retire(&id, mid).await.map_err(err_mob)
}

/// Wire bidirectional trust between two meerkats.
#[wasm_bindgen]
pub async fn mob_wire(mob_id: &str, a: &str, b: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    mob_state
        .mob_wire(
            &id,
            MeerkatId::from(a),
            meerkat_mob::PeerTarget::Local(MeerkatId::from(b)),
        )
        .await
        .map_err(err_mob)
}

/// Wire a local meerkat to a local or external peer target.
#[wasm_bindgen]
pub async fn mob_wire_target(mob_id: &str, local: &str, target_json: &str) -> Result<(), JsValue> {
    mob_wire_peer(mob_id, local, target_json).await
}

/// Wire a local member to a local or external peer target.
#[wasm_bindgen]
pub async fn mob_wire_peer(mob_id: &str, member: &str, peer_json: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let target: meerkat_mob::PeerTarget =
        serde_json::from_str(peer_json).map_err(|e| err_str("invalid_peer_target", e))?;
    mob_state
        .mob_wire(&id, MeerkatId::from(member), target)
        .await
        .map_err(err_mob)
}

/// Unwire bidirectional trust between two meerkats.
#[wasm_bindgen]
pub async fn mob_unwire(mob_id: &str, a: &str, b: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    mob_state
        .mob_unwire(
            &id,
            MeerkatId::from(a),
            meerkat_mob::PeerTarget::Local(MeerkatId::from(b)),
        )
        .await
        .map_err(err_mob)
}

/// Unwire a local meerkat from a local or external peer target.
#[wasm_bindgen]
pub async fn mob_unwire_target(
    mob_id: &str,
    local: &str,
    target_json: &str,
) -> Result<(), JsValue> {
    mob_unwire_peer(mob_id, local, target_json).await
}

/// Unwire a local member from a local or external peer target.
#[wasm_bindgen]
pub async fn mob_unwire_peer(mob_id: &str, member: &str, peer_json: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let target: meerkat_mob::PeerTarget =
        serde_json::from_str(peer_json).map_err(|e| err_str("invalid_peer_target", e))?;
    mob_state
        .mob_unwire(&id, MeerkatId::from(member), target)
        .await
        .map_err(err_mob)
}

/// List all members in a mob.
///
/// Returns JSON array of roster entries.
#[wasm_bindgen]
pub async fn mob_list_members(mob_id: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let members = mob_state.mob_list_members(&id).await.map_err(err_mob)?;
    let json = serde_json::to_string(&members).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
}

/// Append runtime system context to an individual mob member's session.
#[wasm_bindgen]
pub async fn mob_append_system_context(
    mob_id: &str,
    meerkat_id: &str,
    request_json: &str,
) -> Result<JsValue, JsValue> {
    let req: AppendSystemContextOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;
    let mob_id_typed = MobId::from(mob_id);
    let meerkat_id_typed = MeerkatId::from(meerkat_id);

    let (mob_state, session_service) =
        with_runtime_state(|state| Ok((state.mob_state.clone(), state.session_service.clone())))?;
    let session_id =
        resolve_mob_member_session_id(&mob_state, &mob_id_typed, &meerkat_id_typed).await?;
    let result = session_service
        .append_system_context(
            &session_id,
            meerkat_core::AppendSystemContextRequest {
                text: req.text,
                source: req.source,
                idempotency_key: req.idempotency_key,
            },
        )
        .await
        .map_err(err_session_control)?;

    Ok(JsValue::from_str(
        &serde_json::json!({
            "mob_id": mob_id,
            "meerkat_id": meerkat_id,
            "session_id": session_id.to_string(),
            "status": result.status,
        })
        .to_string(),
    ))
}

/// Wire bidirectional comms trust between meerkats in DIFFERENT mobs.
///
/// Unlike `mob_wire` (which is intra-mob), this establishes peer trust across
/// mob boundaries by accessing each member's comms runtime through the shared
/// session service. Both members must have comms enabled.
#[wasm_bindgen]
pub async fn wire_cross_mob(
    mob_a: &str,
    meerkat_a: &str,
    mob_b: &str,
    meerkat_b: &str,
) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;

    // Get session IDs from both mobs' rosters
    let members_a = mob_state
        .mob_list_members(&MobId::from(mob_a))
        .await
        .map_err(err_mob)?;
    let entry_a = members_a
        .iter()
        .find(|m| m.meerkat_id == MeerkatId::from(meerkat_a))
        .ok_or_else(|| err_js("not_found", &format!("{meerkat_a} not in {mob_a}")))?;

    let members_b = mob_state
        .mob_list_members(&MobId::from(mob_b))
        .await
        .map_err(err_mob)?;
    let entry_b = members_b
        .iter()
        .find(|m| m.meerkat_id == MeerkatId::from(meerkat_b))
        .ok_or_else(|| err_js("not_found", &format!("{meerkat_b} not in {mob_b}")))?;

    let sid_a = entry_a
        .member_ref
        .session_id()
        .ok_or_else(|| err_js("no_session", meerkat_a))?;
    let sid_b = entry_b
        .member_ref
        .session_id()
        .ok_or_else(|| err_js("no_session", meerkat_b))?;

    // Get comms runtimes from the shared session service
    let svc = RUNTIME_STATE.with(|cell| {
        let borrow = cell.borrow();
        let state = borrow
            .as_ref()
            .ok_or_else(|| err_js("not_initialized", ""))?;
        Ok::<_, JsValue>(state.session_service.clone())
    })?;

    let comms_a = svc
        .comms_runtime(sid_a)
        .await
        .ok_or_else(|| err_js("no_comms", &format!("{meerkat_a} has no comms runtime")))?;
    let comms_b = svc
        .comms_runtime(sid_b)
        .await
        .ok_or_else(|| err_js("no_comms", &format!("{meerkat_b} has no comms runtime")))?;

    let key_a = comms_a
        .public_key()
        .ok_or_else(|| err_js("no_key", meerkat_a))?;
    let key_b = comms_b
        .public_key()
        .ok_or_else(|| err_js("no_key", meerkat_b))?;

    // Build peer specs with full mob/profile/meerkat addressing
    let name_a = format!("{mob_a}/{}/{meerkat_a}", entry_a.profile);
    let name_b = format!("{mob_b}/{}/{meerkat_b}", entry_b.profile);

    let spec_b = meerkat_core::comms::TrustedPeerSpec::new(
        &name_b,
        key_b.clone(),
        format!("inproc://{name_b}"),
    )
    .map_err(|e| err_str("wire_error", e))?;
    let spec_a =
        meerkat_core::comms::TrustedPeerSpec::new(&name_a, key_a, format!("inproc://{name_a}"))
            .map_err(|e| err_str("wire_error", e))?;

    comms_a
        .add_trusted_peer(spec_b)
        .await
        .map_err(|e| err_str("wire_error", e))?;
    comms_b
        .add_trusted_peer(spec_a)
        .await
        .map_err(|e| err_str("wire_error", e))?;

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct MobMemberSendOptions {
    content: meerkat_core::types::ContentInput,
    #[serde(default)]
    handling_mode: meerkat_core::types::HandlingMode,
    #[serde(default)]
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
}

#[derive(Debug, serde::Deserialize)]
struct MobSpawnHelperOptions {
    prompt: String,
    #[serde(default)]
    meerkat_id: Option<String>,
    #[serde(default)]
    profile_name: Option<String>,
    #[serde(default)]
    runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    backend: Option<meerkat_mob::MobBackendKind>,
}

#[derive(Debug, serde::Deserialize)]
struct MobForkHelperOptions {
    source_member_id: String,
    prompt: String,
    #[serde(default)]
    meerkat_id: Option<String>,
    #[serde(default)]
    profile_name: Option<String>,
    #[serde(default)]
    fork_context: Option<meerkat_mob::ForkContext>,
    #[serde(default)]
    runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    backend: Option<meerkat_mob::MobBackendKind>,
}

/// Send external work to a spawned meerkat through the canonical member path.
///
/// Returns a JSON-encoded delivery receipt.
#[wasm_bindgen]
pub async fn mob_member_send(
    mob_id: &str,
    meerkat_id: &str,
    request_json: &str,
) -> Result<String, JsValue> {
    let req: MobMemberSendOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);
    let receipt = mob_state
        .mob_member_send(
            &id,
            mid,
            req.content,
            req.handling_mode,
            req.render_metadata,
        )
        .await
        .map_err(err_mob)?;
    serde_json::to_string(&receipt).map_err(|e| err_str("serialize", e))
}

/// Read the current execution snapshot for a mob member.
#[wasm_bindgen]
pub async fn mob_member_status(mob_id: &str, meerkat_id: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);
    let snapshot = mob_state
        .mob_member_status(&id, &mid)
        .await
        .map_err(err_mob)?;
    let json = serde_json::to_string(&snapshot).map_err(|e| err_str("serialize", e))?;
    Ok(JsValue::from_str(&json))
}

/// Retire and re-spawn a meerkat with the same profile.
///
/// Returns JSON result envelope with receipt.
#[wasm_bindgen]
pub async fn mob_respawn(
    mob_id: &str,
    meerkat_id: &str,
    initial_message: Option<String>,
) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);
    let initial_message = initial_message.map(|message| {
        serde_json::from_str::<meerkat_core::types::ContentInput>(&message)
            .unwrap_or_else(|_| message.into())
    });
    match mob_state.mob_respawn(&id, mid, initial_message).await {
        Ok(receipt) => {
            let result = serde_json::json!({
                "status": "completed",
                "receipt": receipt,
            });
            Ok(JsValue::from_str(&result.to_string()))
        }
        Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
            receipt,
            failed_peer_ids,
        }) => {
            let result = serde_json::json!({
                "status": "topology_restore_failed",
                "receipt": receipt,
                "failed_peer_ids": failed_peer_ids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
            });
            Ok(JsValue::from_str(&result.to_string()))
        }
        Err(e) => Err(JsValue::from_str(&e.to_string())),
    }
}

/// Force-cancel an active mob member turn.
#[wasm_bindgen]
pub async fn mob_force_cancel(mob_id: &str, meerkat_id: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);
    mob_state.mob_force_cancel(&id, mid).await.map_err(err_mob)
}

/// Spawn a short-lived helper and return its terminal result.
#[wasm_bindgen]
pub async fn mob_spawn_helper(mob_id: &str, request_json: &str) -> Result<JsValue, JsValue> {
    let request: MobSpawnHelperOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let meerkat_id = MeerkatId::from(
        request
            .meerkat_id
            .unwrap_or_else(|| format!("helper-{}", Uuid::new_v4())),
    );
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(profile_name) = request.profile_name {
        options.profile_name = Some(meerkat_mob::ProfileName::from(profile_name));
    }
    options.runtime_mode = request.runtime_mode;
    options.backend = request.backend;
    let result = mob_state
        .mob_spawn_helper(&id, meerkat_id, request.prompt, options)
        .await
        .map_err(err_mob)?;
    let json = serde_json::to_string(&serde_json::json!({
        "output": result.output,
        "tokens_used": result.tokens_used,
        "session_id": result.session_id,
    }))
    .map_err(|e| err_str("serialize", e))?;
    Ok(JsValue::from_str(&json))
}

/// Fork a short-lived helper from an existing member and return its terminal result.
#[wasm_bindgen]
pub async fn mob_fork_helper(mob_id: &str, request_json: &str) -> Result<JsValue, JsValue> {
    let request: MobForkHelperOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let source_member_id = MeerkatId::from(request.source_member_id.as_str());
    let meerkat_id = MeerkatId::from(
        request
            .meerkat_id
            .unwrap_or_else(|| format!("fork-{}", Uuid::new_v4())),
    );
    let fork_context = request
        .fork_context
        .unwrap_or(meerkat_mob::ForkContext::FullHistory);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(profile_name) = request.profile_name {
        options.profile_name = Some(meerkat_mob::ProfileName::from(profile_name));
    }
    options.runtime_mode = request.runtime_mode;
    options.backend = request.backend;
    let result = mob_state
        .mob_fork_helper(
            &id,
            &source_member_id,
            meerkat_id,
            request.prompt,
            fork_context,
            options,
        )
        .await
        .map_err(err_mob)?;
    let json = serde_json::to_string(&serde_json::json!({
        "output": result.output,
        "tokens_used": result.tokens_used,
        "session_id": result.session_id,
    }))
    .map_err(|e| err_str("serialize", e))?;
    Ok(JsValue::from_str(&json))
}

/// Start a configured flow run.
///
/// Returns the run_id as a string.
#[wasm_bindgen]
pub async fn mob_run_flow(
    mob_id: &str,
    flow_id: &str,
    params_json: &str,
) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let fid = FlowId::from(flow_id);
    let params: serde_json::Value = if params_json.is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(params_json).map_err(|e| err_str("invalid_params", e))?
    };
    let run_id = mob_state
        .mob_run_flow(&id, fid, params)
        .await
        .map_err(err_mob)?;
    Ok(JsValue::from_str(&run_id.to_string()))
}

/// Read flow run status.
///
/// Returns JSON with run state and ledgers, or null if not found.
#[wasm_bindgen]
pub async fn mob_flow_status(mob_id: &str, run_id: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let rid: RunId = run_id
        .parse()
        .map_err(|e| err_str("invalid_run_id", format!("{e}")))?;
    let status = mob_state.mob_flow_status(&id, rid).await.map_err(err_mob)?;
    match status {
        Some(run) => {
            let json = serde_json::to_string(&run).map_err(|e| err_str("serialize_error", e))?;
            Ok(JsValue::from_str(&json))
        }
        None => Ok(JsValue::from_str("null")),
    }
}

/// Cancel an in-flight flow run.
#[wasm_bindgen]
pub async fn mob_cancel_flow(mob_id: &str, run_id: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let rid: RunId = run_id
        .parse()
        .map_err(|e| err_str("invalid_run_id", format!("{e}")))?;
    mob_state.mob_cancel_flow(&id, rid).await.map_err(err_mob)
}

// ═══════════════════════════════════════════════════════════
// Event Streaming — subscribe to individual meerkat sessions
// ═══════════════════════════════════════════════════════════

/// Subscription inner type: per-member broadcast or mob-wide mpsc.
enum SubscriptionInner {
    /// Per-member session broadcast receiver.
    Member(std::cell::RefCell<meerkat_session::BroadcastEventReceiver>),
    /// Mob-wide attributed event receiver from the event router.
    /// The entire handle is stored to keep the router task alive (Drop cancels it).
    MobWide(std::cell::RefCell<meerkat_mob::MobEventRouterHandle>),
}

/// Per-subscription state wrapping the inner receiver.
struct EventSubscription {
    inner: SubscriptionInner,
}

#[derive(Default)]
struct SubscriptionRegistry {
    next_handle: u32,
    subscriptions: BTreeMap<u32, EventSubscription>,
}

thread_local! {
    static SUBSCRIPTIONS: RefCell<SubscriptionRegistry> = const { RefCell::new(SubscriptionRegistry {
        next_handle: 1,
        subscriptions: BTreeMap::new(),
    }) };
}

/// Subscribe to a mob member's session event stream.
///
/// Returns a subscription handle. Use `poll_subscription(handle)` to drain
/// buffered events. Each call returns all events since the last poll.
///
/// The subscription captures ALL agent activity: text deltas, tool calls
/// (including comms send_message/peers), turn completions, etc.
#[wasm_bindgen]
pub async fn mob_member_subscribe(mob_id: &str, meerkat_id: &str) -> Result<u32, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let mob_id_typed = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);

    let session_id = resolve_mob_member_session_id(&mob_state, &mob_id_typed, &mid).await?;

    // Get a raw broadcast receiver via the concrete EphemeralSessionService.
    // This avoids the async EventStream wrapper — we use synchronous try_recv()
    // in poll_subscription, which works reliably on wasm32.
    let rx = RUNTIME_STATE.with(|cell| {
        let borrow = cell.borrow();
        let state = borrow
            .as_ref()
            .ok_or_else(|| err_js("not_initialized", "runtime not initialized"))?;
        // We stored the concrete service in RuntimeState for exactly this purpose.
        // subscribe_session_events_raw returns the raw broadcast::Receiver.
        Ok::<_, JsValue>(state.session_service.clone())
    })?;
    let raw_rx = rx
        .subscribe_session_events_raw(&session_id)
        .await
        .map_err(|e| err_str("subscribe_error", e))?;

    let handle = SUBSCRIPTIONS.with(|cell| {
        let mut registry = cell.borrow_mut();
        let h = registry.next_handle;
        registry.next_handle = registry.next_handle.wrapping_add(1);
        while registry.next_handle == 0
            || registry.subscriptions.contains_key(&registry.next_handle)
        {
            registry.next_handle = registry.next_handle.wrapping_add(1);
        }
        registry.subscriptions.insert(
            h,
            EventSubscription {
                inner: SubscriptionInner::Member(std::cell::RefCell::new(raw_rx)),
            },
        );
        h
    });

    Ok(handle)
}

/// Subscribe to mob-wide events (all members, continuously updated).
///
/// Returns a subscription handle. Use `poll_subscription(handle)` to drain
/// buffered events. Each call returns all events since the last poll.
///
/// Unlike `mob_member_subscribe` which streams a single member's agent events,
/// this streams [`AttributedEvent`]s tagged with source meerkat_id and profile
/// for every member in the mob, automatically tracking roster changes.
#[wasm_bindgen]
pub async fn mob_subscribe_events(mob_id: &str) -> Result<u32, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let mob_id_typed = MobId::from(mob_id);

    let router_handle = mob_state
        .subscribe_mob_events(&mob_id_typed)
        .await
        .map_err(err_mob)?;

    let handle = SUBSCRIPTIONS.with(|cell| {
        let mut registry = cell.borrow_mut();
        let h = registry.next_handle;
        registry.next_handle = registry.next_handle.wrapping_add(1);
        while registry.next_handle == 0
            || registry.subscriptions.contains_key(&registry.next_handle)
        {
            registry.next_handle = registry.next_handle.wrapping_add(1);
        }
        registry.subscriptions.insert(
            h,
            EventSubscription {
                inner: SubscriptionInner::MobWide(std::cell::RefCell::new(router_handle)),
            },
        );
        h
    });

    Ok(handle)
}

/// Poll a subscription for new events.
///
/// Returns a JSON array of event objects. Drains all buffered events
/// since the last poll. Non-blocking: returns `[]` if no new events.
///
/// For per-member subscriptions (`mob_member_subscribe`), returns
/// `EventEnvelope<AgentEvent>` objects. For mob-wide subscriptions
/// (`mob_subscribe_events`), returns `AttributedEvent` objects with
/// `source`, `profile`, and `envelope` fields.
#[wasm_bindgen]
pub fn poll_subscription(handle: u32) -> Result<String, JsValue> {
    SUBSCRIPTIONS.with(|cell| {
        let registry = cell.borrow();
        let sub = registry.subscriptions.get(&handle).ok_or_else(|| {
            err_js(
                "invalid_handle",
                &format!("unknown subscription handle: {handle}"),
            )
        })?;

        let mut events: Vec<serde_json::Value> = Vec::new();
        match &sub.inner {
            SubscriptionInner::Member(rx_cell) => {
                use crate::tokio::sync::broadcast::error::TryRecvError;
                let mut rx = rx_cell.borrow_mut();
                loop {
                    match rx.try_recv() {
                        Ok(event) => match serde_json::to_value(&event) {
                            Ok(val) => events.push(val),
                            Err(e) => tracing::warn!(error = %e, "failed to serialize agent event"),
                        },
                        Err(TryRecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "subscription lagged");
                            events.push(serde_json::json!({
                                "type": "lagged",
                                "skipped": n,
                            }));
                            continue;
                        }
                        Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                    }
                }
            }
            SubscriptionInner::MobWide(handle_cell) => {
                let mut router_handle = handle_cell.borrow_mut();
                while let Ok(attributed) = router_handle.event_rx.try_recv() {
                    match serde_json::to_value(&attributed) {
                        Ok(val) => events.push(val),
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to serialize attributed event");
                        }
                    }
                }
            }
        }
        serde_json::to_string(&events).map_err(|e| err_str("serialize_error", e))
    })
}

/// Close a subscription and free resources.
#[wasm_bindgen]
pub fn close_subscription(handle: u32) -> Result<(), JsValue> {
    SUBSCRIPTIONS.with(|cell| {
        let mut registry = cell.borrow_mut();
        registry.subscriptions.remove(&handle).ok_or_else(|| {
            err_js(
                "invalid_handle",
                &format!("unknown subscription handle: {handle}"),
            )
        })?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    use super::build_service_infrastructure;
    use super::{
        EventSubscription, SUBSCRIPTIONS, SubscriptionInner, close_subscription,
        merge_runtime_system_context_state, poll_subscription,
    };
    #[cfg(target_arch = "wasm32")]
    use super::{
        append_system_context, create_session_simple, destroy_session, get_session_state,
        init_runtime_from_config,
    };
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat::SessionServiceControlExt;
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat_core::Config;
    use meerkat_core::time_compat::{Duration, SystemTime};
    use meerkat_core::{
        PendingSystemContextAppend, SeenSystemContextKey, SeenSystemContextState,
        SessionSystemContextState,
    };
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat_mob::SpawnMemberSpec;
    use serde_json::json;
    #[cfg(not(target_arch = "wasm32"))]
    use std::collections::HashMap;

    #[cfg(target_arch = "wasm32")]
    fn init_test_runtime() {
        let init = init_runtime_from_config(
            &json!({
                "anthropic_api_key": "sk-test",
                "model": "claude-sonnet-4-5"
            })
            .to_string(),
        );
        assert!(init.is_ok());
    }

    #[test]
    fn merge_runtime_system_context_state_preserves_concurrent_appends() {
        let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

        let initial_pending = PendingSystemContextAppend {
            text: "initial".to_string(),
            source: Some("mob".to_string()),
            idempotency_key: Some("ctx-initial".to_string()),
            accepted_at: base_time,
        };
        let concurrent_pending = PendingSystemContextAppend {
            text: "concurrent".to_string(),
            source: Some("mob".to_string()),
            idempotency_key: Some("ctx-concurrent".to_string()),
            accepted_at: base_time + Duration::from_secs(1),
        };

        let starting_state = SessionSystemContextState {
            pending: vec![initial_pending.clone()],
            seen: std::collections::BTreeMap::from([(
                "ctx-initial".to_string(),
                SeenSystemContextKey {
                    text: initial_pending.text.clone(),
                    source: initial_pending.source.clone(),
                    state: SeenSystemContextState::Pending,
                },
            )]),
        };
        let agent_state = SessionSystemContextState {
            pending: Vec::new(),
            seen: std::collections::BTreeMap::from([(
                "ctx-initial".to_string(),
                SeenSystemContextKey {
                    text: initial_pending.text.clone(),
                    source: initial_pending.source.clone(),
                    state: SeenSystemContextState::Applied,
                },
            )]),
        };
        let current_registry_state = SessionSystemContextState {
            pending: vec![initial_pending, concurrent_pending.clone()],
            seen: std::collections::BTreeMap::from([
                (
                    "ctx-initial".to_string(),
                    SeenSystemContextKey {
                        text: "initial".to_string(),
                        source: Some("mob".to_string()),
                        state: SeenSystemContextState::Pending,
                    },
                ),
                (
                    "ctx-concurrent".to_string(),
                    SeenSystemContextKey {
                        text: concurrent_pending.text.clone(),
                        source: concurrent_pending.source.clone(),
                        state: SeenSystemContextState::Pending,
                    },
                ),
            ]),
        };

        let merged = merge_runtime_system_context_state(
            agent_state,
            &starting_state,
            &current_registry_state,
        );

        assert_eq!(merged.pending, vec![concurrent_pending]);
        assert_eq!(
            merged.seen.get("ctx-initial").map(|seen| seen.state),
            Some(SeenSystemContextState::Applied)
        );
        assert_eq!(
            merged.seen.get("ctx-concurrent").map(|seen| seen.state),
            Some(SeenSystemContextState::Pending)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::expect_used)]
    #[tokio::test(flavor = "current_thread")]
    async fn mob_append_system_context_targets_member_session() {
        let mut config = Config::default();
        config.providers.api_keys = Some(HashMap::from([(
            "anthropic".to_string(),
            "sk-test".to_string(),
        )]));
        let (service, mob_state) =
            build_service_infrastructure(config, 8).expect("build runtime services");

        let definition: meerkat_mob::MobDefinition = serde_json::from_value(json!({
            "id": "mob-system-context",
            "orchestrator": {
                "profile": "lead"
            },
            "profiles": {
                "lead": {
                    "model": "claude-opus-4-6",
                    "tools": {
                        "comms": true,
                        "mob": true
                    }
                },
                "worker": {
                    "model": "claude-sonnet-4-5",
                    "tools": {
                        "comms": true
                    }
                }
            }
        }))
        .expect("mob definition");
        let mob_id = mob_state
            .mob_create_definition(definition)
            .await
            .expect("create mob");

        let spawn_results = mob_state
            .mob_spawn_many(
                &mob_id,
                vec![
                    SpawnMemberSpec::new("worker", "worker-1")
                        .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
                ],
            )
            .await
            .expect("spawn worker");
        let session_id = spawn_results[0]
            .as_ref()
            .expect("spawn result")
            .session_id()
            .cloned()
            .expect("member session id");

        let append = service
            .append_system_context(
                &session_id,
                meerkat_core::AppendSystemContextRequest {
                    text: "Prioritize coordinating with the lead.".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-worker-1".to_string()),
                },
            )
            .await
            .expect("append system context");

        assert_eq!(
            append.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let exported = service
            .export_session(&session_id)
            .await
            .expect("export member session");
        let system_context_state = exported
            .system_context_state()
            .expect("system-context state metadata");

        assert_eq!(system_context_state.pending.len(), 1);
        assert_eq!(
            system_context_state.pending[0].idempotency_key.as_deref(),
            Some("ctx-worker-1")
        );
        assert_eq!(
            system_context_state.pending[0].text,
            "Prioritize coordinating with the lead."
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn append_system_context_export_stages_context_for_direct_session_handle() {
        init_test_runtime();
        let handle = create_session_simple(
            &json!({
                "model": "claude-sonnet-4-5",
                "api_key": "sk-test"
            })
            .to_string(),
        )
        .expect("create session");

        let result = append_system_context(
            handle,
            &json!({
                "text": "Remember the browser-side coordinator.",
                "source": "web",
                "idempotency_key": "ctx-web-1"
            })
            .to_string(),
        )
        .expect("append system context");
        let result_json = result.as_string().expect("append result string");
        let parsed: serde_json::Value =
            serde_json::from_str(&result_json).expect("append result json");

        assert_eq!(parsed["handle"], handle);
        assert_eq!(parsed["status"], "staged");
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn destroy_session_export_removes_local_handle() {
        init_test_runtime();
        let handle = create_session_simple(
            &json!({
                "model": "claude-sonnet-4-5",
                "api_key": "sk-test"
            })
            .to_string(),
        )
        .expect("create session");

        let before = get_session_state(handle).expect("session state before destroy");
        let before: serde_json::Value = serde_json::from_str(&before).expect("state json");
        assert_eq!(before["handle"], handle);
        assert!(before["session_id"].as_str().is_some());

        destroy_session(handle).expect("destroy session");
        assert!(get_session_state(handle).is_err());
    }

    #[test]
    fn poll_subscription_surfaces_lagged_signal() {
        let (tx, rx) = crate::tokio::sync::broadcast::channel(1);
        let session_id = meerkat_core::SessionId::new();
        tx.send(meerkat_core::EventEnvelope::new(
            session_id.to_string(),
            1,
            None,
            meerkat_core::AgentEvent::TextDelta {
                delta: "first".to_string(),
            },
        ))
        .unwrap_or(0);
        tx.send(meerkat_core::EventEnvelope::new(
            session_id.to_string(),
            2,
            None,
            meerkat_core::AgentEvent::TextDelta {
                delta: "second".to_string(),
            },
        ))
        .unwrap_or(0);

        let handle = SUBSCRIPTIONS.with(|cell| {
            let mut registry = cell.borrow_mut();
            let handle = registry.next_handle;
            registry.next_handle = registry.next_handle.wrapping_add(1);
            registry.subscriptions.insert(
                handle,
                EventSubscription {
                    inner: SubscriptionInner::Member(std::cell::RefCell::new(rx)),
                },
            );
            handle
        });

        let payload_result = poll_subscription(handle);
        assert!(payload_result.is_ok());
        let payload = match payload_result {
            Ok(payload) => payload,
            Err(_) => unreachable!(),
        };
        let parsed_result: Result<serde_json::Value, _> = serde_json::from_str(&payload);
        assert!(parsed_result.is_ok());
        let parsed = match parsed_result {
            Ok(parsed) => parsed,
            Err(_) => unreachable!(),
        };
        let items_opt = parsed.as_array();
        assert!(items_opt.is_some());
        let items = match items_opt {
            Some(items) => items,
            None => unreachable!(),
        };
        assert_eq!(items[0]["type"], "lagged");
        assert_eq!(items[0]["skipped"], 1);
        assert_eq!(items[1]["payload"]["type"], "text_delta");
        assert_eq!(items[1]["payload"]["delta"], "second");

        let close_result = close_subscription(handle);
        assert!(close_result.is_ok());
    }
}
