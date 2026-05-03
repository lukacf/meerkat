//! Meerkat WASM runtime — a real meerkat surface in the browser.
#![allow(clippy::expect_used)]
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
//! - `start_turn(handle, prompt)` → JSON result
//! - `get_session_state(handle)` → JSON
//! - `destroy_session(handle)`
//! - `inspect_mobpack(mobpack_bytes)` → JSON
//! - `poll_events(handle)` → JSON
//!
//! ### Mob lifecycle (delegates to `MobMcpState`)
//! - `mob_create(definition_json)` → mob_id
//! - `mob_status(mob_id)` → JSON
//! - `mob_list()` → JSON
//! - `mob_lifecycle(mob_id, action)` — typed lifecycle action string carrier
//! - `mob_events(mob_id, after_cursor, limit)` → JSON
//! - `mob_spawn(mob_id, specs_json)` → JSON
//! - `mob_retire(mob_id, agent_identity)`
//! - `mob_wire_peer(mob_id, member, peer_json)` / `mob_unwire_peer(mob_id, member, peer_json)` — canonical member/peer wiring
//! - `mob_wire(mob_id, a, b)` / `mob_unwire(mob_id, a, b)` — low-level local-local wiring
//! - `mob_wire_target(mob_id, local, target_json)` / `mob_unwire_target(mob_id, local, target_json)` — low-level aliases
//! - `mob_list_members(mob_id)` → JSON
//! - `mob_append_system_context(mob_id, agent_identity, request_json)` → JSON
//! - `mob_member_send(mob_id, agent_identity, request_json)` → JSON delivery receipt
//! - `mob_member_status(mob_id, agent_identity)` → JSON member snapshot
//! - `mob_respawn(mob_id, agent_identity, initial_message?)` → JSON result envelope
//! - `mob_force_cancel(mob_id, agent_identity)`
//! - `mob_spawn_helper(mob_id, request_json)` → JSON helper result
//! - `mob_fork_helper(mob_id, request_json)` → JSON helper result
//! - `mob_run_flow(mob_id, flow_id, params_json)` → run_id
//! - `mob_flow_status(mob_id, run_id)` → JSON
//! - `mob_cancel_flow(mob_id, run_id)`
//!
//! ### Event Streaming
//! - `mob_member_subscribe(mob_id, agent_identity)` → handle (per-member)
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

// Phase 4d.wasm.1 — External-auth resolver seam for browser-hosted
// OAuth. Host pages register a JS callback that returns a bearer
// token; the provider-runtime registry consults the resolver id when a
// binding uses `CredentialSourceSpec::ExternalResolver`.
pub mod external_auth;

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::sync::Arc;
use uuid::Uuid;
use wasm_bindgen::prelude::*;

use meerkat::{AgentBuildConfig, SessionServiceControlExt};
use meerkat_core::{Config, SessionService};
use meerkat_mob::{AgentIdentity, FlowId, MobDefinition, MobId, RunId};
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
const SKILL_SEPARATOR: &str = "\n\n---\n\n";
const MAX_SESSIONS: usize = 64;

// ═══════════════════════════════════════════════════════════
// Mobpack Types
// ═══════════════════════════════════════════════════════════

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
    /// Optional structural connection reference. When set, overrides the
    /// default provider-match from bootstrap-populated `config.realm`.
    #[serde(default)]
    connection_ref: Option<meerkat_contracts::WireConnectionRef>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
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
    /// Per-provider API keys. At least one must be set.
    #[serde(default)]
    anthropic_api_key: Option<String>,
    #[serde(default)]
    openai_api_key: Option<String>,
    #[serde(default)]
    gemini_api_key: Option<String>,
    #[serde(default = "default_model")]
    model: Option<String>,
    /// Per-provider base URLs. Unset providers default to the upstream vendor URL.
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
    /// Per-provider API keys. At least one must be set.
    #[serde(default)]
    anthropic_api_key: Option<String>,
    #[serde(default)]
    openai_api_key: Option<String>,
    #[serde(default)]
    gemini_api_key: Option<String>,
    #[serde(default = "default_model")]
    model: Option<String>,
    /// Per-provider base URLs. Unset providers default to the upstream vendor URL.
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
    #[cfg(target_arch = "wasm32")]
    js_tools: Vec<JsToolEntry>,
}

/// Populate `config.realm["default"]` with InlineSecret-backed bindings
/// for each provider in `api_keys`. Plan §6.10 replacement for the
/// deleted shared settings write path (plan §6.10).
/// The first provider becomes the default binding; per-provider base
/// URLs, when provided via `base_urls`, are applied to the matching
/// backend profile.
fn populate_realm_from_api_keys(
    config: &mut meerkat_core::Config,
    api_keys: &HashMap<String, String>,
    base_urls: Option<&HashMap<String, String>>,
) {
    let mut entries: Vec<(&str, &str)> = api_keys
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    // Deterministic order: anthropic first (historical default), then
    // openai, gemini, everything else alphabetically.
    entries.sort_by_key(|(k, _)| match *k {
        "anthropic" => 0,
        "openai" => 1,
        "gemini" | "google" => 2,
        _ => 3,
    });

    let mut section = meerkat_core::RealmConfigSection::from_inline_api_keys(&entries);
    if let Some(urls) = base_urls {
        for (provider, url) in urls {
            let id = format!("default_{provider}");
            if let Some(bp) = section.backend.get_mut(&id) {
                bp.base_url = Some(url.clone());
            }
        }
    }
    config.realm.insert("default".to_string(), section);
}

/// Resolve per-provider API keys into a map consumed by
/// [`populate_realm_from_api_keys`].
///
/// Empty/whitespace-only keys are treated as missing so callers get a fast failure
/// instead of a deferred auth error. Plan §6 dogma §5: typed per-provider fields
/// only — no bare `api_key` folklore that means "anthropic by convention".
fn build_provider_api_keys(
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
    if let Some(k) = non_blank(anthropic_api_key) {
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
            "at least one API key must be provided (anthropic_api_key, openai_api_key, or gemini_api_key)",
        ));
    }
    Ok(keys)
}

/// Resolve per-provider base URLs into a map keyed by provider name
/// that `populate_realm_from_api_keys` applies to the backend profiles.
///
/// Plan §6 dogma §5: typed per-provider fields only — no bare `base_url`
/// folklore that maps to "whichever provider the default model infers to".
fn build_provider_base_urls(
    anthropic_base_url: Option<&str>,
    openai_base_url: Option<&str>,
    gemini_base_url: Option<&str>,
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
    if urls.is_empty() { None } else { Some(urls) }
}

/// Build the shared service infrastructure from a Config populated with provider keys.
fn build_service_infrastructure(
    config: Config,
    max_sessions: usize,
) -> Result<(Arc<WasmSessionService>, Arc<MobMcpState>), JsValue> {
    // Plan §4d.wasm.1 closure — wire the JS external-auth callback into
    // the provider runtime registry. The resolver itself handles the
    // "no callback registered" case by returning `MissingSecret`; we
    // always register the bridge so realm bindings configured with
    // `CredentialSourceSpec::ExternalResolver { handle: WASM_EXTERNAL_AUTH_RESOLVER_ID }`
    // work whether or not the host page has (yet) installed a callback.
    #[cfg(target_arch = "wasm32")]
    let factory = meerkat::AgentFactory::minimal().with_external_auth_resolver(
        crate::external_auth::WASM_EXTERNAL_AUTH_RESOLVER_ID,
        std::sync::Arc::new(crate::external_auth::WasmExternalAuthResolver),
    );
    #[cfg(not(target_arch = "wasm32"))]
    let factory = meerkat::AgentFactory::minimal();
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);

    // NO default_llm_client — build_agent() resolves the correct provider per-model
    // from realm config bindings. This is architecturally correct: per-agent
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

fn spawn_member_result_payload(
    mob_id: &MobId,
    result: &meerkat_mob::SpawnResult,
) -> meerkat_contracts::MobSpawnManyResultEntry {
    let identity_str = result.agent_identity.to_string();
    meerkat_contracts::MobSpawnManyResultEntry::spawned(
        identity_str.clone(),
        meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
    )
}

fn helper_result_payload(mob_id: &MobId, result: &meerkat_mob::HelperResult) -> serde_json::Value {
    let identity_str = result.agent_identity.to_string();
    serde_json::json!({
        "output": result.output,
        "tokens_used": result.tokens_used,
        "agent_identity": result.agent_identity,
        "member_ref": meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
    })
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

async fn resolve_mob_member_bridge_session_id(
    mob_state: &MobMcpState,
    mob_id: &MobId,
    agent_identity: &AgentIdentity,
) -> Result<meerkat_core::SessionId, JsValue> {
    let handle = mob_state.handle_for(mob_id).await.map_err(err_mob)?;
    handle
        .resolve_bridge_session_id(agent_identity)
        .await
        .ok_or_else(|| {
            err_js(
                "no_session",
                &format!("member '{agent_identity}' has no bridge session"),
            )
        })
}

// ═══════════════════════════════════════════════════════════
// Mobpack Parsing
// ═══════════════════════════════════════════════════════════

#[derive(Debug)]
struct ParsedMobpack {
    manifest: meerkat_mob_pack::manifest::MobpackManifest,
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

    let manifest: meerkat_mob_pack::manifest::MobpackManifest =
        toml::from_str(manifest_text).map_err(|e| format!("invalid manifest.toml: {e}"))?;

    let definition: MobDefinitionHeader = serde_json::from_slice(
        files
            .get("definition.json")
            .ok_or_else(|| "definition.json is missing".to_string())?,
    )
    .map_err(|e| format!("invalid definition.json: {e}"))?;

    if let Some(requires) = &manifest.requires {
        for capability in requires.typed_capabilities() {
            if meerkat_contracts::capability::browser_mobpack_capability_decision(capability)
                .is_forbidden()
            {
                return Err(format!(
                    "forbidden capability '{}' is not allowed in browser-safe mode",
                    capability.raw()
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

// Plan §6.14 deleted the legacy flat-path `create_llm_client` helper.
// Per-session LlmClient construction now flows through AgentFactory::
// build_agent via the provider runtime registry seeded at bootstrap
// (init_runtime_from_config / init_runtime_from_mobpack populate
// `config.realm` via `populate_realm_from_api_keys`).

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
            name: name.into(),
            description,
            input_schema: schema,
            provenance: Some(meerkat_core::types::ToolProvenance {
                kind: meerkat_core::types::ToolSourceKind::Callback,
                source_id: "wasm".into(),
            }),
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
            name: name.into(),
            description,
            input_schema: schema,
            provenance: Some(meerkat_core::types::ToolProvenance {
                kind: meerkat_core::types::ToolSourceKind::Callback,
                source_id: "wasm".into(),
            }),
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
/// `credentials_json`: `{ "anthropic_api_key"?: "...", "openai_api_key"?: "...", "gemini_api_key"?: "...", "model"?: "claude-sonnet-4-5" }`
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
    let base_urls = build_provider_base_urls(
        creds.anthropic_base_url.as_deref(),
        creds.openai_base_url.as_deref(),
        creds.gemini_base_url.as_deref(),
    );
    let mut config = Config::default();
    config.agent.model.clone_from(&model);
    populate_realm_from_api_keys(&mut config, &api_keys, base_urls.as_ref());

    let (session_service, mob_state) = build_service_infrastructure(config, MAX_SESSIONS)?;

    install_runtime_state(RuntimeState {
        mob_state,
        session_service,
        sessions: BTreeMap::new(),
        next_handle: 1,
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
/// `config_json`: `{ "anthropic_api_key"?: "...", "openai_api_key"?: "...", "gemini_api_key"?: "...", "model"?: "claude-sonnet-4-5", "max_sessions"?: 64 }`
#[wasm_bindgen]
pub fn init_runtime_from_config(config_json: &str) -> Result<JsValue, JsValue> {
    init_tracing();
    patch_fetch_response_url();
    let rt_config: RuntimeConfig =
        serde_json::from_str(config_json).map_err(|e| err_str("invalid_config", e))?;

    let api_keys = build_provider_api_keys(
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
    let base_urls = build_provider_base_urls(
        rt_config.anthropic_base_url.as_deref(),
        rt_config.openai_base_url.as_deref(),
        rt_config.gemini_base_url.as_deref(),
    );
    let mut config = Config::default();
    config.agent.model.clone_from(&model);
    populate_realm_from_api_keys(&mut config, &api_keys, base_urls.as_ref());

    let (session_service, mob_state) = build_service_infrastructure(config, max_sessions)?;

    install_runtime_state(RuntimeState {
        mob_state,
        session_service,
        sessions: BTreeMap::new(),
        next_handle: 1,
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

fn build_session_request_with_connection_ref(
    connection_ref: Option<&meerkat_contracts::WireConnectionRef>,
    model: &str,
    config: &SessionConfig,
    system_prompt: Option<String>,
) -> Result<meerkat_core::service::CreateSessionRequest, JsValue> {
    // Credentials flow through bootstrap-populated
    // `config.realm` (populate_realm_from_api_keys) or the host's
    // registered external-auth resolver. No per-session api_key.
    if model.trim().is_empty() {
        return Err(err_js("invalid_config", "model must not be empty"));
    }

    // Reject reserved mob labels in caller-supplied labels map.
    meerkat::surface::validate_raw_labels(config.labels.as_ref())
        .map_err(|e| err_js("invalid_config", &e))?;

    let mut build_config = AgentBuildConfig::new(model);
    if let Some(conn) = connection_ref {
        build_config.connection_ref = Some(conn.clone().into());
    }
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
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
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
    let model = config.model.clone();
    let request = build_session_request_with_connection_ref(
        config.connection_ref.as_ref(),
        &model,
        &config,
        system_prompt,
    )?;

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

    for applied in &current_state.applied {
        if !starting_state.applied.contains(applied) && !agent_state.applied.contains(applied) {
            agent_state.applied.push(applied.clone());
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
pub async fn append_system_context(handle: u32, request_json: &str) -> Result<JsValue, JsValue> {
    let req: AppendSystemContextOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;

    let (session_service, session_id) = with_runtime_state(|state| {
        let session = state
            .sessions
            .get(&handle)
            .ok_or_else(|| err_js("SESSION_NOT_FOUND", &format!("unknown handle: {handle}")))?;
        Ok((state.session_service.clone(), session.session_id.clone()))
    })?;
    let status = session_service
        .append_system_context(
            &session_id,
            meerkat_core::AppendSystemContextRequest {
                text: req.text,
                source: req.source,
                idempotency_key: req.idempotency_key,
            },
        )
        .await
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
pub async fn start_turn(handle: u32, prompt: &str) -> Result<JsValue, JsValue> {
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
                pre_turn_context_appends: Vec::new(),
                turn_metadata: None,
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
/// `action` is a compatibility string carrier that is immediately
/// deserialized into the typed wire lifecycle action contract.
#[wasm_bindgen]
pub async fn mob_lifecycle(mob_id: &str, action: &str) -> Result<(), JsValue> {
    let action = parse_mob_lifecycle_action_arg(action).map_err(|err| {
        err_js(
            "invalid_action",
            &format!("invalid lifecycle action: {err}"),
        )
    })?;
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    // WASM lifecycle wrapper is `() on success`; the structured
    // MobDestroyReport is available through public/RPC lifecycle result
    // envelopes for consumers that need cleanup detail.
    let _destroy_report = mob_state
        .mob_lifecycle_action(&id, action)
        .await
        .map_err(err_mob)?;
    Ok(())
}

fn parse_mob_lifecycle_action_arg(
    action: &str,
) -> Result<meerkat_contracts::WireMobLifecycleAction, serde_json::Error> {
    serde_json::from_value(serde_json::Value::String(action.to_string()))
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

/// Spawn one or more members in a mob.
///
/// `specs_json`: JSON array of `{ "profile": "...", "agent_identity": "...", "initial_message"?: "...",
///                "runtime_mode"?: "autonomous_host"|"turn_driven", "backend"?: "session"|"external" }`
///
/// Returns JSON array of typed result entries per spec.
#[wasm_bindgen]
pub async fn mob_spawn(mob_id: &str, specs_json: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let specs: Vec<SpawnSpecInput> =
        serde_json::from_str(specs_json).map_err(|e| err_str("invalid_specs", e))?;

    let mut spawn_specs = Vec::with_capacity(specs.len());
    for s in specs {
        let mut spec =
            meerkat_mob::SpawnMemberSpec::new(s.profile.as_str(), s.agent_identity.as_str());
        spec.initial_message = s.initial_message;
        spec.runtime_mode = s.runtime_mode;
        spec.backend = s.backend;
        spec.context = s.context;
        spec.labels = s.labels;
        spec.additional_instructions = s.additional_instructions;
        spawn_specs.push(spec);
    }

    let results = mob_state
        .mob_spawn_many(&id, spawn_specs)
        .await
        .map_err(err_mob)?;

    let result_json: Vec<meerkat_contracts::MobSpawnManyResultEntry> = results
        .into_iter()
        .map(|r| match r {
            Ok(spawn_result) => spawn_member_result_payload(&id, &spawn_result),
            Err(e) => meerkat_contracts::MobSpawnManyResultEntry::failed(e.to_string()),
        })
        .collect();

    let json = serde_json::to_string(&result_json).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnSpecInput {
    profile: String,
    #[serde(alias = "meerkat_id")]
    agent_identity: String,
    #[serde(default)]
    initial_message: Option<meerkat_core::types::ContentInput>,
    #[serde(default)]
    runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    backend: Option<meerkat_mob::MobBackendKind>,
    /// Application-defined labels for this member.
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    /// Opaque application context passed through to the agent build pipeline.
    #[serde(default)]
    context: Option<serde_json::Value>,
    /// Legacy 0.6 pre-release SDKs exposed `generation` even though the
    /// runtime owns member generations. Accept and ignore it for raw JS callers.
    #[serde(default, rename = "generation")]
    _generation: Option<u64>,
    /// Additional instruction sections appended to the system prompt.
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
}

/// Retire a member from a mob.
#[wasm_bindgen]
pub async fn mob_retire(mob_id: &str, agent_identity: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let identity = AgentIdentity::from(agent_identity);
    mob_state.mob_retire(&id, identity).await.map_err(err_mob)
}

/// Wire bidirectional trust between two members.
#[wasm_bindgen]
pub async fn mob_wire(mob_id: &str, a: &str, b: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    mob_state
        .mob_wire(
            &id,
            AgentIdentity::from(a),
            meerkat_mob::PeerTarget::Local(AgentIdentity::from(b)),
        )
        .await
        .map_err(err_mob)
}

/// Wire a local member to a local or external peer target.
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
        .mob_wire(&id, AgentIdentity::from(member), target)
        .await
        .map_err(err_mob)
}

/// Unwire bidirectional trust between two members.
#[wasm_bindgen]
pub async fn mob_unwire(mob_id: &str, a: &str, b: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    mob_state
        .mob_unwire(
            &id,
            AgentIdentity::from(a),
            meerkat_mob::PeerTarget::Local(AgentIdentity::from(b)),
        )
        .await
        .map_err(err_mob)
}

/// Unwire a local member from a local or external peer target.
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
        .mob_unwire(&id, AgentIdentity::from(member), target)
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
    // Serialize through the domain shape, then splice in the server-resolved
    // `member_ref` and drop the binding-era `agent_runtime_id` / `fence_token`
    // fields so app code never reasons about incarnation counters.
    let mut array = serde_json::to_value(&members)
        .map_err(|e| err_str("serialize_error", e))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (entry, member) in array.iter_mut().zip(members.iter()) {
        if let Some(obj) = entry.as_object_mut() {
            obj.remove("agent_runtime_id");
            obj.remove("fence_token");
            let identity_str = member.agent_identity.to_string();
            obj.insert(
                "member_ref".to_string(),
                serde_json::to_value(meerkat_contracts::WireMemberRef::encode(
                    id.as_str(),
                    &identity_str,
                ))
                .unwrap_or(serde_json::Value::Null),
            );
        }
    }
    let json = serde_json::to_string(&array).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
}

/// Append runtime system context to an individual mob member's session.
#[wasm_bindgen]
pub async fn mob_append_system_context(
    mob_id: &str,
    agent_identity: &str,
    request_json: &str,
) -> Result<JsValue, JsValue> {
    let req: AppendSystemContextOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;
    let mob_id_typed = MobId::from(mob_id);
    let agent_identity_typed = AgentIdentity::from(agent_identity);

    let (mob_state, session_service) =
        with_runtime_state(|state| Ok((state.mob_state.clone(), state.session_service.clone())))?;
    let bridge_session_id =
        resolve_mob_member_bridge_session_id(&mob_state, &mob_id_typed, &agent_identity_typed)
            .await?;
    let result = session_service
        .append_system_context(
            &bridge_session_id,
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
            "agent_identity": agent_identity,
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
    agent_a: &str,
    mob_b: &str,
    agent_b: &str,
) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;

    // Resolve roster entries and bridge session IDs via mob handles.
    let identity_a = meerkat_mob::AgentIdentity::from(agent_a);
    let identity_b = meerkat_mob::AgentIdentity::from(agent_b);

    let handle_a = mob_state
        .handle_for(&MobId::from(mob_a))
        .await
        .map_err(err_mob)?;
    let entry_a = handle_a
        .get_member(&identity_a)
        .await
        .ok_or_else(|| err_js("no_member", agent_a))?;
    let sid_a = handle_a
        .resolve_bridge_session_id(&identity_a)
        .await
        .ok_or_else(|| err_js("no_session", agent_a))?;

    let handle_b = mob_state
        .handle_for(&MobId::from(mob_b))
        .await
        .map_err(err_mob)?;
    let entry_b = handle_b
        .get_member(&identity_b)
        .await
        .ok_or_else(|| err_js("no_member", agent_b))?;
    let sid_b = handle_b
        .resolve_bridge_session_id(&identity_b)
        .await
        .ok_or_else(|| err_js("no_session", agent_b))?;

    // Get comms runtimes from the shared session service
    let svc = RUNTIME_STATE.with(|cell| {
        let borrow = cell.borrow();
        let state = borrow
            .as_ref()
            .ok_or_else(|| err_js("not_initialized", ""))?;
        Ok::<_, JsValue>(state.session_service.clone())
    })?;

    let comms_a = svc
        .comms_runtime(&sid_a)
        .await
        .ok_or_else(|| err_js("no_comms", &format!("{agent_a} has no comms runtime")))?;
    let comms_b = svc
        .comms_runtime(&sid_b)
        .await
        .ok_or_else(|| err_js("no_comms", &format!("{agent_b} has no comms runtime")))?;

    let key_a = comms_a
        .public_key()
        .ok_or_else(|| err_js("no_key", agent_a))?;
    let key_b = comms_b
        .public_key()
        .ok_or_else(|| err_js("no_key", agent_b))?;

    // Build peer specs with full mob/profile/meerkat addressing
    let name_a = format!("{mob_a}/{}/{agent_a}", entry_a.role);
    let name_b = format!("{mob_b}/{}/{agent_b}", entry_b.role);

    let spec_a = build_inproc_trusted_peer(&name_a, &key_a)?;
    let spec_b = build_inproc_trusted_peer(&name_b, &key_b)?;

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

/// Build an inproc [`TrustedPeerDescriptor`] for intra-/cross-mob wire in the
/// embedded wasm runtime.
///
/// V5 dogma: the core seam is keyed by typed [`PeerId`] (a UUIDv5 derived
/// from the Ed25519 signing pubkey), not by display name. This helper
/// parses the `ed25519:<base64>` form returned by
/// [`CommsRuntime::public_key`] back into typed `PubKey` bytes, derives the
/// canonical `PeerId` via
/// [`meerkat_comms::router::peer_id_from_pubkey`] so router and trust-store
/// lookups round-trip, and stamps the 32-byte pubkey on the descriptor so
/// receiver-side signature verification continues to work.
fn build_inproc_trusted_peer(
    name: &str,
    pubkey_str: &str,
) -> Result<meerkat_core::comms::TrustedPeerDescriptor, JsValue> {
    let pubkey = meerkat_comms::identity::PubKey::from_pubkey_string(pubkey_str)
        .map_err(|e| err_str("wire_error", format!("invalid pubkey `{pubkey_str}`: {e}")))?;
    let peer_id = meerkat_comms::router::peer_id_from_pubkey(&pubkey);
    let peer_name = meerkat_core::comms::PeerName::new(name)
        .map_err(|e| err_str("wire_error", format!("invalid peer name `{name}`: {e}")))?;
    Ok(meerkat_core::comms::TrustedPeerDescriptor {
        peer_id,
        name: peer_name,
        address: meerkat_core::comms::PeerAddress::new(
            meerkat_core::comms::PeerTransport::Inproc,
            name,
        ),
        pubkey: *pubkey.as_bytes(),
    })
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
    agent_identity: Option<String>,
    #[serde(default)]
    role_name: Option<String>,
    #[serde(default)]
    connection_ref: Option<meerkat_contracts::WireConnectionRef>,
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
    agent_identity: Option<String>,
    #[serde(default)]
    role_name: Option<String>,
    #[serde(default)]
    connection_ref: Option<meerkat_contracts::WireConnectionRef>,
    #[serde(default)]
    fork_context: Option<meerkat_mob::ForkContext>,
    #[serde(default)]
    runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    backend: Option<meerkat_mob::MobBackendKind>,
}

/// Send external work to a spawned member through the canonical member path.
///
/// Returns a JSON-encoded delivery receipt.
#[wasm_bindgen]
pub async fn mob_member_send(
    mob_id: &str,
    agent_identity: &str,
    request_json: &str,
) -> Result<String, JsValue> {
    let req: MobMemberSendOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = AgentIdentity::from(agent_identity);
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
    let identity_str = receipt.identity.to_string();
    let payload = serde_json::json!({
        "agent_identity": receipt.identity,
        "member_ref": meerkat_contracts::WireMemberRef::encode(id.as_str(), &identity_str),
        "handling_mode": match receipt.handling_mode {
            meerkat_core::HandlingMode::Queue => "queue",
            meerkat_core::HandlingMode::Steer => "steer",
        },
    });
    serde_json::to_string(&payload).map_err(|e| err_str("serialize", e))
}

/// Read the current execution snapshot for a mob member.
#[wasm_bindgen]
pub async fn mob_member_status(mob_id: &str, agent_identity: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = AgentIdentity::from(agent_identity);
    let snapshot = mob_state
        .mob_member_status(&id, &mid)
        .await
        .map_err(err_mob)?;
    let json = serde_json::to_string(&snapshot).map_err(|e| err_str("serialize", e))?;
    Ok(JsValue::from_str(&json))
}

/// Retire and re-spawn a member with the same profile.
///
/// Returns JSON result envelope with receipt.
#[wasm_bindgen]
pub async fn mob_respawn(
    mob_id: &str,
    agent_identity: &str,
    initial_message: Option<String>,
) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = AgentIdentity::from(agent_identity);
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
pub async fn mob_force_cancel(mob_id: &str, agent_identity: &str) -> Result<(), JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let mid = AgentIdentity::from(agent_identity);
    mob_state.mob_force_cancel(&id, mid).await.map_err(err_mob)
}

/// Spawn a short-lived helper and return its terminal result.
#[wasm_bindgen]
pub async fn mob_spawn_helper(mob_id: &str, request_json: &str) -> Result<JsValue, JsValue> {
    let request: MobSpawnHelperOptions =
        serde_json::from_str(request_json).map_err(|e| err_str("invalid_request", e))?;
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let identity = AgentIdentity::from(
        request
            .agent_identity
            .unwrap_or_else(|| format!("helper-{}", Uuid::new_v4())),
    );
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role_name) = request.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role_name));
    }
    if let Some(connection_ref) = request.connection_ref {
        options.connection_ref = Some(connection_ref.into());
    }
    options.runtime_mode = request.runtime_mode;
    options.backend = request.backend;
    let result = mob_state
        .mob_spawn_helper(&id, identity, request.prompt, options)
        .await
        .map_err(err_mob)?;
    let json = serde_json::to_string(&helper_result_payload(&id, &result))
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
    let source_member_id = AgentIdentity::from(request.source_member_id.as_str());
    let identity = AgentIdentity::from(
        request
            .agent_identity
            .unwrap_or_else(|| format!("fork-{}", Uuid::new_v4())),
    );
    let fork_context = request
        .fork_context
        .unwrap_or(meerkat_mob::ForkContext::FullHistory);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role_name) = request.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role_name));
    }
    if let Some(connection_ref) = request.connection_ref {
        options.connection_ref = Some(connection_ref.into());
    }
    options.runtime_mode = request.runtime_mode;
    options.backend = request.backend;
    let result = mob_state
        .mob_fork_helper(
            &id,
            &source_member_id,
            identity,
            request.prompt,
            fork_context,
            options,
        )
        .await
        .map_err(err_mob)?;
    let json = serde_json::to_string(&helper_result_payload(&id, &result))
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
// Event Streaming — subscribe to individual member sessions
// ═══════════════════════════════════════════════════════════

/// Subscription inner type: per-member broadcast or mob-wide mpsc.
enum SubscriptionInner {
    /// Per-member session broadcast receiver.
    Member(std::cell::RefCell<meerkat_session::BroadcastEventReceiver>),
    /// Mob-wide attributed event receiver from the event router.
    /// The entire handle is stored to keep the router task alive (Drop cancels it).
    MobWide(std::cell::RefCell<meerkat_mob::MobEventRouterHandle>),
    #[cfg(all(test, target_arch = "wasm32"))]
    InjectedProjectionFailure,
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
pub async fn mob_member_subscribe(mob_id: &str, agent_identity: &str) -> Result<u32, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let mob_id_typed = MobId::from(mob_id);
    let mid = AgentIdentity::from(agent_identity);

    let bridge_session_id =
        resolve_mob_member_bridge_session_id(&mob_state, &mob_id_typed, &mid).await?;

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
        .subscribe_session_events_raw(&bridge_session_id)
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
/// this streams [`AttributedEvent`]s tagged with source runtime identity and role
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
/// If any buffered event cannot be projected to JSON, returns a typed
/// `serialize_error` instead of silently omitting the event.
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
                        Ok(event) => events.push(
                            serialize_subscription_item(&event, "subscription agent event")
                                .map_err(|message| err_js("serialize_error", &message))?,
                        ),
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
                    events.push(
                        serialize_subscription_item(&attributed, "subscription attributed event")
                            .map_err(|message| err_js("serialize_error", &message))?,
                    );
                }
            }
            #[cfg(all(test, target_arch = "wasm32"))]
            SubscriptionInner::InjectedProjectionFailure => {
                struct FailingProjection;

                impl Serialize for FailingProjection {
                    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
                    where
                        S: serde::Serializer,
                    {
                        Err(serde::ser::Error::custom("injected projection failure"))
                    }
                }

                events.push(
                    serialize_subscription_item(
                        &FailingProjection,
                        "subscription injected test event",
                    )
                    .map_err(|message| err_js("serialize_error", &message))?,
                );
            }
        }
        serde_json::to_string(&events).map_err(|e| err_str("serialize_error", e))
    })
}

fn serialize_subscription_item<T: Serialize + ?Sized>(
    item: &T,
    projection: &str,
) -> Result<serde_json::Value, String> {
    serde_json::to_value(item).map_err(|e| format!("failed to serialize {projection}: {e}"))
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
    use super::{
        EventSubscription, SUBSCRIPTIONS, SubscriptionInner, close_subscription,
        merge_runtime_system_context_state, parse_mob_lifecycle_action_arg, parse_mobpack,
        poll_subscription, serialize_subscription_item,
    };
    #[cfg(target_arch = "wasm32")]
    use super::{
        append_system_context, create_session_simple, destroy_session, get_session_state,
        init_runtime_from_config,
    };
    #[cfg(not(target_arch = "wasm32"))]
    use super::{build_service_infrastructure, populate_realm_from_api_keys};
    #[cfg(not(target_arch = "wasm32"))]
    use super::{helper_result_payload, spawn_member_result_payload};
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
    use meerkat_mob::{MobId, SpawnMemberSpec};
    use serde_json::json;
    #[cfg(not(target_arch = "wasm32"))]
    use std::collections::HashMap;

    fn test_mobpack_bytes(capabilities: &[&str]) -> Vec<u8> {
        let capability_values = capabilities
            .iter()
            .map(|capability| format!("\"{capability}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let manifest = format!(
            r#"[mobpack]
name = "browser-test"
version = "1.0.0"

[requires]
capabilities = [{capability_values}]
"#
        );

        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append_test_mobpack_file(&mut builder, "manifest.toml", manifest.as_bytes());
        append_test_mobpack_file(&mut builder, "definition.json", br#"{"id":"browser-test"}"#);
        builder.finish().expect("finish tar archive");
        let encoder = builder.into_inner().expect("take gzip encoder");
        encoder.finish().expect("finish gzip archive")
    }

    fn append_test_mobpack_file<W: std::io::Write>(
        builder: &mut tar::Builder<W>,
        path: &str,
        bytes: &[u8],
    ) {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).expect("set test mobpack path");
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        builder
            .append(&header, bytes)
            .expect("append test mobpack file");
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn test_connection_ref() -> meerkat_core::ConnectionRef {
        meerkat_core::ConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("default").expect("test realm id"),
            binding: meerkat_core::connection::BindingId::parse("default_anthropic")
                .expect("test binding id"),
            profile: None,
        }
    }

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
            applied: Vec::new(),
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
            applied: vec![initial_pending.clone()],
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
            applied: Vec::new(),
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

    #[test]
    fn parse_mobpack_accepts_browser_safe_typed_capabilities() {
        let bytes = test_mobpack_bytes(&["sessions", "comms", "vendor.custom"]);

        let parsed = parse_mobpack(&bytes).expect("browser-safe mobpack should parse");

        assert_eq!(
            parsed
                .manifest
                .requires
                .as_ref()
                .expect("requires section")
                .capabilities,
            vec![
                "sessions".to_string(),
                "comms".to_string(),
                "vendor.custom".to_string(),
            ]
        );
    }

    #[test]
    fn parse_mobpack_rejects_browser_forbidden_typed_capabilities() {
        for capability in ["shell", "mcp_stdio", "process_spawn"] {
            let bytes = test_mobpack_bytes(&[capability]);

            let error = parse_mobpack(&bytes).expect_err("browser-forbidden capability rejected");

            assert!(
                error.contains(&format!("forbidden capability '{capability}'")),
                "unexpected error for {capability}: {error}"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::expect_used)]
    #[tokio::test(flavor = "current_thread")]
    async fn mob_append_system_context_targets_member_session() {
        let mut config = Config::default();
        populate_realm_from_api_keys(
            &mut config,
            &HashMap::from([("anthropic".to_string(), "sk-test".to_string())]),
            None,
        );
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

        let _spawn_results = mob_state
            .mob_spawn_many(
                &mob_id,
                vec![
                    SpawnMemberSpec::new("worker", "worker-1")
                        .with_connection_ref(test_connection_ref())
                        .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
                ],
            )
            .await
            .expect("spawn worker");
        let handle = mob_state.handle_for(&mob_id).await.expect("mob handle");
        let bridge_session_id = handle
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("worker-1"))
            .await
            .expect("member bridge session id");

        let append = service
            .append_system_context(
                &bridge_session_id,
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
            .export_session(&bridge_session_id)
            .await
            .expect("export member bridge session");
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

    #[test]
    fn spawn_spec_input_rejects_legacy_resume_session_fields() {
        let sid = meerkat_core::SessionId::new();
        let payload = json!([{
            "profile": "worker",
            "agent_identity": "worker-1",
            "resume_bridge_session_id": sid,
        }]);

        let specs_result: Result<Vec<super::SpawnSpecInput>, _> = serde_json::from_value(payload);
        assert!(
            specs_result.is_err(),
            "legacy resume bridge/session fields should be rejected in 0.6"
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn spawn_spec_input_accepts_legacy_meerkat_id_alias() {
        let payload = json!([{
            "profile": "worker",
            "meerkat_id": "worker-1",
            "runtime_mode": "turn_driven",
        }]);

        let specs: Vec<super::SpawnSpecInput> =
            serde_json::from_value(payload).expect("legacy meerkat_id alias should deserialize");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].agent_identity, "worker-1");
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn spawn_spec_input_ignores_legacy_generation() {
        let payload = json!([{
            "profile": "worker",
            "agent_identity": "worker-1",
            "generation": 99,
        }]);

        let specs: Vec<super::SpawnSpecInput> =
            serde_json::from_value(payload).expect("legacy generation should be ignored");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].agent_identity, "worker-1");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::expect_used)]
    #[test]
    fn spawn_member_result_payload_returns_typed_spawn_many_entry() {
        let identity = meerkat_mob::AgentIdentity::from("test-member");
        let runtime_id = meerkat_mob::AgentRuntimeId::initial(identity.clone());
        let fence = meerkat_mob::FenceToken::new(1);
        let result = meerkat_mob::SpawnResult::new(identity, runtime_id, fence);
        let mob_id = MobId::from("mob-test");

        let entry = spawn_member_result_payload(&mob_id, &result);
        let payload = serde_json::to_value(&entry).expect("serialize typed spawn_many result");
        assert_eq!(payload["status"], "spawned");
        assert_eq!(payload["result"]["agent_identity"], "test-member");
        assert!(
            payload.get("agent_identity").is_none(),
            "spawn_many row must not retain flat agent_identity carrier"
        );
        assert!(
            payload.get("member_ref").is_none(),
            "spawn_many row must not retain flat member_ref carrier"
        );
        assert!(
            payload.get("ok").is_none(),
            "spawn_many row must not retain legacy ok carrier"
        );
        assert!(
            payload.get("error").is_none(),
            "spawn_many row must not retain string-error carrier"
        );
        assert!(
            payload.get("agent_runtime_id").is_none(),
            "binding-era agent_runtime_id must not leak to app-facing payloads"
        );
        let member_ref = payload["result"]["member_ref"]
            .as_str()
            .expect("member_ref");
        assert!(!member_ref.is_empty(), "member_ref must be populated");

        let round_trip: meerkat_contracts::MobSpawnManyResultEntry =
            serde_json::from_value(payload).expect("typed spawn_many entry must deserialize");
        assert_eq!(round_trip, entry);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::expect_used)]
    #[test]
    fn spawn_member_failure_payload_uses_typed_result_message() {
        let entry = meerkat_contracts::MobSpawnManyResultEntry::failed("profile missing");
        let payload = serde_json::to_value(&entry).expect("serialize failed spawn_many result");

        assert_eq!(payload["status"], "failed");
        assert_eq!(payload["result"]["message"], "profile missing");
        assert!(
            payload.get("error").is_none(),
            "failed spawn_many row must not retain string-error carrier"
        );

        let round_trip: meerkat_contracts::MobSpawnManyResultEntry =
            serde_json::from_value(payload)
                .expect("typed failed spawn_many entry must deserialize");
        assert_eq!(round_trip, entry);
    }

    #[test]
    fn mob_lifecycle_action_string_carrier_uses_typed_contract_boundary() {
        let action = parse_mob_lifecycle_action_arg("resume")
            .expect("valid lifecycle action must deserialize");
        assert_eq!(action, meerkat_contracts::WireMobLifecycleAction::Resume);

        let err = parse_mob_lifecycle_action_arg("explode")
            .expect_err("unknown lifecycle action must fail typed deserialization");
        assert!(
            err.to_string().contains("unknown variant"),
            "unexpected error: {err}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::expect_used)]
    #[tokio::test(flavor = "current_thread")]
    async fn helper_result_payload_returns_identity_native_fields() {
        let mut config = Config::default();
        populate_realm_from_api_keys(
            &mut config,
            &HashMap::from([("anthropic".to_string(), "sk-test".to_string())]),
            None,
        );
        let (_service, mob_state) =
            build_service_infrastructure(config, 8).expect("build runtime services");

        let definition: meerkat_mob::MobDefinition = serde_json::from_value(json!({
            "id": "mob-web-helper-payload",
            "orchestrator": { "profile": "lead" },
            "profiles": {
                "lead": { "model": "claude-opus-4-6", "tools": { "mob": true, "comms": true } },
                "worker": { "model": "claude-sonnet-4-5", "tools": { "mob": true, "comms": true } }
            }
        }))
        .expect("mob definition");
        let mob_id = mob_state
            .mob_create_definition(definition)
            .await
            .expect("create mob");
        let mut options = meerkat_mob::HelperOptions::default();
        options.role_name = Some(meerkat_mob::ProfileName::from("worker"));
        options.connection_ref = Some(test_connection_ref());

        let result = mob_state
            .mob_spawn_helper(
                &mob_id,
                meerkat_mob::AgentIdentity::from("helper-1"),
                "say hi".to_string(),
                options,
            )
            .await
            .expect("spawn helper");

        let payload = helper_result_payload(&mob_id, &result);
        assert!(payload.get("output").is_some());
        assert!(payload["tokens_used"].as_u64().is_some());
        assert_eq!(payload["agent_identity"], "helper-1");
        assert!(
            payload.get("agent_runtime_id").is_none(),
            "binding-era agent_runtime_id must not leak to app-facing payloads"
        );
        let member_ref = payload["member_ref"].as_str().expect("member_ref");
        assert!(!member_ref.is_empty(), "member_ref must be populated");
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test(async)]
    async fn append_system_context_export_stages_context_for_direct_session_handle() {
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
        .await
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
    fn poll_subscription_empty_success_is_clean_empty_array() {
        let (_tx, rx) = crate::tokio::sync::broadcast::channel(1);

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

        let payload = poll_subscription(handle).expect("empty poll should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&payload).expect("empty poll should be json");
        assert_eq!(
            parsed.as_array().map(Vec::len),
            Some(0),
            "clean empty poll must remain a successful empty array"
        );

        let close_result = close_subscription(handle);
        assert!(close_result.is_ok());
    }

    #[test]
    fn serialize_subscription_item_reports_projection_failure() {
        struct FailingProjection;

        impl serde::Serialize for FailingProjection {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("injected projection failure"))
            }
        }

        let error =
            serialize_subscription_item(&FailingProjection, "subscription injected test event")
                .expect_err("projection failure should be reported");

        assert!(
            error.contains("failed to serialize subscription injected test event"),
            "projection failure should name the failed subscription projection"
        );
        assert!(
            error.contains("injected projection failure"),
            "projection failure should preserve the serialization cause"
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn attributed_subscription_item_serializes_runtime_id_source_shape() {
        let source = meerkat_mob::AgentRuntimeId::new(
            meerkat_mob::AgentIdentity::from("worker-runtime"),
            meerkat_mob::Generation::new(3),
        );
        let attributed = meerkat_mob::AttributedEvent {
            source,
            source_fence_token: meerkat_mob::FenceToken::new(9),
            role: meerkat_mob::ProfileName::from("worker"),
            envelope: meerkat_core::EventEnvelope::new(
                "worker-runtime",
                7,
                Some("mob-web-unit".to_string()),
                meerkat_core::AgentEvent::TextDelta {
                    delta: "hello".to_string(),
                },
            ),
        };

        let parsed = serialize_subscription_item(&attributed, "subscription attributed event")
            .expect("canonical attributed subscription event should serialize");

        assert!(
            parsed["source"].as_str().is_none(),
            "AgentRuntimeId source must not be projected as a lossy string"
        );
        assert_eq!(parsed["source"]["identity"], "worker-runtime");
        assert_eq!(parsed["source"]["generation"], 3);
        assert_eq!(parsed["source_fence_token"], 9);
        assert_eq!(parsed["role"], "worker");
        assert_eq!(parsed["envelope"]["payload"]["type"], "text_delta");
        assert_eq!(parsed["envelope"]["payload"]["delta"], "hello");
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test]
    #[allow(clippy::expect_used)]
    fn poll_subscription_fails_closed_when_projection_serialization_fails() {
        let handle = SUBSCRIPTIONS.with(|cell| {
            let mut registry = cell.borrow_mut();
            let handle = registry.next_handle;
            registry.next_handle = registry.next_handle.wrapping_add(1);
            registry.subscriptions.insert(
                handle,
                EventSubscription {
                    inner: SubscriptionInner::InjectedProjectionFailure,
                },
            );
            handle
        });

        let error =
            poll_subscription(handle).expect_err("projection failure must fail public poll");
        let error_json = error.as_string().expect("typed error json");
        let parsed: serde_json::Value =
            serde_json::from_str(&error_json).expect("typed error should be json");
        assert_eq!(parsed["code"], "serialize_error");
        assert!(
            parsed["message"]
                .as_str()
                .unwrap_or_default()
                .contains("injected projection failure"),
            "projection failure should surface the serialization cause"
        );

        let close_result = close_subscription(handle);
        assert!(close_result.is_ok());
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
