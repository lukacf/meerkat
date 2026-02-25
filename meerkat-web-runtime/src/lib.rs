//! Meerkat WASM runtime — a real meerkat surface in the browser.
//!
//! Routes through `AgentFactory::build_agent()` with override-first resource
//! injection, same pipeline as CLI/RPC/REST/MCP. Uses real agent loop, real LLM
//! providers (browser fetch), real comms (inproc), and real tool dispatch.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

use meerkat::AgentBuildConfig;
use meerkat_core::Config;

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

const MAX_SYSTEM_PROMPT_BYTES: usize = 100 * 1024;
const FORBIDDEN_CAPABILITIES: &[&str] = &["shell", "mcp_stdio", "process_spawn"];
const SKILL_SEPARATOR: &str = "\n\n---\n\n";

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
}

fn default_max_tokens() -> u32 {
    4096
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
// Error helpers
// ═══════════════════════════════════════════════════════════

fn err_js(code: &str, message: &str) -> JsValue {
    let json = serde_json::json!({ "code": code, "message": message });
    JsValue::from_str(&json.to_string())
}

fn err_str(code: &str, msg: impl std::fmt::Display) -> JsValue {
    err_js(code, &msg.to_string())
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
    let composite = meerkat_tools::builtin::CompositeDispatcher::new_wasm(
        task_store, &config, None, None,
    )
    .map_err(|e| format!("failed to create tool dispatcher: {e}"))?;
    Ok(Arc::new(composite))
}

#[cfg(not(target_arch = "wasm32"))]
fn build_wasm_tool_dispatcher() -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, String> {
    Ok(Arc::new(meerkat_tools::EmptyToolDispatcher))
}

// ═══════════════════════════════════════════════════════════
// Exported WASM API
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

    // Create a minimal Config for the factory.
    let meerkat_config = Config::default();

    let handle = REGISTRY.with(|cell| {
        let mut registry = cell.borrow_mut();
        let handle = registry.next_handle;
        registry.next_handle = registry.next_handle.saturating_add(1);

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

/// Run a turn through the real meerkat agent loop via AgentFactory::build_agent().
///
/// Returns JSON: `{ "text", "usage", "status", "session_id", "turns", "tool_calls" }`
#[wasm_bindgen]
pub async fn start_turn(
    handle: u32,
    prompt: &str,
    _options_json: &str,
) -> Result<JsValue, JsValue> {
    // Extract what we need from the session (release borrow quickly).
    let (mut build_config, config, prior_session, run_id) = REGISTRY
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

            // Rebuild tools + store fresh each turn (they're cheap, avoids ownership issues).
            bc.tool_dispatcher_override = session.build_config_template.tool_dispatcher_override.clone();
            bc.session_store_override = session.build_config_template.session_store_override.clone();

            // Resume from prior session if available.
            bc.resume_session = session.meerkat_session.take();

            Ok((bc, session.config.clone(), session.meerkat_session.take(), run_id))
        })
        .map_err(|e: String| err_str("session_not_found", e))?;

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
