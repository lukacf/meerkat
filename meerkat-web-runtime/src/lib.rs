//! Meerkat WASM runtime — real agent stack in the browser.
//!
//! This is a full meerkat surface, same as CLI/RPC/REST/MCP.
//! Uses the real agent loop from meerkat-core and real LLM providers
//! from meerkat-client. reqwest uses browser fetch on wasm32.
//!
//! The runtime loads mobpacks, extracts skills for the system prompt,
//! creates real Agent instances, and runs turns with streaming events.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

use meerkat_client::adapter::LlmClientAdapter;
use meerkat_client::types::LlmClient;
use meerkat_core::{AgentBuilder, AgentSessionStore, AgentToolDispatcher};

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
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
struct MobDefinition {
    id: String,
}

// ═══════════════════════════════════════════════════════════
// Session Config
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
}

fn default_max_tokens() -> u32 {
    4096
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionStatus {
    Idle,
    Running,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

// ═══════════════════════════════════════════════════════════
// Event Model (mirrors AgentEvent)
// ═══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RuntimeEvent {
    RunStarted {
        session_id: u32,
        run_id: u64,
        prompt: String,
    },
    TextDelta {
        delta: String,
    },
    TextComplete {
        content: String,
    },
    ToolCallRequested {
        id: String,
        name: String,
    },
    ToolResultReceived {
        id: String,
        name: String,
        is_error: bool,
    },
    TurnCompleted {
        run_id: u64,
        usage: Usage,
    },
    RunCompleted {
        session_id: u32,
        run_id: u64,
        result: String,
        usage: Usage,
    },
    RunFailed {
        session_id: u32,
        run_id: u64,
        error: String,
        error_code: String,
    },
    SystemPromptTruncated {
        original_bytes: usize,
        truncated_to: usize,
    },
}

// ═══════════════════════════════════════════════════════════
// No-op Tool Dispatcher + Session Store (browser has no tools/persistence)
// ═══════════════════════════════════════════════════════════

struct NoOpTools;

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for NoOpTools {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        Arc::new([])
    }
    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::types::ToolResult, meerkat_core::error::ToolError> {
        Err(meerkat_core::error::ToolError::NotFound {
            name: call.name.to_string(),
        })
    }
}

struct NoOpStore;

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentSessionStore for NoOpStore {
    async fn save(
        &self,
        _session: &meerkat_core::session::Session,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }
    async fn load(
        &self,
        _id: &str,
    ) -> Result<Option<meerkat_core::session::Session>, meerkat_core::error::AgentError> {
        Ok(None)
    }
}

// ═══════════════════════════════════════════════════════════
// Runtime Session
// ═══════════════════════════════════════════════════════════

struct RuntimeSession {
    handle: u32,
    mob_id: String,
    model: String,
    max_tokens: u32,
    compiled_system_prompt: String,
    skills: BTreeMap<String, String>,
    /// The real meerkat LLM client adapter
    client: Arc<LlmClientAdapter>,
    events: VecDeque<RuntimeEvent>,
    seq: u64,
    run_counter: u64,
    status: SessionStatus,
    usage: Usage,
    /// Accumulated text from last run
    last_result: String,
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
    definition: MobDefinition,
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

    let definition: MobDefinition = serde_json::from_slice(
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
) -> (String, Option<RuntimeEvent>) {
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
        let safe_end = {
            let mut i = MAX_SYSTEM_PROMPT_BYTES.min(joined.len());
            while i > 0 && !joined.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        let event = RuntimeEvent::SystemPromptTruncated {
            original_bytes: joined.len(),
            truncated_to: safe_end,
        };
        (joined[..safe_end].to_string(), Some(event))
    } else {
        (joined, None)
    }
}

// ═══════════════════════════════════════════════════════════
// Provider Resolution
// ═══════════════════════════════════════════════════════════

fn create_llm_client(
    model: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> Result<Arc<dyn LlmClient>, String> {
    if model.starts_with("claude") {
        let mut builder = meerkat_client::anthropic::AnthropicClientBuilder::new(api_key.into());
        if let Some(url) = base_url {
            builder = builder.base_url(url.into());
        }
        let client = builder
            .build()
            .map_err(|e| format!("failed to create Anthropic client: {e}"))?;
        Ok(Arc::new(client))
    } else if model.starts_with("gpt") {
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
// Registry Helpers
// ═══════════════════════════════════════════════════════════

fn with_session_mut<T>(
    handle: u32,
    f: impl FnOnce(&mut RuntimeSession) -> Result<T, String>,
) -> Result<T, JsValue> {
    REGISTRY
        .with(|cell| {
            let mut registry = cell.borrow_mut();
            let session = registry
                .sessions
                .get_mut(&handle)
                .ok_or_else(|| format!("unknown session handle: {handle}"))?;
            f(session)
        })
        .map_err(|e| err_str("session_not_found", e))
}

fn push_event(session: &mut RuntimeSession, event: RuntimeEvent) {
    session.seq = session.seq.saturating_add(1);
    session.events.push_back(event);
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
// Exported WASM API
// ═══════════════════════════════════════════════════════════

/// Create a session from a mobpack + API key + model.
///
/// `config_json`: `{ "model": "claude-sonnet-4-5", "api_key": "sk-...", "max_tokens"?: N, "base_url"?: "..." }`
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

    let llm_client = create_llm_client(&config.model, api_key, config.base_url.as_deref())
        .map_err(|e| err_str("provider_error", e))?;

    let adapter = Arc::new(LlmClientAdapter::new(llm_client, config.model.clone()));

    let (compiled_prompt, truncation_event) = compile_system_prompt(
        &parsed.definition.id,
        &parsed.manifest.mobpack.name,
        &parsed.skills,
        config.system_prompt.as_deref(),
    );

    let handle = REGISTRY.with(|cell| {
        let mut registry = cell.borrow_mut();
        let handle = registry.next_handle;
        registry.next_handle = registry.next_handle.saturating_add(1);

        let mut session = RuntimeSession {
            handle,
            mob_id: parsed.definition.id,
            model: config.model,
            max_tokens: config.max_tokens,
            compiled_system_prompt: compiled_prompt,
            skills: parsed.skills,
            client: adapter,
            events: VecDeque::new(),
            seq: 0,
            run_counter: 0,
            status: SessionStatus::Idle,
            usage: Usage::default(),
            last_result: String::new(),
        };

        if let Some(event) = truncation_event {
            push_event(&mut session, event);
        }

        registry.sessions.insert(handle, session);
        handle
    });

    Ok(handle)
}

/// Run a turn: send prompt through the REAL meerkat agent loop.
///
/// This creates an Agent via AgentBuilder, sets the system prompt from
/// mobpack skills, and runs the agent loop which calls the LLM provider
/// via reqwest (browser fetch on wasm32).
///
/// Returns JSON: `{ "run_id", "text", "usage", "session_id", "status" }`
#[wasm_bindgen]
pub async fn start_turn(
    handle: u32,
    prompt: &str,
    _options_json: &str,
) -> Result<JsValue, JsValue> {
    // Extract what we need from the session (release borrow quickly)
    let (client, system_prompt, max_tokens, run_id) = with_session_mut(handle, |session| {
        if session.status == SessionStatus::Running {
            return Err(format!("session {} is already running", session.handle));
        }
        session.status = SessionStatus::Running;
        session.run_counter = session.run_counter.saturating_add(1);
        let run_id = session.run_counter;

        push_event(
            session,
            RuntimeEvent::RunStarted {
                session_id: session.handle,
                run_id,
                prompt: prompt.to_string(),
            },
        );

        Ok((
            session.client.clone(),
            session.compiled_system_prompt.clone(),
            session.max_tokens,
            run_id,
        ))
    })?;

    // Build and run the agent (outside of RefCell borrow)
    let tools: Arc<NoOpTools> = Arc::new(NoOpTools);
    let store: Arc<NoOpStore> = Arc::new(NoOpStore);

    let mut agent = AgentBuilder::new()
        .system_prompt(system_prompt)
        .max_tokens_per_turn(max_tokens)
        .build(client, tools, store)
        .await;

    let run_result = agent.run(prompt.into()).await;

    // Record results back in session
    match run_result {
        Ok(result) => {
            let result_json = with_session_mut(handle, |session| {
                let turn_usage = Usage {
                    input_tokens: result.usage.input_tokens,
                    output_tokens: result.usage.output_tokens,
                };
                session.usage.input_tokens = session
                    .usage
                    .input_tokens
                    .saturating_add(turn_usage.input_tokens);
                session.usage.output_tokens = session
                    .usage
                    .output_tokens
                    .saturating_add(turn_usage.output_tokens);
                session.last_result = result.text.clone();

                push_event(
                    session,
                    RuntimeEvent::TextComplete {
                        content: result.text.clone(),
                    },
                );
                push_event(
                    session,
                    RuntimeEvent::RunCompleted {
                        session_id: session.handle,
                        run_id,
                        result: result.text.clone(),
                        usage: turn_usage,
                    },
                );
                session.status = SessionStatus::Idle;

                Ok(serde_json::json!({
                    "run_id": run_id,
                    "text": result.text,
                    "usage": { "input_tokens": result.usage.input_tokens, "output_tokens": result.usage.output_tokens },
                    "session_id": session.handle,
                    "status": "completed",
                    "turns": result.turns,
                    "tool_calls": result.tool_calls,
                })
                .to_string())
            })?;

            Ok(JsValue::from_str(&result_json))
        }
        Err(err) => {
            let error_msg = format!("{err}");
            with_session_mut(handle, |session| {
                push_event(
                    session,
                    RuntimeEvent::RunFailed {
                        session_id: session.handle,
                        run_id,
                        error: error_msg.clone(),
                        error_code: "agent_error".to_string(),
                    },
                );
                session.status = SessionStatus::Idle;
                Ok(())
            })?;

            let result = serde_json::json!({
                "run_id": run_id,
                "text": "",
                "usage": { "input_tokens": 0, "output_tokens": 0 },
                "session_id": handle,
                "status": "failed",
                "error": error_msg,
            });
            Ok(JsValue::from_str(&result.to_string()))
        }
    }
}

/// Drain and return all pending events.
#[wasm_bindgen]
pub fn poll_events(handle: u32) -> Result<String, JsValue> {
    with_session_mut(handle, |session| {
        let drained: Vec<RuntimeEvent> = session.events.drain(..).collect();
        serde_json::to_string(&drained).map_err(|e| format!("failed encoding events: {e}"))
    })
}

/// Get current session state metadata.
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
                "status": session.status,
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

/// Remove a session from the registry.
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
