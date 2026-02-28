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
//!
//! ### Session (direct agent loop, existing per-session approach)
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
//! - `mob_lifecycle(mob_id, action)` — stop/resume/complete/destroy
//! - `mob_events(mob_id, after_cursor, limit)` → JSON
//! - `mob_spawn(mob_id, specs_json)` → JSON
//! - `mob_retire(mob_id, meerkat_id)`
//! - `mob_wire(mob_id, a, b)`
//! - `mob_unwire(mob_id, a, b)`
//! - `mob_list_members(mob_id)` → JSON
//! - `mob_send_message(mob_id, meerkat_id, message)`
//! - `mob_respawn(mob_id, meerkat_id, initial_message?)` — retire + re-spawn same profile
//! - `mob_inject_and_subscribe(mob_id, meerkat_id, message)` → interaction_id
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
use wasm_bindgen::prelude::*;

use meerkat::AgentBuildConfig;
use meerkat_core::Config;
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
        tracing_wasm::set_as_global_default();
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn init_tracing() {}

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
    #[serde(default)]
    base_url: Option<String>,
    /// Enable comms for this session (registers in InprocRegistry).
    #[serde(default)]
    comms_name: Option<String>,
    /// Whether this session runs in host mode (enables comms tools).
    #[serde(default)]
    host_mode: bool,
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
    /// Optional custom base URL — mapped to the default model's provider.
    #[serde(default)]
    base_url: Option<String>,
}

fn default_model() -> Option<String> {
    Some("claude-sonnet-4-5".to_string())
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
    /// Optional custom base URL — mapped to the default model's provider.
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default = "default_max_sessions")]
    max_sessions: usize,
}

fn default_max_sessions() -> usize {
    MAX_SESSIONS
}

// ═══════════════════════════════════════════════════════════
// Event Model
// ═══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

// ═══════════════════════════════════════════════════════════
// Runtime Session (holds factory-built agent state)
// ═══════════════════════════════════════════════════════════

struct RuntimeSession {
    handle: u32,
    mob_id: String,
    model: String,
    /// The meerkat Config snapshot for this session's factory.
    config: Config,
    /// Pre-built AgentBuildConfig (with overrides set).
    build_config_template: AgentBuildConfig,
    /// Preserved session for multi-turn context.
    meerkat_session: Option<meerkat_core::session::Session>,
    run_counter: u64,
    usage: Usage,
}

#[derive(Default)]
struct RuntimeRegistry {
    next_handle: u32,
    sessions: BTreeMap<u32, RuntimeSession>,
}

thread_local! {
    static REGISTRY: RefCell<RuntimeRegistry> = const {
        RefCell::new(RuntimeRegistry {
            next_handle: 1,
            sessions: BTreeMap::new(),
        })
    };
}

// ═══════════════════════════════════════════════════════════
// RuntimeState — service-based infrastructure
// ═══════════════════════════════════════════════════════════

type WasmSessionService = meerkat::EphemeralSessionService<meerkat::FactoryAgentBuilder>;

struct RuntimeState {
    mob_state: Arc<MobMcpState>,
    /// Concrete session service — needed for subscribe_session_events_raw.
    session_service: Arc<WasmSessionService>,
    #[allow(dead_code)]
    model: String,
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
    } else if model.starts_with("gpt")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("chatgpt")
    {
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
// WASM Tool Dispatcher
// ═══════════════════════════════════════════════════════════

#[cfg(target_arch = "wasm32")]
fn build_wasm_tool_dispatcher() -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, String> {
    let task_store: Arc<dyn meerkat_tools::builtin::TaskStore> =
        Arc::new(meerkat_tools::builtin::MemoryTaskStore::new());
    let config = meerkat_tools::builtin::BuiltinToolConfig::default();
    let composite =
        meerkat_tools::builtin::CompositeDispatcher::new_wasm(task_store, &config, None, None)
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
    let _parsed = parse_mobpack(mobpack_bytes).map_err(|e| err_str("invalid_mobpack", e))?;
    let creds: Credentials =
        serde_json::from_str(credentials_json).map_err(|e| err_str("invalid_credentials", e))?;

    let api_keys = build_provider_api_keys(
        creds.api_key.as_deref(),
        creds.anthropic_api_key.as_deref(),
        creds.openai_api_key.as_deref(),
        creds.gemini_api_key.as_deref(),
    )?;

    let model = creds
        .model
        .unwrap_or_else(|| "claude-sonnet-4-5".to_string());

    let providers: Vec<String> = api_keys.keys().cloned().collect();
    let mut config = Config::default();
    config.agent.model.clone_from(&model);
    config.providers.api_keys = Some(api_keys);
    // Map single base_url to the default model's inferred provider.
    if let Some(url) = &creds.base_url
        && !url.trim().is_empty()
    {
        if let Some(provider) = infer_provider_name(&model) {
            config.providers.base_urls = Some(HashMap::from([(provider.to_string(), url.clone())]));
        } else {
            tracing::warn!(
                model = &*model,
                "base_url ignored: cannot infer provider from model"
            );
        }
    }

    let (session_service, mob_state) = build_service_infrastructure(config, MAX_SESSIONS)?;

    RUNTIME_STATE.with(|cell| {
        *cell.borrow_mut() = Some(RuntimeState {
            mob_state,
            session_service,
            model: model.clone(),
        });
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
    let rt_config: RuntimeConfig =
        serde_json::from_str(config_json).map_err(|e| err_str("invalid_config", e))?;

    let api_keys = build_provider_api_keys(
        rt_config.api_key.as_deref(),
        rt_config.anthropic_api_key.as_deref(),
        rt_config.openai_api_key.as_deref(),
        rt_config.gemini_api_key.as_deref(),
    )?;

    let model = rt_config
        .model
        .unwrap_or_else(|| "claude-sonnet-4-5".to_string());
    let max_sessions = rt_config.max_sessions;

    let providers: Vec<String> = api_keys.keys().cloned().collect();
    let mut config = Config::default();
    config.agent.model.clone_from(&model);
    config.providers.api_keys = Some(api_keys);
    // Map single base_url to the default model's inferred provider.
    if let Some(url) = &rt_config.base_url
        && !url.trim().is_empty()
    {
        if let Some(provider) = infer_provider_name(&model) {
            config.providers.base_urls = Some(HashMap::from([(provider.to_string(), url.clone())]));
        } else {
            tracing::warn!(
                model = &*model,
                "base_url ignored: cannot infer provider from model"
            );
        }
    }

    let (session_service, mob_state) = build_service_infrastructure(config, max_sessions)?;

    RUNTIME_STATE.with(|cell| {
        *cell.borrow_mut() = Some(RuntimeState {
            mob_state,
            session_service,
            model: model.clone(),
        });
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
// Exported WASM API — Session (existing per-session approach)
// ═══════════════════════════════════════════════════════════

/// Create a session from a mobpack + config.
///
/// Routes through `AgentFactory::build_agent()` with override-first
/// resource injection — same pipeline as all other meerkat surfaces.
///
/// `config_json`: `{ "model": "...", "api_key": "sk-...", "max_tokens"?: N,
///                    "comms_name"?: "...", "host_mode"?: true }`
#[wasm_bindgen]
pub fn create_session(mobpack_bytes: &[u8], config_json: &str) -> Result<u32, JsValue> {
    let parsed = parse_mobpack(mobpack_bytes).map_err(|e| err_str("invalid_mobpack", e))?;
    let config: SessionConfig =
        serde_json::from_str(config_json).map_err(|e| err_str("invalid_config", e))?;

    if config.model.trim().is_empty() {
        return Err(err_js("invalid_config", "model must not be empty"));
    }
    let api_key = config.api_key.as_deref().unwrap_or("");
    if api_key.is_empty() {
        return Err(err_js("invalid_config", "api_key must not be empty"));
    }

    // Compile system prompt from mobpack skills.
    let system_prompt = compile_system_prompt(
        &parsed.definition.id,
        &parsed.manifest.mobpack.name,
        &parsed.skills,
        config.system_prompt.as_deref(),
    );

    // Create LLM client.
    let llm_client = create_llm_client(&config.model, api_key, config.base_url.as_deref())
        .map_err(|e| err_str("provider_error", e))?;

    // Build tool dispatcher (in-memory tasks, no shell).
    let tools = build_wasm_tool_dispatcher().map_err(|e| err_str("tool_error", e))?;

    // Build session store (in-memory).
    let store: Arc<dyn meerkat_core::AgentSessionStore> = Arc::new(
        meerkat_store::StoreAdapter::new(Arc::new(meerkat_store::MemoryStore::new())),
    );

    // Prepare AgentBuildConfig with all overrides set.
    let mut build_config = AgentBuildConfig::new(&config.model);
    build_config.system_prompt = Some(system_prompt);
    build_config.max_tokens = Some(config.max_tokens);
    build_config.tool_dispatcher_override = Some(tools);
    build_config.session_store_override = Some(store);
    build_config.llm_client_override = Some(llm_client);
    if let Some(name) = config.comms_name.clone() {
        build_config.comms_name = Some(name);
        build_config.host_mode = config.host_mode;
    }
    build_config.additional_instructions = config.additional_instructions.clone();
    build_config.app_context = config.app_context.clone();

    // Create a minimal Config for the factory.
    let meerkat_config = Config::default();

    let handle = REGISTRY.with(|cell| {
        let mut registry = cell.borrow_mut();
        let handle = registry.next_handle;
        registry.next_handle = registry.next_handle.wrapping_add(1);
        // Skip 0 (reserved) and any colliding handle.
        while registry.next_handle == 0 || registry.sessions.contains_key(&registry.next_handle) {
            registry.next_handle = registry.next_handle.wrapping_add(1);
        }

        let session = RuntimeSession {
            handle,
            mob_id: parsed.definition.id,
            model: config.model.clone(),
            config: meerkat_config,
            build_config_template: build_config,
            meerkat_session: None,
            run_counter: 0,
            usage: Usage::default(),
        };

        registry.sessions.insert(handle, session);
        handle
    });

    Ok(handle)
}

/// Per-turn options parsed from `options_json`.
#[derive(Debug, Default, Deserialize)]
struct TurnOptions {
    /// Additional instruction sections appended for this turn only.
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
}

/// Run a turn through the real meerkat agent loop via AgentFactory::build_agent().
///
/// Returns JSON: `{ "text", "usage", "status", "session_id", "turns", "tool_calls" }`
///
/// Convention: always resolves (Ok). Check `status` field for "completed" vs "failed".
/// Only rejects (Err) for infrastructure errors (session not found, build failure).
/// Agent-level errors (LLM failure, timeout) resolve with `status: "failed"` + `error` field.
#[wasm_bindgen]
pub async fn start_turn(handle: u32, prompt: &str, options_json: &str) -> Result<JsValue, JsValue> {
    let options: TurnOptions = if options_json.is_empty() || options_json == "{}" {
        TurnOptions::default()
    } else {
        serde_json::from_str(options_json).map_err(|e| err_str("invalid_options", e))?
    };
    // Extract what we need from the session (release borrow quickly).
    let (mut build_config, config, run_id) = REGISTRY
        .with(|cell| {
            let mut registry = cell.borrow_mut();
            let session = registry
                .sessions
                .get_mut(&handle)
                .ok_or_else(|| format!("unknown session handle: {handle}"))?;
            session.run_counter = session.run_counter.saturating_add(1);
            let run_id = session.run_counter;

            // Clone the template and take the prior session.
            let mut bc = AgentBuildConfig::new(&session.model);
            bc.system_prompt = session.build_config_template.system_prompt.clone();
            bc.max_tokens = session.build_config_template.max_tokens;
            bc.llm_client_override = session.build_config_template.llm_client_override.clone();
            bc.comms_name = session.build_config_template.comms_name.clone();
            bc.host_mode = session.build_config_template.host_mode;
            bc.additional_instructions = session
                .build_config_template
                .additional_instructions
                .clone();
            bc.app_context = session.build_config_template.app_context.clone();

            // Rebuild tools + store fresh each turn (they're cheap, avoids ownership issues).
            bc.tool_dispatcher_override = session
                .build_config_template
                .tool_dispatcher_override
                .clone();
            bc.session_store_override =
                session.build_config_template.session_store_override.clone();

            // Resume from prior session if available.
            bc.resume_session = session.meerkat_session.take();

            Ok((bc, session.config.clone(), run_id))
        })
        .map_err(|e: String| err_str("session_not_found", e))?;

    // Apply per-turn options (override session-level values when present).
    if options.additional_instructions.is_some() {
        build_config.additional_instructions = options.additional_instructions;
    }

    // Build the agent through AgentFactory::build_agent() — same pipeline as all surfaces.
    let factory = meerkat::AgentFactory::minimal();
    let mut agent = factory
        .build_agent(build_config, &config)
        .await
        .map_err(|e| err_str("build_agent_error", e))?;

    // Run the turn.
    let run_result = agent.run(prompt.into()).await;

    // Preserve the agent's session for future turns.
    let agent_session = agent.session().clone();

    match run_result {
        Ok(result) => {
            REGISTRY.with(|cell| {
                let mut registry = cell.borrow_mut();
                if let Some(session) = registry.sessions.get_mut(&handle) {
                    session.meerkat_session = Some(agent_session);
                    session.usage.input_tokens = session
                        .usage
                        .input_tokens
                        .saturating_add(result.usage.input_tokens);
                    session.usage.output_tokens = session
                        .usage
                        .output_tokens
                        .saturating_add(result.usage.output_tokens);
                }
            });

            let result_json = serde_json::json!({
                "run_id": run_id,
                "text": result.text,
                "usage": {
                    "input_tokens": result.usage.input_tokens,
                    "output_tokens": result.usage.output_tokens,
                },
                "session_id": handle,
                "status": "completed",
                "turns": result.turns,
                "tool_calls": result.tool_calls,
            });
            Ok(JsValue::from_str(&result_json.to_string()))
        }
        Err(err) => {
            let error_msg = format!("{err}");
            REGISTRY.with(|cell| {
                let mut registry = cell.borrow_mut();
                if let Some(session) = registry.sessions.get_mut(&handle) {
                    session.meerkat_session = Some(agent_session);
                }
            });

            let result_json = serde_json::json!({
                "run_id": run_id,
                "text": "",
                "usage": { "input_tokens": 0, "output_tokens": 0 },
                "session_id": handle,
                "status": "failed",
                "error": error_msg,
            });
            Ok(JsValue::from_str(&result_json.to_string()))
        }
    }
}

/// Get current session state.
#[wasm_bindgen]
pub fn get_session_state(handle: u32) -> Result<String, JsValue> {
    REGISTRY
        .with(|cell| {
            let registry = cell.borrow();
            let session = registry
                .sessions
                .get(&handle)
                .ok_or_else(|| format!("unknown session handle: {handle}"))?;
            let state = serde_json::json!({
                "session_id": session.handle,
                "mob_id": session.mob_id,
                "model": session.model,
                "usage": session.usage,
                "run_counter": session.run_counter,
            });
            Ok(state.to_string())
        })
        .map_err(|e: String| err_str("session_not_found", e))
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
    REGISTRY.with(|cell| {
        let mut registry = cell.borrow_mut();
        registry
            .sessions
            .remove(&handle)
            .ok_or_else(|| err_js("session_not_found", &format!("unknown handle: {handle}")))?;
        Ok(())
    })
}

/// Drain and return all pending events (placeholder for future event streaming).
#[wasm_bindgen]
pub fn poll_events(_handle: u32) -> Result<String, JsValue> {
    // Events will be populated once we wire AgentEvent streaming through
    // the build_config event channel. For now, return empty array.
    Ok("[]".to_string())
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
/// `action`: one of "stop", "resume", "complete", "destroy".
#[wasm_bindgen]
pub async fn mob_lifecycle(mob_id: &str, action: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    match action {
        "stop" => mob_state.mob_stop(&id).await.map_err(err_mob)?,
        "resume" => mob_state.mob_resume(&id).await.map_err(err_mob)?,
        "complete" => mob_state.mob_complete(&id).await.map_err(err_mob)?,
        "destroy" => mob_state.mob_destroy(&id).await.map_err(err_mob)?,
        _ => {
            return Err(err_js(
                "invalid_action",
                &format!(
                    "unknown lifecycle action: {action} (expected stop/resume/complete/destroy)"
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
///                "runtime_mode"?: "autonomous_host"|"turn_driven", "backend"?: "subagent"|"external" }`
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
        let resume_session_id = match s.resume_session_id {
            Some(id) => Some(meerkat_core::SessionId::parse(&id).map_err(|_| {
                err_js(
                    "invalid_resume_session_id",
                    &format!("invalid session ID: {id}"),
                )
            })?),
            None => None,
        };
        spawn_specs.push(meerkat_mob::SpawnMemberSpec {
            profile_name: meerkat_mob::ProfileName::from(s.profile.as_str()),
            meerkat_id: MeerkatId::from(s.meerkat_id.as_str()),
            initial_message: s.initial_message,
            runtime_mode: s.runtime_mode,
            backend: s.backend,
            context: s.context,
            labels: s.labels,
            resume_session_id,
        });
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
    initial_message: Option<String>,
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
        .mob_wire(&id, MeerkatId::from(a), MeerkatId::from(b))
        .await
        .map_err(err_mob)
}

/// Unwire bidirectional trust between two meerkats.
#[wasm_bindgen]
pub async fn mob_unwire(mob_id: &str, a: &str, b: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    mob_state
        .mob_unwire(&id, MeerkatId::from(a), MeerkatId::from(b))
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

/// Send an external message to a spawned meerkat.
#[wasm_bindgen]
pub async fn mob_send_message(
    mob_id: &str,
    meerkat_id: &str,
    message: &str,
) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);
    mob_state
        .mob_send_message(&id, mid, message.to_string())
        .await
        .map_err(err_mob)
}

/// Retire and re-spawn a meerkat with the same profile.
///
/// Returns JSON: `{ "meerkat_id", "status": "respawn_enqueued" }`
#[wasm_bindgen]
pub async fn mob_respawn(
    mob_id: &str,
    meerkat_id: &str,
    initial_message: Option<String>,
) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);
    mob_state
        .mob_respawn(&id, mid, initial_message)
        .await
        .map_err(err_mob)?;
    let result = serde_json::json!({
        "meerkat_id": meerkat_id,
        "status": "respawn_enqueued",
    });
    Ok(JsValue::from_str(&result.to_string()))
}

/// Inject a message and subscribe for interaction-scoped events.
///
/// Returns JSON: `{ "interaction_id": "..." }`
#[wasm_bindgen]
pub async fn mob_inject_and_subscribe(
    mob_id: &str,
    meerkat_id: &str,
    message: &str,
) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = MeerkatId::from(meerkat_id);
    let subscription = mob_state
        .mob_inject_and_subscribe(&id, mid, message.to_string())
        .await
        .map_err(err_mob)?;
    let result = serde_json::json!({ "interaction_id": subscription.id.to_string() });
    Ok(JsValue::from_str(&result.to_string()))
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

    // Get the member's session_id from the mob roster.
    let members = mob_state
        .mob_list_members(&mob_id_typed)
        .await
        .map_err(err_mob)?;
    let entry = members
        .iter()
        .find(|m| m.meerkat_id == mid)
        .ok_or_else(|| {
            err_js(
                "member_not_found",
                &format!("meerkat '{meerkat_id}' not in mob '{mob_id}'"),
            )
        })?;

    let session_id = entry
        .member_ref
        .session_id()
        .ok_or_else(|| {
            err_js(
                "no_session",
                &format!("meerkat '{meerkat_id}' has no session"),
            )
        })?
        .clone();

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
