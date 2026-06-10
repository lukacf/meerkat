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
use std::sync::Arc;
use wasm_bindgen::prelude::*;

use meerkat::{AgentBuildConfig, SessionServiceControlExt};
use meerkat_core::{Config, SessionService};
use meerkat_mob::{AgentIdentity, FlowId, MobDefinition, MobId, RunId};
use meerkat_mob_mcp::{MobMcpDestroyError, MobMcpState};

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
const MAX_SESSIONS: usize = 100_000;

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
    /// Optional structural auth binding reference. When set, overrides the
    /// default provider-match from bootstrap-populated `config.realm`.
    #[serde(default)]
    auth_binding: Option<meerkat_contracts::WireAuthBindingRef>,
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
    /// Optional explicit model override. When unset, the default is resolved
    /// from the catalog for the highest-priority configured provider.
    #[serde(default)]
    model: Option<String>,
    /// Per-provider base URLs. Unset providers default to the upstream vendor URL.
    #[serde(default)]
    anthropic_base_url: Option<String>,
    #[serde(default)]
    openai_base_url: Option<String>,
    #[serde(default)]
    gemini_base_url: Option<String>,
    /// Maximum concurrent sessions for the bootstrapped service.
    #[serde(default = "default_max_sessions")]
    max_sessions: usize,
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
    /// Optional explicit model override. When unset, the default is resolved
    /// from the catalog for the highest-priority configured provider.
    #[serde(default)]
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

/// Normalize a credentials-map provider key to its typed identity.
///
/// The `google` alias maps to the Gemini identity; unrecognized keys yield
/// `None`.
fn canonical_provider_key(key: &str) -> Option<meerkat_core::Provider> {
    let normalized = if key == "google" { "gemini" } else { key };
    meerkat_core::Provider::parse_strict(normalized)
}

/// Build the bootstrap `Config` from this surface's actual inputs (explicit
/// model override + configured API keys + base URLs) and resolve the default
/// model through the canonical create-session ladder.
///
/// The browser bootstrap synthesizes its config from `credentials_json`
/// alone: the embedded template's global agent model is an operator-file fact
/// that does not exist on this surface, so `config.agent.model` carries only
/// the caller's explicit override (or nothing). After
/// [`populate_realm_from_api_keys`] makes the config truthful about which
/// providers are configured, the model decision itself is owned by
/// [`meerkat::resolve_create_session_default_model`] — this surface keeps no
/// parallel default-model ladder.
fn build_bootstrap_config(
    explicit_model: Option<String>,
    api_keys: &HashMap<String, String>,
    base_urls: Option<&HashMap<String, String>>,
) -> (meerkat_core::Config, String) {
    let mut config = Config::default();
    config.agent.model = explicit_model.unwrap_or_default();
    populate_realm_from_api_keys(&mut config, api_keys, base_urls);
    let model = meerkat::resolve_create_session_default_model(&config);
    config.agent.model.clone_from(&model);
    (config, model)
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
    /// Trust-verified bootstrap mobpack (definition id + skills), consumed when
    /// `create_session_simple` builds a standalone session so the verified pack
    /// contributes its system prompt rather than being discarded at init.
    bootstrap_mobpack: Option<BootstrapMobpack>,
    #[cfg(target_arch = "wasm32")]
    js_tools: Vec<JsToolEntry>,
}

/// Verified bootstrap mobpack retained for standalone session system-prompt
/// assembly. Built only after `verify_extracted_pack_trust` succeeds.
struct BootstrapMobpack {
    definition_id: String,
    name: String,
    skills: BTreeMap<String, String>,
}

impl BootstrapMobpack {
    fn from_parsed(parsed: ParsedMobpack) -> Self {
        Self {
            definition_id: parsed.definition.id,
            name: parsed.manifest.mobpack.name,
            skills: parsed.skills,
        }
    }

    fn compile_system_prompt(&self, user_system_prompt: Option<&str>) -> String {
        compile_system_prompt(
            &self.definition_id,
            &self.name,
            &self.skills,
            user_system_prompt,
        )
    }
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
    // Deterministic cross-provider order owned by the model catalog
    // (`provider_priority()` == [Anthropic, OpenAI, Gemini]). Each provider's
    // sort index is its position in that list; providers absent from the list
    // (and unrecognized strings) sort last. The `google` alias normalizes to
    // the Gemini identity before lookup.
    let priority = meerkat_core::model_profile::catalog::provider_priority();
    let provider_rank = |key: &str| -> usize {
        match canonical_provider_key(key) {
            Some(provider) => priority
                .iter()
                .position(|p| *p == provider)
                .unwrap_or(priority.len()),
            None => priority.len(),
        }
    };
    entries.sort_by_key(|(k, _)| provider_rank(k));

    // A per-provider default model is only a true config fact for providers
    // that have a key-backed binding in this realm — but `Config::default()`
    // pre-fills `config.models` for every catalog provider. Clear the
    // unconfigured entries so the canonical create-session default-model
    // ladder (which walks non-empty `config.models` entries in catalog
    // priority order) sees only the providers this surface actually
    // configured.
    let configured = |provider: meerkat_core::Provider| {
        api_keys
            .keys()
            .any(|key| canonical_provider_key(key) == Some(provider))
    };
    if !configured(meerkat_core::Provider::Anthropic) {
        config.models.anthropic.clear();
    }
    if !configured(meerkat_core::Provider::OpenAI) {
        config.models.openai.clear();
    }
    if !configured(meerkat_core::Provider::Gemini) {
        config.models.gemini.clear();
    }

    let mut section = meerkat_core::RealmConfigSection::from_inline_api_keys(&entries);
    if let Some(urls) = base_urls {
        for (provider, url) in urls {
            // Look up the backend by its typed provider identity rather than
            // re-synthesizing the `default_<provider>` id name convention.
            // `from_inline_api_keys` mints one backend per provider, keyed by
            // the provider string it was given.
            if let Some(bp) = section
                .backend
                .values_mut()
                .find(|bp| bp.provider == *provider)
            {
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

/// Host-safe structured error envelope (`{code, message}`). The wasm surface
/// wraps it into a `JsValue` via [`js_from_value`]; keeping the typed envelope
/// as the Rust-owned value lets non-wasm unit tests assert the typed `code`
/// without constructing a `JsValue` (which panics outside wasm).
fn err_value(code: &str, message: impl std::fmt::Display) -> serde_json::Value {
    serde_json::json!({ "code": code, "message": message.to_string() })
}

fn js_from_value(value: serde_json::Value) -> JsValue {
    JsValue::from_str(&value.to_string())
}

fn err_js(code: &str, message: &str) -> JsValue {
    js_from_value(err_value(code, message))
}

fn err_str(code: &str, msg: impl std::fmt::Display) -> JsValue {
    err_js(code, &msg.to_string())
}

fn err_mob(e: meerkat_mob::MobError) -> JsValue {
    err_str("mob_error", e)
}

fn mob_destroy_error_value(e: MobMcpDestroyError) -> serde_json::Value {
    match e {
        MobMcpDestroyError::Incomplete { report } => serde_json::json!({
            "code": "mob_destroy_incomplete",
            "message": MobMcpDestroyError::incomplete_message(&report),
            "destroy_report": report,
            "retryable": true,
        }),
        MobMcpDestroyError::Mob(error) => {
            serde_json::json!({ "code": "mob_error", "message": error.to_string() })
        }
    }
}

fn err_mob_destroy(e: MobMcpDestroyError) -> JsValue {
    JsValue::from_str(&mob_destroy_error_value(e).to_string())
}

/// Build the typed `{ code, message, ... }` error envelope for a
/// [`meerkat_core::SessionError`]. Each variant carries its stable
/// `SessionError::code()` so callers can classify the terminal fault — agent
/// faults surface as `AGENT_ERROR`, never flattened to a generic
/// `internal_error` and never laundered into an Ok-with-status payload.
fn session_error_envelope(e: meerkat_core::SessionError) -> serde_json::Value {
    match e {
        meerkat_core::SessionError::NotFound { .. } => {
            serde_json::json!({ "code": "SESSION_NOT_FOUND", "message": "session not found" })
        }
        meerkat_core::SessionError::Busy { .. } => {
            serde_json::json!({ "code": "SESSION_BUSY", "message": "session is busy" })
        }
        meerkat_core::SessionError::PersistenceDisabled => serde_json::json!({
            "code": "SESSION_PERSISTENCE_DISABLED",
            "message": "session persistence is disabled",
        }),
        meerkat_core::SessionError::CompactionDisabled => serde_json::json!({
            "code": "SESSION_COMPACTION_DISABLED",
            "message": "session compaction is disabled",
        }),
        meerkat_core::SessionError::NotRunning { .. } => {
            serde_json::json!({ "code": "SESSION_NOT_RUNNING", "message": "session is not running" })
        }
        meerkat_core::SessionError::Unsupported(message) => {
            serde_json::json!({ "code": "SESSION_UNSUPPORTED", "message": message })
        }
        meerkat_core::SessionError::FailedWithData { message, data } => {
            let mut data = data;
            if let serde_json::Value::Object(map) = &mut data {
                map.entry("message".to_string())
                    .or_insert_with(|| serde_json::Value::String(message));
            }
            data
        }
        meerkat_core::SessionError::Store(_) => {
            serde_json::json!({ "code": "SESSION_STORE_ERROR", "message": e.to_string() })
        }
        // Carry the typed `SessionError::code()` ("AGENT_ERROR") rather than
        // flattening agent faults into a generic "internal_error", so callers
        // can classify the terminal fault on the Err channel.
        meerkat_core::SessionError::Agent(_) => {
            serde_json::json!({ "code": e.code(), "message": e.to_string() })
        }
    }
}

fn err_session(e: meerkat_core::SessionError) -> JsValue {
    JsValue::from_str(&session_error_envelope(e).to_string())
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

fn err_invalid_session_handle(handle: u32) -> JsValue {
    err_js(
        "invalid_session_handle",
        &format!("unknown browser session handle: {handle}"),
    )
}

/// Parse a caller-supplied prompt into typed [`meerkat_core::types::ContentInput`],
/// failing closed when the prompt is structured-but-malformed content.
///
/// The Web SDK serializes a `ContentBlock[]` prompt as a JSON array
/// (`WireContentInput::Blocks`) and a plain text prompt as a bare string. Both
/// `ContentInput` and `WireContentInput` are `#[serde(untagged)]` (string OR
/// array), so the JSON shape is the discriminator.
///
/// Rule 4 / Rule 8: a JSON value that is block-shaped (array or object) but does
/// not deserialize into `ContentInput` must surface `INVALID_PARAMS` rather than
/// being silently misread as a single plain-text block. Only a prompt that is
/// genuinely not structured JSON (a non-JSON string, or a JSON string literal)
/// is treated as text.
fn parse_prompt_content_input(
    prompt: &str,
) -> Result<meerkat_core::types::ContentInput, serde_json::Value> {
    match serde_json::from_str::<serde_json::Value>(prompt) {
        // A JSON array is the only block-shaped `ContentInput`/`WireContentInput`
        // form (the Web SDK emits `ContentBlock[]` as a JSON array). Deserialize
        // it strictly and fail closed — a malformed block array must never be
        // silently downgraded to a single plain-text block.
        Ok(value @ serde_json::Value::Array(_)) => {
            serde_json::from_value::<meerkat_core::types::ContentInput>(value).map_err(|e| {
                err_value(
                    "INVALID_PARAMS",
                    format!("prompt is structured block content but not valid ContentInput: {e}"),
                )
            })
        }
        // Anything else (a plain non-JSON string, a JSON object/number/bool, or a
        // bare JSON string literal) is treated as plain text. The SDK never
        // JSON-encodes plain text, so a non-array value is the caller's literal
        // text rather than structured block content.
        _ => Ok(meerkat_core::types::ContentInput::from(prompt)),
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
    parse_mobpack_from_files(&files)
}

/// Extract, trust-verify, and parse a mobpack for runtime bootstrap.
///
/// Rule 8 / browser-safety: the browser advertises "initialization from a
/// mobpack", so it must run the canonical [`meerkat_mob_pack::trust`]
/// verification — not just a local parser. Verification runs under
/// [`TrustPolicy::Strict`] by default so an unsigned or untrusted pack fails
/// closed. The verified [`ParsedMobpack`] (definition + skills) is then returned
/// for the runtime build instead of being discarded.
fn extract_verify_and_parse_mobpack(
    bytes: &[u8],
    trust_policy: meerkat_mob_pack::trust::TrustPolicy,
    trusted_signers: &meerkat_mob_pack::trust::TrustedSigners,
) -> Result<ParsedMobpack, serde_json::Value> {
    let files = extract_targz_safe(bytes).map_err(|e| {
        err_value(
            "invalid_mobpack",
            format!("failed to parse mobpack archive: {e}"),
        )
    })?;
    meerkat_mob_pack::trust::verify_extracted_pack_trust(&files, trust_policy, trusted_signers)
        .map_err(|e| err_value("untrusted_mobpack", e))?;
    parse_mobpack_from_files(&files).map_err(|e| err_value("invalid_mobpack", e))
}

fn parse_mobpack_from_files(files: &BTreeMap<String, Vec<u8>>) -> Result<ParsedMobpack, String> {
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
    for (path, content) in files {
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

/// Extract a mobpack tar.gz using the typed extraction owner in
/// `meerkat-mob-pack` (single owner of archive path-safety semantics).
fn extract_targz_safe(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    meerkat_mob_pack::targz::extract_targz_safe(bytes).map_err(|error| error.to_string())
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
        // Fire-and-forget tools defer the real work to the host: the host
        // watches ToolCallRequested events in the stream and acts on the call
        // asynchronously. Rule 8: do NOT fabricate an `is_error:false`
        // synchronous *success* (which would assert the host action completed
        // before it happened). Instead, model it as a typed detached async op so
        // the turn boundary does not block on (or assert) the deferred action,
        // and label the transcript result as pending rather than completed.
        if Self::is_fire_and_forget(call.name) {
            let detached =
                meerkat_core::ops::AsyncOpRef::detached(meerkat_core::ops::OperationId::new());
            let pending = meerkat_core::ToolResult::new(
                call.id.to_string(),
                "deferred to host: this tool call was dispatched to the host \
                 asynchronously and has not completed; its result (if any) arrives \
                 through a later session/mob message, not this tool result"
                    .to_string(),
                false,
            );
            return Ok(meerkat_core::ops::ToolDispatchOutcome::new(
                pending,
                vec![detached],
                Vec::new(),
            ));
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

        Ok(parse_js_tool_result(call.id, &result_str)?.into())
    }
}

/// JSON shape returned by a JS tool callback.
///
/// `content` carries the canonical [`meerkat_contracts::WireToolResultContent`]
/// typed owner — a `string | ContentBlock[]` union — so multimodal block
/// content (images, video) survives as typed blocks rather than being narrowed
/// to a string at the boundary.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Deserialize)]
struct JsToolResult {
    content: meerkat_contracts::WireToolResultContent,
    #[serde(default)]
    is_error: bool,
}

/// Parse a JS tool-callback JSON string into a typed [`meerkat_core::ToolResult`],
/// preserving multimodal block content via [`meerkat_contracts::WireToolResultContent`].
///
/// Not wasm-gated so the typed-content conversion is unit-testable on the host.
#[cfg(any(target_arch = "wasm32", test))]
fn parse_js_tool_result(
    call_id: &str,
    result_str: &str,
) -> Result<meerkat_core::ToolResult, meerkat_core::error::ToolError> {
    use meerkat_contracts::WireToolResultContent;

    let parsed: JsToolResult = serde_json::from_str(result_str).map_err(|e| {
        meerkat_core::error::ToolError::execution_failed(format!("invalid tool result JSON: {e}"))
    })?;

    match parsed.content {
        WireToolResultContent::Text(text) => Ok(meerkat_core::ToolResult::new(
            call_id.to_string(),
            text,
            parsed.is_error,
        )),
        WireToolResultContent::Blocks(blocks) => {
            let blocks = blocks
                .into_iter()
                .map(meerkat_core::ContentBlock::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    meerkat_core::error::ToolError::execution_failed(format!(
                        "invalid tool result content block: {e}"
                    ))
                })?;
            Ok(meerkat_core::ToolResult::with_blocks(
                call_id.to_string(),
                blocks,
                parsed.is_error,
            ))
        }
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

/// Primary bootstrap: trust-verify a mobpack and create service infrastructure.
///
/// `mobpack_bytes`: tar.gz mobpack archive. Verified via the canonical
/// [`meerkat_mob_pack::trust`] authority under [`TrustPolicy::Strict`] — an
/// unsigned or untrusted pack fails closed with a typed error.
/// `credentials_json`: `{ "anthropic_api_key"?: "...", "openai_api_key"?: "...",
/// "gemini_api_key"?: "...", "model"?: "<catalog default for configured provider>",
/// "max_sessions"?: 100000 }`
///
/// Stores an `EphemeralSessionService<FactoryAgentBuilder>` and a `MobMcpState`
/// in a `thread_local! RuntimeState` for subsequent mob/comms calls.
#[wasm_bindgen]
pub fn init_runtime(mobpack_bytes: &[u8], credentials_json: &str) -> Result<JsValue, JsValue> {
    init_tracing();
    patch_fetch_response_url();
    // Browser bootstrap must run canonical trust verification (Strict by
    // default), not a local parser. The verified pack is consumed below.
    let parsed = extract_verify_and_parse_mobpack(
        mobpack_bytes,
        meerkat_mob_pack::trust::TrustPolicy::Strict,
        &meerkat_mob_pack::trust::TrustedSigners::default(),
    )
    .map_err(js_from_value)?;
    let creds: Credentials =
        serde_json::from_str(credentials_json).map_err(|e| err_str("invalid_credentials", e))?;

    let api_keys = build_provider_api_keys(
        creds.anthropic_api_key.as_deref(),
        creds.openai_api_key.as_deref(),
        creds.gemini_api_key.as_deref(),
    )?;

    let max_sessions = creds.max_sessions;

    let providers: Vec<String> = api_keys.keys().cloned().collect();
    let base_urls = build_provider_base_urls(
        creds.anthropic_base_url.as_deref(),
        creds.openai_base_url.as_deref(),
        creds.gemini_base_url.as_deref(),
    );
    // The default model comes from the canonical
    // `resolve_create_session_default_model` ladder over the bootstrap
    // config, after the api-keys→realm population makes that config truthful
    // about which providers are configured.
    let (config, model) = build_bootstrap_config(creds.model, &api_keys, base_urls.as_ref());

    let (session_service, mob_state) = build_service_infrastructure(config, max_sessions)?;

    let bootstrap_mobpack = BootstrapMobpack::from_parsed(parsed);
    let mobpack_name = bootstrap_mobpack.name.clone();
    let skill_count = bootstrap_mobpack.skills.len();

    install_runtime_state(RuntimeState {
        mob_state,
        session_service,
        sessions: BTreeMap::new(),
        next_handle: 1,
        bootstrap_mobpack: Some(bootstrap_mobpack),
        #[cfg(target_arch = "wasm32")]
        js_tools: Vec::new(),
    });

    let result = serde_json::json!({
        "status": "initialized",
        "model": model,
        "providers": providers,
        "max_sessions": max_sessions,
        "mobpack": { "name": mobpack_name, "skills": skill_count },
    });
    Ok(JsValue::from_str(&result.to_string()))
}

// ═══════════════════════════════════════════════════════════
// Bootstrap: init_runtime_from_config
// ═══════════════════════════════════════════════════════════

/// Advanced bare-bones bootstrap without a mobpack.
///
/// `config_json`: `{ "anthropic_api_key"?: "...", "openai_api_key"?: "...",
/// "gemini_api_key"?: "...", "model"?: "<catalog default for configured provider>",
/// "max_sessions"?: 100000 }`
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

    let max_sessions = rt_config.max_sessions;

    let providers: Vec<String> = api_keys.keys().cloned().collect();
    let base_urls = build_provider_base_urls(
        rt_config.anthropic_base_url.as_deref(),
        rt_config.openai_base_url.as_deref(),
        rt_config.gemini_base_url.as_deref(),
    );
    // The default model comes from the canonical
    // `resolve_create_session_default_model` ladder over the bootstrap
    // config, after the api-keys→realm population makes that config truthful
    // about which providers are configured.
    let (config, model) = build_bootstrap_config(rt_config.model, &api_keys, base_urls.as_ref());

    let (session_service, mob_state) = build_service_infrastructure(config, max_sessions)?;

    install_runtime_state(RuntimeState {
        mob_state,
        session_service,
        sessions: BTreeMap::new(),
        next_handle: 1,
        bootstrap_mobpack: None,
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

fn build_session_request_with_auth_binding(
    auth_binding: Option<&meerkat_contracts::WireAuthBindingRef>,
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
    if let Some(conn) = auth_binding {
        build_config.auth_binding = Some(conn.clone().into());
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
    let model = config.model.clone();
    let request = build_session_request_with_auth_binding(
        config.auth_binding.as_ref(),
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
///
/// The pack's skills/definition become session prompt truth here, so this
/// ingress runs the same canonical [`meerkat_mob_pack::trust`] verification
/// as `init_runtime` (Strict, fail-closed) — never the local parse-only path.
#[wasm_bindgen]
pub fn create_session(mobpack_bytes: &[u8], config_json: &str) -> Result<u32, JsValue> {
    let parsed = extract_verify_and_parse_mobpack(
        mobpack_bytes,
        meerkat_mob_pack::trust::TrustPolicy::Strict,
        &meerkat_mob_pack::trust::TrustedSigners::default(),
    )
    .map_err(js_from_value)?;
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
///
/// When the runtime was bootstrapped from a trust-verified mobpack
/// (`init_runtime`), that pack's skills and definition are folded into the
/// system prompt so the verified bootstrap pack is actually consumed rather
/// than discarded.
#[wasm_bindgen]
pub fn create_session_simple(config_json: &str) -> Result<u32, JsValue> {
    let config: SessionConfig =
        serde_json::from_str(config_json).map_err(|e| err_str("invalid_config", e))?;
    let (system_prompt, mob_id) = with_runtime_state(|state| {
        Ok(match &state.bootstrap_mobpack {
            Some(pack) => (
                Some(pack.compile_system_prompt(config.system_prompt.as_deref())),
                pack.definition_id.clone(),
            ),
            None => (config.system_prompt.clone(), String::new()),
        })
    })?;
    create_runtime_backed_session(config, system_prompt, mob_id)
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
fn system_context_request_from_append(
    append: &meerkat_core::PendingSystemContextAppend,
) -> meerkat_core::AppendSystemContextRequest {
    meerkat_core::AppendSystemContextRequest {
        content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(append.text.clone()),
        source: append.source.clone(),
        idempotency_key: append.idempotency_key.clone(),
        source_kind: append.source_kind,
        peer_response_terminal: None,
    }
}

#[cfg(test)]
fn merge_runtime_system_context_state(
    mut agent_state: meerkat_core::SessionSystemContextState,
    starting_state: &meerkat_core::SessionSystemContextState,
    current_state: &meerkat_core::SessionSystemContextState,
) -> meerkat_core::SessionSystemContextState {
    let starting_state = starting_state
        .clone()
        .restore_from_snapshot()
        .expect("starting system-context state should restore");
    let current_state = current_state
        .clone()
        .restore_from_snapshot()
        .expect("current system-context state should restore");
    agent_state = agent_state
        .restore_from_snapshot()
        .expect("agent system-context state should restore");

    for applied in current_state.applied() {
        if !starting_state.applied().contains(applied) && !agent_state.applied().contains(applied) {
            let _ = agent_state.record_applied_blocks(std::slice::from_ref(applied), "");
        }
    }

    for pending in current_state.pending() {
        if !starting_state.pending().contains(pending) && !agent_state.pending().contains(pending) {
            let req = system_context_request_from_append(pending);
            let active_turn_scoped = pending
                .idempotency_key
                .as_ref()
                .is_some_and(|key| current_state.active_turn_pending_keys().contains(key));
            let result = if active_turn_scoped {
                agent_state.stage_active_turn_append(&req, pending.accepted_at)
            } else {
                agent_state.stage_append(&req, pending.accepted_at)
            };
            result.expect("merged system-context append should stage");
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
            .ok_or_else(|| err_invalid_session_handle(handle))?;
        Ok((state.session_service.clone(), session.session_id.clone()))
    })?;
    let status = session_service
        .append_system_context(
            &session_id,
            meerkat_core::AppendSystemContextRequest {
                content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(req.text),
                source: req.source,
                idempotency_key: req.idempotency_key,
                // JS-originated context appends are durable, never steers.
                source_kind: meerkat_core::session::SystemContextSource::Normal,
                peer_response_terminal: None,
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
/// On success, resolves (Ok) with a JSON-serialized [`meerkat_contracts::WireRunResult`]
/// — the same canonical wire shape RPC's `turn/start` returns — exposing
/// `terminal_cause_kind` so callers read the runtime-owned terminal class
/// instead of a fabricated status string.
///
/// On any [`meerkat_core::SessionError`] (including `Agent`), rejects (Err)
/// with the typed error envelope (`{ code, message }`), carrying the stable
/// `SessionError::code()`. Agent-level failures surface on the Err channel as
/// `AGENT_ERROR`; they are NOT laundered into an Ok value with a `status`
/// string.
#[wasm_bindgen]
pub async fn start_turn(handle: u32, prompt: &str) -> Result<JsValue, JsValue> {
    let (session_service, session_id) = with_runtime_state(|state| {
        let session = state
            .sessions
            .get(&handle)
            .ok_or_else(|| err_invalid_session_handle(handle))?;
        Ok((state.session_service.clone(), session.session_id.clone()))
    })?;

    // Parse the prompt as structured ContentInput (supports both plain strings
    // and JSON-serialized content blocks from the Web SDK). Structured-but-
    // malformed block JSON fails closed with INVALID_PARAMS instead of being
    // silently misread as plain text.
    let content_input = parse_prompt_content_input(prompt).map_err(js_from_value)?;

    let run_result = session_service
        .start_turn(
            &session_id,
            meerkat_core::service::StartTurnRequest {
                prompt: content_input,
                system_prompt: None,
                event_tx: None,
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
            },
        )
        .await;

    match run_result {
        Ok(result) => {
            let wire: meerkat_contracts::WireRunResult = result.into();
            let result_json =
                serde_json::to_string(&wire).map_err(|e| err_str("internal_error", e))?;
            Ok(JsValue::from_str(&result_json))
        }
        // Agent-level failures (LLM failure, timeout, budget) reject on the Err
        // channel carrying the typed `SessionError::code()`, mirroring how RPC's
        // `turn/start` surfaces terminal faults. No Ok-with-status laundering.
        Err(err) => Err(err_session(err)),
    }
}

/// Get current session state.
#[wasm_bindgen]
pub fn get_session_state(handle: u32) -> Result<String, JsValue> {
    let (session_service, session_id, mob_id) = with_runtime_state(|state| {
        let session = state
            .sessions
            .get(&handle)
            .ok_or_else(|| err_invalid_session_handle(handle))?;
        Ok((
            state.session_service.clone(),
            session.session_id.clone(),
            session.mob_id.clone(),
        ))
    })?;
    let view =
        futures::executor::block_on(session_service.read(&session_id)).map_err(err_session)?;
    let state = serde_json::json!({
        "handle": handle,
        "session_id": session_id.to_string(),
        "mob_id": mob_id,
        "model": view.state.model,
        "usage": view.billing.usage,
        "message_count": view.state.message_count,
        "is_active": view.state.is_active,
        "last_assistant_text": view.state.last_assistant_text,
    });
    Ok(state.to_string())
}

/// Inspect a mobpack without creating a session.
///
/// Read-only metadata projection: nothing from the pack is consumed as
/// runtime/prompt authority here, so (matching `rkat mob inspect`) unsigned
/// packs are inspectable and trust verification is deliberately not applied.
/// Every consuming ingress (`init_runtime`, `create_session`) verifies trust.
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
///
/// The browser-local handle is retired on success: subsequent
/// `get_session_state`/`start_turn`/`poll_events` on this handle fail closed
/// with `invalid_session_handle` rather than returning an addressable archived
/// projection. The handle is only removed after cleanup succeeds, so a failed
/// destroy leaves the handle live for retry.
#[wasm_bindgen]
pub fn destroy_session(handle: u32) -> Result<(), JsValue> {
    let (session_service, mob_state, session_id) = with_runtime_state(|state| {
        let session = state
            .sessions
            .get(&handle)
            .ok_or_else(|| err_invalid_session_handle(handle))?;
        Ok((
            state.session_service.clone(),
            state.mob_state.clone(),
            session.session_id.clone(),
        ))
    })?;
    futures::executor::block_on(destroy_session_with_services(
        session_service,
        mob_state,
        session_id,
    ))
    .map_err(err_web_destroy_session)?;
    // Fail-closed: retire the handle so it is no longer addressable.
    with_runtime_state_mut(|state| {
        state.sessions.remove(&handle);
        Ok(())
    })
}

#[derive(Debug)]
enum WebDestroySessionError {
    Session(meerkat_core::SessionError),
    Mob(MobMcpDestroyError),
}

fn err_web_destroy_session(error: WebDestroySessionError) -> JsValue {
    match error {
        WebDestroySessionError::Session(error) => err_session(error),
        WebDestroySessionError::Mob(error) => err_mob_destroy(error),
    }
}

async fn destroy_session_with_services(
    session_service: Arc<WasmSessionService>,
    mob_state: Arc<MobMcpState>,
    session_id: meerkat_core::SessionId,
) -> Result<(), WebDestroySessionError> {
    let archive_not_found = match session_service.archive(&session_id).await {
        Ok(()) => None,
        Err(error @ meerkat_core::SessionError::NotFound { .. }) => Some(error),
        Err(error) => return Err(WebDestroySessionError::Session(error)),
    };
    let retained_mob_cleanup = if archive_not_found.is_some() {
        mob_state
            .has_bridge_session_scoped_mobs(&session_id.to_string())
            .await
    } else {
        false
    };
    mob_state
        .destroy_bridge_session_mobs(&session_id.to_string())
        .await
        .map_err(WebDestroySessionError::Mob)?;
    if !retained_mob_cleanup && let Some(error) = archive_not_found {
        return Err(WebDestroySessionError::Session(error));
    }
    Ok(())
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
            .ok_or_else(|| err_invalid_session_handle(handle))?;
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
                    events.push(SubscriptionLaggedItem::new(skipped).to_value()?);
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
/// Returns JSON with the generated mob status plus the legacy state projection.
#[wasm_bindgen]
pub async fn mob_status(mob_id: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let state = mob_state.mob_status(&id).await.map_err(err_mob)?;
    let result = serde_json::json!({
        "mob_id": mob_id,
        "status": state.as_str(),
        "state": state.as_str(),
    });
    Ok(JsValue::from_str(&result.to_string()))
}

/// List all mobs.
///
/// Returns a generated `MobListResult` JSON envelope.
#[wasm_bindgen]
pub async fn mob_list() -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let mobs = mob_state.mob_list().await;
    let rows: Vec<serde_json::Value> = mobs
        .into_iter()
        .map(|(id, state)| {
            serde_json::json!({
                "mob_id": id.to_string(),
                "status": state.as_str(),
            })
        })
        .collect();
    let result = serde_json::json!({ "mobs": rows });
    let json = serde_json::to_string(&result).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
}

/// Perform a lifecycle action on a mob.
///
/// `action` is a compatibility string carrier that is immediately
/// deserialized into the typed wire lifecycle action contract.
///
/// Returns the canonical [`meerkat_contracts::MobLifecycleResult`] envelope as
/// JSON, mirroring the public MCP/RPC shape. For `destroy`, this carries the
/// structured `MobDestroyReport` (destroyed bridge sessions, retained mobs,
/// cleanup errors) instead of discarding it.
#[wasm_bindgen]
pub async fn mob_lifecycle(mob_id: &str, action: &str) -> Result<JsValue, JsValue> {
    let action = parse_mob_lifecycle_action_arg(action).map_err(|err| {
        err_js(
            "invalid_action",
            &format!("invalid lifecycle action: {err}"),
        )
    })?;
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    // `WireMobLifecycleAction` is `Copy`; reuse it for the result envelope.
    let destroy_report = mob_state
        .mob_lifecycle_action(&id, action)
        .await
        .map_err(err_mob_destroy)?;
    let destroy_report = destroy_report
        .map(|report| serde_json::to_value(&report).map_err(|e| err_str("serialize_error", e)))
        .transpose()?;
    let result = meerkat_contracts::MobLifecycleResult {
        mob_id: mob_id.to_string(),
        action,
        ok: true,
        destroy_report,
    };
    let json = serde_json::to_string(&result).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
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
/// `after_cursor` is carried as an opaque string so the full u64 runtime cursor
/// survives the JS boundary — JS numbers cannot represent all u64 values and the
/// prior `u32` parameter both truncated and (on the SDK side) silently rewound
/// the stream to position 0 for large/non-finite values. An empty string means
/// "from the start" (cursor 0); a non-numeric cursor fails closed.
#[wasm_bindgen]
pub async fn mob_events(mob_id: &str, after_cursor: &str, limit: u32) -> Result<JsValue, JsValue> {
    let after_cursor = parse_mob_event_cursor(after_cursor).map_err(js_from_value)?;
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let events = mob_state
        .mob_events(&id, after_cursor, limit as usize)
        .await
        .map_err(err_mob)?;
    let json = serde_json::to_string(&events).map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
}

/// Parse an opaque mob-event cursor string into the runtime's u64 cursor.
///
/// An empty/whitespace cursor maps to 0 (stream start). Any other value must be
/// a base-10 u64; a malformed cursor fails closed with `INVALID_PARAMS` rather
/// than silently rewinding the stream.
fn parse_mob_event_cursor(after_cursor: &str) -> Result<u64, serde_json::Value> {
    let trimmed = after_cursor.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    trimmed.parse::<u64>().map_err(|e| {
        err_value(
            "INVALID_PARAMS",
            format!("invalid mob event cursor '{after_cursor}': {e}"),
        )
    })
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
            Err(e) => meerkat_contracts::MobSpawnManyResultEntry::failed(e.cause(), e.to_string()),
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
    let serialized = serde_json::to_value(&members).map_err(|e| err_str("serialize_error", e))?;
    let mut array = match serialized {
        serde_json::Value::Array(entries) => entries,
        _ => {
            return Err(err_str(
                "serialize_error",
                "member roster must serialize to a JSON array",
            ));
        }
    };
    for (entry, member) in array.iter_mut().zip(members.iter()) {
        let obj = entry.as_object_mut().ok_or_else(|| {
            err_str(
                "serialize_error",
                "member roster entry must be a JSON object",
            )
        })?;
        obj.remove("agent_runtime_id");
        obj.remove("fence_token");
        let identity_str = member.agent_identity.to_string();
        let member_ref = serde_json::to_value(meerkat_contracts::WireMemberRef::encode(
            id.as_str(),
            &identity_str,
        ))
        .map_err(|e| err_str("serialize_error", e))?;
        obj.insert("member_ref".to_string(), member_ref);
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
                content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(req.text),
                source: req.source,
                idempotency_key: req.idempotency_key,
                // JS-originated context appends are durable, never steers.
                source_kind: meerkat_core::session::SystemContextSource::Normal,
                peer_response_terminal: None,
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

/// Legacy cross-mob bidirectional wire convenience.
///
/// No generated composition authority currently owns the two-mob transaction,
/// so this surface fails closed instead of hand-rolling partial rollback over
/// two independent MobMachine `WireExternalPeer` commands.
#[wasm_bindgen]
pub async fn wire_cross_mob(
    _mob_a: &str,
    _agent_a: &str,
    _mob_b: &str,
    _agent_b: &str,
) -> Result<(), JsValue> {
    Err(err_js(
        "wire_cross_mob_unsupported",
        "wire_cross_mob requires generated cross-mob composition authority",
    ))
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
    auth_binding: Option<meerkat_contracts::WireAuthBindingRef>,
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
    auth_binding: Option<meerkat_contracts::WireAuthBindingRef>,
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
        "mob_id": id.as_str(),
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
    // Project through the typed `MobMemberStatusResult` so the server-resolved
    // `member_ref` and the peer-connectivity tri-state travel in-band as typed
    // contract fields, rather than splicing `member_ref` into a free-form JSON
    // object out-of-band. Same opaque handle the spawn/respawn paths mint.
    let member_ref = meerkat_contracts::WireMemberRef::encode(id.as_str(), agent_identity);
    let result = snapshot
        .to_member_status_result(member_ref)
        .map_err(err_mob)?;
    let json = serde_json::to_string(&result).map_err(|e| err_str("serialize", e))?;
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
    let initial_message = initial_message
        .map(|message| parse_prompt_content_input(&message))
        .transpose()
        .map_err(js_from_value)?;
    match mob_state.mob_respawn(&id, mid, initial_message).await {
        Ok(receipt) => {
            let identity_str = receipt.identity.to_string();
            let result = serde_json::json!({
                "status": "completed",
                "receipt": {
                    "identity": identity_str,
                    "member_ref": meerkat_contracts::WireMemberRef::encode(
                        id.as_str(),
                        &identity_str,
                    ),
                },
            });
            Ok(JsValue::from_str(&result.to_string()))
        }
        Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
            receipt,
            failed_peer_ids,
        }) => {
            let identity_str = receipt.identity.to_string();
            let result = serde_json::json!({
                "status": "topology_restore_failed",
                "receipt": {
                    "identity": identity_str,
                    "member_ref": meerkat_contracts::WireMemberRef::encode(
                        id.as_str(),
                        &identity_str,
                    ),
                },
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
    // #115: the surface must not mint mob-member identity. A missing
    // `agent_identity` fails closed rather than fabricating a synthetic
    // `helper-{uuid}` on the runtime identity path — identity allocation is
    // the mob substrate's responsibility.
    let Some(agent_identity) = request.agent_identity else {
        return Err(err_str(
            "invalid_request",
            "mob_spawn_helper requires agent_identity; the surface does not allocate member identity",
        ));
    };
    let identity = AgentIdentity::from(agent_identity);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role_name) = request.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role_name));
    }
    if let Some(auth_binding) = request.auth_binding {
        options.auth_binding = Some(auth_binding.into());
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
    // #115: the surface must not mint mob-member identity (no synthetic
    // `fork-{uuid}` fallback).
    let Some(agent_identity) = request.agent_identity else {
        return Err(err_str(
            "invalid_request",
            "mob_fork_helper requires agent_identity; the surface does not allocate member identity",
        ));
    };
    let identity = AgentIdentity::from(agent_identity);
    let fork_context = request
        .fork_context
        .unwrap_or(meerkat_mob::ForkContext::FullHistory);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role_name) = request.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role_name));
    }
    if let Some(auth_binding) = request.auth_binding {
        options.auth_binding = Some(auth_binding.into());
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
/// Returns a generated `MobFlowStatusResult` JSON envelope whose `run` field
/// carries run state and ledgers, or null when not found.
#[wasm_bindgen]
pub async fn mob_flow_status(mob_id: &str, run_id: &str) -> Result<JsValue, JsValue> {
    let mob_state = with_mob_state(Ok)?;
    let id = MobId::from(mob_id);
    let rid: RunId = run_id
        .parse()
        .map_err(|e| err_str("invalid_run_id", format!("{e}")))?;
    let status = mob_state.mob_flow_status(&id, rid).await.map_err(err_mob)?;
    let json = serde_json::to_string(&serde_json::json!({ "run": status }))
        .map_err(|e| err_str("serialize_error", e))?;
    Ok(JsValue::from_str(&json))
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
                            events.push(SubscriptionLaggedItem::new(n).to_value()?);
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

/// Typed lag-fault sentinel for the subscription/poll stream.
///
/// Rule 4 / Rule 9: the lag fault is a typed item, not an ad-hoc
/// `json!({type:'lagged',skipped:n})` literal duplicated at every poll site. A
/// single typed owner serializes the `{ type: "lagged", skipped: <u64> }`
/// schema consumed by `@rkat/web`'s `SubscriptionLaggedEvent`.
///
/// NOTE: the fully-generated cross-language contract (typed `EventEnvelope` +
/// `Lagged` variant emitted from `meerkat-contracts` through SDK codegen) is the
/// canonical destination per remediation row #204; this in-lane struct removes
/// the dual hand-modeling on the Rust side until that contract lands.
#[derive(Debug, Clone, Copy, Serialize)]
struct SubscriptionLaggedItem {
    #[serde(rename = "type")]
    kind: SubscriptionLaggedKind,
    skipped: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum SubscriptionLaggedKind {
    Lagged,
}

impl SubscriptionLaggedItem {
    fn new(skipped: u64) -> Self {
        Self {
            kind: SubscriptionLaggedKind::Lagged,
            skipped,
        }
    }

    /// Serialize the typed sentinel into the stream's `serde_json::Value`.
    fn to_value(self) -> Result<serde_json::Value, JsValue> {
        serde_json::to_value(self).map_err(|e| err_str("serialize_error", e))
    }
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
    use super::{Credentials, build_bootstrap_config, extract_verify_and_parse_mobpack};
    use super::{
        EventSubscription, SUBSCRIPTIONS, SubscriptionInner, SubscriptionLaggedItem,
        close_subscription, destroy_session_with_services, merge_runtime_system_context_state,
        mob_destroy_error_value, parse_js_tool_result, parse_mob_event_cursor,
        parse_mob_lifecycle_action_arg, parse_mobpack, parse_prompt_content_input,
        poll_subscription, serialize_subscription_item, session_error_envelope,
    };
    #[cfg(target_arch = "wasm32")]
    use super::{
        append_system_context, clear_tool_callbacks, create_session_simple, destroy_session,
        get_session_state, init_runtime_from_config, register_js_tool,
    };
    #[cfg(not(target_arch = "wasm32"))]
    use super::{build_service_infrastructure, populate_realm_from_api_keys};
    #[cfg(not(target_arch = "wasm32"))]
    use super::{helper_result_payload, spawn_member_result_payload};
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat::{SessionService, SessionServiceControlExt};
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat_core::Config;
    use meerkat_core::time_compat::{Duration, SystemTime};
    use meerkat_core::{
        PendingSystemContextAppend, SeenSystemContextState, SessionSystemContextState,
    };
    #[cfg(not(target_arch = "wasm32"))]
    use meerkat_mob::{MobId, SpawnMemberSpec};
    use serde_json::json;
    #[cfg(not(target_arch = "wasm32"))]
    use std::collections::HashMap;
    #[cfg(not(target_arch = "wasm32"))]
    use std::sync::Arc;

    fn request_from_append(
        append: &PendingSystemContextAppend,
    ) -> meerkat_core::AppendSystemContextRequest {
        meerkat_core::AppendSystemContextRequest {
            content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                append.text.clone(),
            ),
            source: append.source.clone(),
            idempotency_key: append.idempotency_key.clone(),
            source_kind: append.source_kind,
            peer_response_terminal: None,
        }
    }

    fn system_context_state_with_pending(
        appends: &[PendingSystemContextAppend],
        active_turn_keys: &[&str],
    ) -> SessionSystemContextState {
        let mut state = SessionSystemContextState::default();
        for append in appends {
            let req = request_from_append(append);
            let active_turn_scoped = append
                .idempotency_key
                .as_deref()
                .is_some_and(|key| active_turn_keys.contains(&key));
            let result = if active_turn_scoped {
                state.stage_active_turn_append(&req, append.accepted_at)
            } else {
                state.stage_append(&req, append.accepted_at)
            };
            result.expect("test append should stage");
        }
        state
    }

    fn system_context_state_with_applied(
        appends: &[PendingSystemContextAppend],
    ) -> SessionSystemContextState {
        let mut state = system_context_state_with_pending(appends, &[]);
        state.mark_pending_applied();
        state
    }

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
    fn test_auth_binding() -> meerkat_core::AuthBindingRef {
        meerkat_core::AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("default").expect("test realm id"),
            binding: meerkat_core::connection::BindingId::parse("default_anthropic")
                .expect("test binding id"),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn populate_realm_applies_base_url_by_typed_provider() {
        let mut config = Config::default();
        let api_keys = HashMap::from([
            ("anthropic".to_string(), "sk-ant".to_string()),
            ("openai".to_string(), "sk-oai".to_string()),
        ]);
        let base_urls = HashMap::from([(
            "openai".to_string(),
            "https://proxy.example/openai".to_string(),
        )]);
        populate_realm_from_api_keys(&mut config, &api_keys, Some(&base_urls));

        let section = config.realm.get("default").expect("default realm");
        // base_url landed on the openai backend, matched by typed provider
        // identity, not by reconstructing a `default_openai` id name.
        let openai_backend = section
            .backend
            .values()
            .find(|bp| bp.provider == "openai")
            .expect("openai backend");
        assert_eq!(
            openai_backend.base_url.as_deref(),
            Some("https://proxy.example/openai")
        );
        let anthropic_backend = section
            .backend
            .values()
            .find(|bp| bp.provider == "anthropic")
            .expect("anthropic backend");
        assert_eq!(anthropic_backend.base_url, None);
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

    #[cfg(not(target_arch = "wasm32"))]
    async fn insert_web_partial_destroy_mob(
        mob_state: &Arc<meerkat_mob_mcp::MobMcpState>,
        owner_session_id: &str,
        events: Arc<meerkat_mob::store::InMemoryMobEventStore>,
    ) -> MobId {
        let mob_id = MobId::from("web-session-destroy-partial");
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        let owner_session_id = meerkat_core::types::SessionId::parse(owner_session_id)
            .expect("valid owner bridge session id");
        let storage = meerkat_mob::MobStorage::with_events(events);
        let handle = meerkat_mob::MobBuilder::new(definition, storage)
            .with_owner_bridge_session_create_authority(owner_session_id, true, false)
            .with_session_service(mob_state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create archive-owned mob with failing event clear");
        mob_state.mob_insert_handle(mob_id.clone(), handle).await;
        mob_id
    }

    #[test]
    fn merge_runtime_system_context_state_preserves_concurrent_appends() {
        let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

        let initial_pending = PendingSystemContextAppend {
            text: "initial".to_string(),
            source: Some("mob".to_string()),
            idempotency_key: Some("ctx-initial".to_string()),
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            accepted_at: base_time,
            peer_response_terminal: None,
        };
        let concurrent_pending = PendingSystemContextAppend {
            text: "concurrent".to_string(),
            source: Some("mob".to_string()),
            idempotency_key: Some("ctx-concurrent".to_string()),
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            accepted_at: base_time + Duration::from_secs(1),
            peer_response_terminal: None,
        };

        let starting_state =
            system_context_state_with_pending(std::slice::from_ref(&initial_pending), &[]);
        let agent_state = system_context_state_with_applied(std::slice::from_ref(&initial_pending));
        let current_registry_state = system_context_state_with_pending(
            &[initial_pending, concurrent_pending.clone()],
            &["ctx-concurrent"],
        );

        let merged = merge_runtime_system_context_state(
            agent_state,
            &starting_state,
            &current_registry_state,
        );

        assert_eq!(merged.pending(), std::slice::from_ref(&concurrent_pending));
        assert_eq!(
            merged.seen().get("ctx-initial").map(|seen| seen.state),
            Some(SeenSystemContextState::Applied)
        );
        assert_eq!(
            merged.seen().get("ctx-concurrent").map(|seen| seen.state),
            Some(SeenSystemContextState::Pending)
        );
        assert!(merged.active_turn_pending_keys().contains("ctx-concurrent"));
    }

    #[test]
    fn parse_mobpack_accepts_browser_safe_typed_capabilities() {
        let bytes = test_mobpack_bytes(&["sessions", "comms", "vendor.custom"]);

        let parsed = parse_mobpack(&bytes).expect("browser-safe mobpack should parse");

        let tokens: Vec<String> = parsed
            .manifest
            .requires
            .as_ref()
            .expect("requires section")
            .capabilities
            .iter()
            .map(|c| c.token().to_string())
            .collect();
        assert_eq!(
            tokens,
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
                    "model": "claude-opus-4-8",
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
                        .with_auth_binding(test_auth_binding())
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
                    content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(
                        "Prioritize coordinating with the lead.".to_string(),
                    ),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-worker-1".to_string()),
                    source_kind: meerkat_core::session::SystemContextSource::Normal,
                    peer_response_terminal: None,
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

        assert_eq!(system_context_state.pending_len(), 1);
        assert_eq!(
            system_context_state.pending()[0].idempotency_key.as_deref(),
            Some("ctx-worker-1")
        );
        assert_eq!(
            system_context_state.pending()[0].text,
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
    fn spawn_member_failure_payload_uses_typed_result_cause() {
        let entry = meerkat_contracts::MobSpawnManyResultEntry::failed(
            meerkat_contracts::MobSpawnManyFailureCause::ProfileNotFound,
            "profile missing",
        );
        let payload = serde_json::to_value(&entry).expect("serialize failed spawn_many result");

        assert_eq!(payload["status"], "failed");
        assert_eq!(payload["result"]["cause"], "profile_not_found");
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
                "lead": { "model": "claude-opus-4-8", "tools": { "mob": true, "comms": true } },
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
        options.auth_binding = Some(test_auth_binding());

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

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn mob_destroy_error_value_preserves_typed_incomplete_data() {
        let mut report = meerkat_mob::MobDestroyReport::default();
        report.errors.push("forced incomplete cleanup".to_string());

        let value =
            mob_destroy_error_value(meerkat_mob_mcp::MobMcpDestroyError::Incomplete { report });

        assert_eq!(value["code"], "mob_destroy_incomplete");
        assert_eq!(value["retryable"], true);
        assert!(
            value["destroy_report"]["errors"]
                .as_array()
                .is_some_and(|errors| !errors.is_empty()),
            "web destroy error projection must carry the destroy report: {value}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test(flavor = "current_thread")]
    async fn destroy_session_without_retained_mob_cleanup_returns_not_found_on_retry() {
        let mut config = Config::default();
        populate_realm_from_api_keys(
            &mut config,
            &HashMap::from([("anthropic".to_string(), "sk-test".to_string())]),
            None,
        );
        let (service, mob_state) =
            build_service_infrastructure(config, 8).expect("build runtime services");
        let created = service
            .create_session(meerkat_core::service::CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "".into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(32),
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create deferred web session");
        let session_id = created.session_id;

        destroy_session_with_services(service.clone(), mob_state.clone(), session_id.clone())
            .await
            .expect("first destroy without mob cleanup should archive session");
        let retry = destroy_session_with_services(service, mob_state, session_id)
            .await
            .expect_err("stale destroy without retained mob cleanup should stay NotFound");
        assert!(
            matches!(
                retry,
                super::WebDestroySessionError::Session(meerkat_core::SessionError::NotFound { .. })
            ),
            "retry without retained mob cleanup should not be success-classified: {retry:?}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test(flavor = "current_thread")]
    async fn destroy_session_cleanup_retry_surfaces_retained_mob_partial_state() {
        let mut config = Config::default();
        populate_realm_from_api_keys(
            &mut config,
            &HashMap::from([("anthropic".to_string(), "sk-test".to_string())]),
            None,
        );
        let (service, mob_state) =
            build_service_infrastructure(config, 8).expect("build runtime services");
        let created = service
            .create_session(meerkat_core::service::CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "".into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(32),
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            })
            .await
            .expect("create deferred web session");
        let session_id = created.session_id;
        let events = Arc::new(meerkat_mob::store::InMemoryMobEventStore::new());
        events.fail_clear_until_allowed();
        let mob_id =
            insert_web_partial_destroy_mob(&mob_state, &session_id.to_string(), events.clone())
                .await;

        let first =
            destroy_session_with_services(service.clone(), mob_state.clone(), session_id.clone())
                .await
                .expect_err("first cleanup should surface incomplete mob destroy");
        assert!(
            matches!(
                first,
                super::WebDestroySessionError::Mob(
                    meerkat_mob_mcp::MobMcpDestroyError::Incomplete { .. }
                )
            ),
            "web destroy should surface typed partial cleanup, got unexpected error"
        );

        let retry =
            destroy_session_with_services(service.clone(), mob_state.clone(), session_id.clone())
                .await
                .expect_err(
                    "retry should report retained mob cleanup state, not session not found",
                );
        assert!(
            matches!(
                retry,
                super::WebDestroySessionError::Mob(
                    meerkat_mob_mcp::MobMcpDestroyError::Incomplete { .. }
                )
            ),
            "web destroy retry should keep using retained mob cleanup authority"
        );

        events.allow_clear();
        destroy_session_with_services(service, mob_state.clone(), session_id)
            .await
            .expect("retry after event store recovery should complete cleanup");
        assert!(
            mob_state.handle_for(&mob_id).await.is_err(),
            "complete web destroy retry must remove the mob retry anchor"
        );
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
    fn destroy_session_retires_handle_and_fails_closed() {
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

        // Row #215: a destroyed handle is retired, not left as an addressable
        // archived projection. Subsequent reads fail closed with the typed
        // invalid_session_handle code.
        let after = get_session_state(handle)
            .expect_err("destroyed handle must fail closed, not return archived JSON");
        let after: serde_json::Value =
            serde_json::from_str(&after.as_string().expect("typed error envelope string"))
                .expect("typed error envelope json");
        assert_eq!(after["code"], "invalid_session_handle");

        // Re-destroy is idempotent at the typed-code level (still invalid_session_handle).
        let redestroy = destroy_session(handle)
            .expect_err("re-destroying a retired handle must surface invalid_session_handle");
        let redestroy: serde_json::Value =
            serde_json::from_str(&redestroy.as_string().expect("typed error envelope string"))
                .expect("typed error envelope json");
        assert_eq!(redestroy["code"], "invalid_session_handle");
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

    // ── Row #39: content-bearing prompt parsing fails closed ───────────────

    #[test]
    fn parse_prompt_content_input_plain_string_is_text() {
        let parsed = parse_prompt_content_input("hello world").expect("plain text parses");
        assert_eq!(
            parsed,
            meerkat_core::types::ContentInput::Text("hello world".to_string())
        );
    }

    #[test]
    fn parse_prompt_content_input_valid_blocks_parse_to_blocks() {
        let blocks_json = json!([{ "type": "text", "text": "hi" }]).to_string();
        let parsed = parse_prompt_content_input(&blocks_json).expect("valid blocks parse");
        let blocks = match parsed {
            meerkat_core::types::ContentInput::Blocks(blocks) => blocks,
            meerkat_core::types::ContentInput::Text(text) => {
                unreachable!("expected blocks, got text: {text}")
            }
        };
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn parse_prompt_content_input_malformed_blocks_fail_closed() {
        // Block-shaped JSON (an array) that is NOT a valid ContentInput must
        // error with INVALID_PARAMS, never be misread as a single text block.
        let malformed = json!([{ "type": "image", "media_type": 12345 }]).to_string();
        let err = parse_prompt_content_input(&malformed)
            .expect_err("malformed block JSON must fail closed");
        assert_eq!(err["code"], "INVALID_PARAMS");
    }

    #[test]
    fn parse_prompt_content_input_non_array_json_is_text() {
        // A bare number/boolean is not block-shaped content; it is plain text.
        let parsed = parse_prompt_content_input("42").expect("number text parses");
        assert_eq!(
            parsed,
            meerkat_core::types::ContentInput::Text("42".to_string())
        );
    }

    // ── Row #40: provider-aware default model resolution ───────────────────
    //
    // The bootstrap default model is the OUTPUT of the canonical
    // `meerkat::resolve_create_session_default_model` ladder over the
    // bootstrap config; the surface keeps no parallel ladder. Each test pins
    // both the expected value AND that the value is exactly the canonical
    // ladder over the produced config.

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bootstrap_default_model_for_only_openai_is_openai_catalog_default() {
        let keys = HashMap::from([("openai".to_string(), "sk-oai".to_string())]);
        let (config, model) = build_bootstrap_config(None, &keys, None);
        let expected =
            meerkat_core::model_profile::catalog::default_model(meerkat_core::Provider::OpenAI)
                .expect("openai catalog default")
                .to_string();
        assert_eq!(model, expected);
        // Must NOT hardcode the anthropic global default when anthropic is absent.
        assert_ne!(
            model,
            meerkat_core::model_profile::catalog::global_default_model()
        );
        assert_eq!(
            model,
            meerkat::resolve_create_session_default_model(&config)
        );
        assert_eq!(config.agent.model, model);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bootstrap_default_model_prefers_highest_priority_configured_provider() {
        // openai + gemini configured (no anthropic): priority picks openai.
        let keys = HashMap::from([
            ("gemini".to_string(), "g".to_string()),
            ("openai".to_string(), "o".to_string()),
        ]);
        let (config, model) = build_bootstrap_config(None, &keys, None);
        assert_eq!(
            model,
            meerkat_core::model_profile::catalog::default_model(meerkat_core::Provider::OpenAI)
                .expect("openai catalog default")
        );
        assert_eq!(
            model,
            meerkat::resolve_create_session_default_model(&config)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bootstrap_default_model_unrecognized_key_falls_back_to_global_default() {
        let keys = HashMap::from([("notaprovider".to_string(), "x".to_string())]);
        let (config, model) = build_bootstrap_config(None, &keys, None);
        assert_eq!(
            model,
            meerkat_core::model_profile::catalog::global_default_model()
        );
        assert_eq!(
            model,
            meerkat::resolve_create_session_default_model(&config)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bootstrap_explicit_model_outranks_configured_provider_defaults() {
        // An explicit caller model is the bootstrap config's global agent
        // default — ladder tier 1 — and outranks every provider default.
        let keys = HashMap::from([("openai".to_string(), "sk-oai".to_string())]);
        let (config, model) = build_bootstrap_config(Some("custom-model".to_string()), &keys, None);
        assert_eq!(model, "custom-model");
        assert_eq!(config.agent.model, "custom-model");
        assert_eq!(
            model,
            meerkat::resolve_create_session_default_model(&config)
        );
    }

    // ── Row #60: realm binding order pinned to catalog provider_priority ────

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn default_realm_binding_order_equals_provider_priority() {
        // Insertion order intentionally reversed vs catalog priority to prove
        // the surface sorts by provider_priority(), not by HashMap iteration.
        let mut config = Config::default();
        let api_keys = HashMap::from([
            ("gemini".to_string(), "g".to_string()),
            ("openai".to_string(), "o".to_string()),
            ("anthropic".to_string(), "a".to_string()),
        ]);
        populate_realm_from_api_keys(&mut config, &api_keys, None);
        let section = config.realm.get("default").expect("default realm");

        // The highest-priority configured provider (anthropic) seeds the
        // per-realm default_binding.
        let top = meerkat_core::model_profile::catalog::provider_priority()
            .first()
            .expect("at least one provider in priority");
        let expected_binding = format!("default_{}", top.as_str());
        assert_eq!(
            section.default_binding.as_deref(),
            Some(expected_binding.as_str())
        );

        // Each backend's provider identity round-trips through Provider::parse_strict.
        for backend in section.backend.values() {
            assert!(
                meerkat_core::Provider::parse_strict(&backend.provider).is_some(),
                "backend provider '{}' must be a typed catalog identity",
                backend.provider
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn unrecognized_realm_key_sorts_last_and_does_not_seed_default() {
        let mut config = Config::default();
        let api_keys = HashMap::from([
            ("notaprovider".to_string(), "x".to_string()),
            ("anthropic".to_string(), "a".to_string()),
        ]);
        populate_realm_from_api_keys(&mut config, &api_keys, None);
        let section = config.realm.get("default").expect("default realm");
        // A recognized provider (anthropic) wins the default over the
        // unrecognized key, which sorts last.
        assert_eq!(
            section.default_binding.as_deref(),
            Some("default_anthropic")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn only_openai_seeds_single_openai_binding_via_catalog_priority() {
        // With only openai configured, the per-realm default binding is the
        // openai binding (its catalog-priority position is irrelevant when it
        // is the sole provider — it is the only entry, so it seeds the default).
        // No binding for any other provider is invented.
        let mut config = Config::default();
        let api_keys = HashMap::from([("openai".to_string(), "sk-oai".to_string())]);
        populate_realm_from_api_keys(&mut config, &api_keys, None);
        let section = config.realm.get("default").expect("default realm");

        // Exactly one backend, whose provider is the typed `openai` identity
        // (round-trips through Provider::parse_strict — no stringly invention).
        assert_eq!(section.backend.len(), 1);
        let backend = section.backend.values().next().expect("openai backend");
        assert_eq!(backend.provider, "openai");
        assert_eq!(
            meerkat_core::Provider::parse_strict(&backend.provider),
            Some(meerkat_core::Provider::OpenAI),
        );

        // The single configured provider seeds the per-realm default binding,
        // and no anthropic/gemini binding is conjured into existence.
        assert_eq!(section.default_binding.as_deref(), Some("default_openai"));
        assert!(!section.backend.contains_key("default_anthropic"));
        assert!(!section.backend.contains_key("default_gemini"));

        // The default binding string is owned by the catalog provider list:
        // its `default_<provider>` form names a provider that is itself a typed
        // catalog identity, never an arbitrary key.
        let default_provider = section
            .default_binding
            .as_deref()
            .and_then(|id| id.strip_prefix("default_"))
            .expect("default binding uses default_<provider> form");
        assert!(
            meerkat_core::model_profile::catalog::provider_priority()
                .iter()
                .any(|p| p.as_str() == default_provider),
            "default binding provider '{default_provider}' must be catalog-owned"
        );
    }

    // ── Row #186: Credentials carries max_sessions ─────────────────────────

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn credentials_deserialize_threads_max_sessions() {
        let creds: Credentials = serde_json::from_str(
            &json!({ "anthropic_api_key": "sk-test", "max_sessions": 7 }).to_string(),
        )
        .expect("credentials parse");
        assert_eq!(creds.max_sessions, 7);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn credentials_max_sessions_defaults_to_runtime_constant() {
        let creds: Credentials =
            serde_json::from_str(&json!({ "anthropic_api_key": "sk-test" }).to_string())
                .expect("credentials parse");
        assert_eq!(creds.max_sessions, super::MAX_SESSIONS);
    }

    // ── Row #163: mobpack init runs canonical trust verification ───────────

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn unsigned_mobpack_fails_closed_under_strict_policy() {
        // test_mobpack_bytes produces an unsigned pack (no signature.toml).
        let bytes = test_mobpack_bytes(&["sessions"]);
        let err = extract_verify_and_parse_mobpack(
            &bytes,
            meerkat_mob_pack::trust::TrustPolicy::Strict,
            &meerkat_mob_pack::trust::TrustedSigners::default(),
        )
        .expect_err("unsigned pack must fail closed under Strict policy");
        assert_eq!(err["code"], "untrusted_mobpack");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn unsigned_mobpack_accepted_under_permissive_and_definition_consumed() {
        let bytes = test_mobpack_bytes(&["sessions"]);
        let parsed = extract_verify_and_parse_mobpack(
            &bytes,
            meerkat_mob_pack::trust::TrustPolicy::Permissive,
            &meerkat_mob_pack::trust::TrustedSigners::default(),
        )
        .expect("permissive policy accepts unsigned pack");
        // The verified definition is consumed, not discarded.
        assert_eq!(parsed.definition.id, "browser-test");
    }

    /// Build a trusted, signer-registered mobpack via the canonical
    /// `meerkat_mob_pack::pack` signing authority and return both the signed
    /// archive bytes and a `TrustedSigners` that recognizes the embedded key.
    ///
    /// The pack is signed through `pack_directory`'s real signing path (key file
    /// → Ed25519 signature over the canonical digest), so the produced archive
    /// is verifiable under `TrustPolicy::Strict`. The embedded public key is read
    /// back out of the produced `signature.toml` to seed the trust store, which
    /// keeps the test free of a direct `ed25519_dalek` dependency.
    #[cfg(not(target_arch = "wasm32"))]
    fn trusted_signed_mobpack() -> (Vec<u8>, meerkat_mob_pack::trust::TrustedSigners) {
        use std::str::FromStr;

        // Unique on-disk pack directory (no `tempfile` dep needed in this crate).
        let dir = std::env::temp_dir().join(format!(
            "rkat-web-runtime-signed-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create pack dir");

        // Minimal valid pack: manifest (name + version, no [trust]) and a
        // definition whose `id` the browser header reads back.
        std::fs::write(
            dir.join("manifest.toml"),
            b"[mobpack]\nname = \"browser-test\"\nversion = \"1.0.0\"\n",
        )
        .expect("write manifest");
        std::fs::write(dir.join("definition.json"), br#"{"id":"browser-test"}"#)
            .expect("write definition");

        // 32-byte Ed25519 secret as 64 hex chars (the signing key file format).
        let key_path = dir.join("signing.key");
        std::fs::write(
            &key_path,
            "0909090909090909090909090909090909090909090909090909090909090909",
        )
        .expect("write signing key");

        let packed = meerkat_mob_pack::pack::pack_directory(
            &dir,
            Some(meerkat_mob_pack::pack::SigningRequest {
                signer_id: "ci",
                key_path: &key_path,
            }),
        )
        .expect("sign pack");

        // Read the embedded public key back out of the produced signature so the
        // trust store recognizes the very signer that signed this pack.
        let files = super::extract_targz_safe(&packed.archive_bytes).expect("extract signed pack");
        let signature_bytes = files.get("signature.toml").expect("signature.toml present");
        let signature: meerkat_mob_pack::signing::PackSignature =
            toml::from_str(std::str::from_utf8(signature_bytes).expect("signature utf8"))
                .expect("parse signature");

        let trusted_key = meerkat_mob_pack::vocabulary::Ed25519PublicKeyHex::from_str(
            signature.public_key.as_hex(),
        )
        .expect("public key hex");
        let signer_id = meerkat_mob_pack::vocabulary::SignerId::from_str("ci").expect("signer id");

        let mut trusted = meerkat_mob_pack::trust::TrustedSigners::default();
        trusted.signers.insert(signer_id, trusted_key);

        std::fs::remove_dir_all(&dir).ok();
        (packed.archive_bytes, trusted)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn trusted_signed_mobpack_succeeds_under_strict_and_is_consumed() {
        let (archive, trusted) = trusted_signed_mobpack();
        let parsed = extract_verify_and_parse_mobpack(
            &archive,
            meerkat_mob_pack::trust::TrustPolicy::Strict,
            &trusted,
        )
        .expect("trusted-signed pack must pass Strict verification");
        // The verified pack is threaded forward (consumed), not dropped: the
        // definition id and manifest name survive into the runtime bootstrap
        // material via `BootstrapMobpack::from_parsed`.
        assert_eq!(parsed.definition.id, "browser-test");
        let bootstrap = super::BootstrapMobpack::from_parsed(parsed);
        assert_eq!(bootstrap.name, "browser-test");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn signed_mobpack_with_untrusted_signer_fails_closed_under_strict() {
        // The pack is validly signed but the signer is NOT in the trust store;
        // Strict policy must reject it (fail closed), proving signing alone is
        // insufficient — trust is required.
        let (archive, _trusted) = trusted_signed_mobpack();
        let err = extract_verify_and_parse_mobpack(
            &archive,
            meerkat_mob_pack::trust::TrustPolicy::Strict,
            &meerkat_mob_pack::trust::TrustedSigners::default(),
        )
        .expect_err("unknown signer must fail closed under Strict");
        assert_eq!(err["code"], "untrusted_mobpack");
    }

    // ── Row #204: lagged sentinel serializes to the typed schema ───────────

    #[test]
    fn lagged_sentinel_serializes_to_typed_schema() {
        // A cursor beyond u32::MAX proves the skipped count is carried as u64.
        let skipped = u64::from(u32::MAX) + 5;
        let value = SubscriptionLaggedItem::new(skipped)
            .to_value()
            .expect("lagged sentinel serializes");
        assert_eq!(value["type"], "lagged");
        assert_eq!(value["skipped"].as_u64(), Some(skipped));
    }

    // ── Row #217: mob event cursor round-trips u64 / fails closed ──────────

    #[test]
    fn mob_event_cursor_round_trips_beyond_u32() {
        let cursor = u64::from(u32::MAX) + 17;
        let parsed = parse_mob_event_cursor(&cursor.to_string()).expect("large cursor parses");
        assert_eq!(parsed, cursor);
    }

    #[test]
    fn mob_event_cursor_empty_is_stream_start() {
        assert_eq!(parse_mob_event_cursor("").expect("empty cursor"), 0);
        assert_eq!(parse_mob_event_cursor("   ").expect("blank cursor"), 0);
    }

    #[test]
    fn mob_event_cursor_malformed_fails_closed() {
        let err = parse_mob_event_cursor("not-a-number")
            .expect_err("non-numeric cursor must fail closed");
        assert_eq!(err["code"], "INVALID_PARAMS");
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
        tx.send(meerkat_core::EventEnvelope::new_session(
            session_id.clone(),
            1,
            None,
            meerkat_core::AgentEvent::TextDelta {
                delta: "first".to_string(),
            },
        ))
        .unwrap_or(0);
        tx.send(meerkat_core::EventEnvelope::new_session(
            session_id,
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

    // ── ROW #83: WASM direct turn must NOT launder agent faults into an Ok
    // status string. The Err channel (built by `session_error_envelope`, which
    // `err_session`/`start_turn` use) carries the typed `SessionError::code()`.
    #[test]
    fn start_turn_agent_error_carries_typed_code_on_err_channel() {
        let err = meerkat_core::SessionError::Agent(meerkat_core::error::AgentError::ToolError(
            "boom".to_string(),
        ));
        // The typed code that `start_turn`'s Err arm now surfaces.
        assert_eq!(err.code(), "AGENT_ERROR");

        // The Err envelope must carry that typed code — not a fabricated
        // "completed"/"failed" status string and not a flattened
        // "internal_error".
        let envelope = session_error_envelope(err);
        assert_eq!(envelope["code"], "AGENT_ERROR");
        assert_ne!(
            envelope["code"], "internal_error",
            "agent fault must not be flattened into a generic internal_error code"
        );
        assert!(
            envelope.get("status").is_none(),
            "agent fault must not be laundered into an in-band status string"
        );
    }

    // ── ROW #83: success path returns the canonical WireRunResult — exposing
    // `terminal_cause_kind` — with no fabricated in-band `status` string.
    #[test]
    fn start_turn_success_exposes_terminal_cause_kind_no_status_string() {
        let run = meerkat_core::RunResult {
            text: "hello".to_string(),
            session_id: meerkat_core::SessionId::new(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        };
        let wire: meerkat_contracts::WireRunResult = run.into();
        let json = serde_json::to_value(&wire).expect("wire result should serialize");
        // Canonical terminal-class field is present (None serializes as absent,
        // but the field is the typed owner, not a hand-rolled status literal).
        assert!(
            json.get("status").is_none(),
            "success path must not fabricate an in-band status string"
        );
        assert_eq!(json["text"], "hello");
        // terminal_cause_kind is the runtime-owned terminal class surfaced to JS.
        assert!(wire.terminal_cause_kind.is_none());
    }

    // ── ROW #138: JS tool callback returning block content must reach
    // `ToolResult` as typed multimodal blocks, NOT a stringified scalar.
    #[test]
    fn js_tool_callback_block_content_reaches_tool_result_as_typed_blocks() {
        let result_json = serde_json::json!({
            "content": [
                { "type": "text", "text": "see image" },
                { "type": "image", "media_type": "image/png", "source": "inline", "data": "QUJD" }
            ],
            "is_error": false
        })
        .to_string();

        let tool_result =
            parse_js_tool_result("call_1", &result_json).expect("block content should parse");

        assert_eq!(tool_result.tool_use_id, "call_1");
        assert!(!tool_result.is_error);
        assert_eq!(
            tool_result.content.len(),
            2,
            "both blocks must survive as typed content blocks"
        );
        assert!(
            matches!(
                tool_result.content[0],
                meerkat_core::ContentBlock::Text { .. }
            ),
            "first block should be typed Text"
        );
        assert!(
            matches!(
                tool_result.content[1],
                meerkat_core::ContentBlock::Image { .. }
            ),
            "image block must reach ToolResult as a typed Image, not stringified"
        );
        assert!(
            tool_result.has_images(),
            "typed image content must be detectable as an image"
        );
    }

    // ── ROW #138: plain-string tool result still parses through the typed
    // `WireToolResultContent::Text` arm.
    #[test]
    fn js_tool_callback_string_content_parses_as_text() {
        let result_json = serde_json::json!({ "content": "ok", "is_error": true }).to_string();
        let tool_result =
            parse_js_tool_result("call_2", &result_json).expect("string content should parse");
        assert!(tool_result.is_error);
        assert_eq!(tool_result.text_content(), "ok");
    }

    // ── ROW #173: a fire-and-forget JS tool dispatch must model the deferred
    // host action as a typed DETACHED async op (which does not gate the turn
    // boundary) and must NOT fabricate a completed `is_error:false` success
    // transcript that asserts the host action finished before it did.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test::wasm_bindgen_test(async)]
    #[allow(clippy::expect_used)]
    async fn fire_and_forget_dispatch_emits_detached_async_op_not_synchronous_success() {
        use meerkat_core::AgentToolDispatcher;

        init_test_runtime();
        register_js_tool(
            "request_approval".to_string(),
            "fire-and-forget approval".to_string(),
            json!({ "type": "object" }).to_string(),
        )
        .expect("register fire-and-forget tool");

        let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw args");
        let call = meerkat_core::ToolCallView {
            id: "call_ff",
            name: "request_approval",
            args: &args,
        };

        let outcome = super::JsToolDispatcher
            .dispatch(call)
            .await
            .expect("fire-and-forget dispatch should not error");

        // The deferred host action is recorded as a typed detached async op so
        // the turn boundary is not blocked on (or asserting) the host action.
        assert_eq!(outcome.async_ops.len(), 1, "exactly one deferred async op");
        assert_eq!(
            outcome.async_ops[0].wait_policy,
            meerkat_core::ops::WaitPolicy::Detached,
            "deferred host action must be DETACHED, not a turn-blocking barrier"
        );

        // The transcript result is NOT a fabricated completed success: it is
        // explicitly labeled deferred/pending so the model is not told the host
        // action succeeded before it happened.
        assert_eq!(outcome.result.tool_use_id, "call_ff");
        assert!(
            !outcome.result.text_content().contains("acknowledged"),
            "result must not assert a completed 'acknowledged' success"
        );
        assert!(
            outcome.result.text_content().contains("deferred"),
            "result must describe the deferred/pending host action"
        );

        clear_tool_callbacks();
    }
}
