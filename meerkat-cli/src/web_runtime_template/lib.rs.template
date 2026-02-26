//! Generic WASM runtime for Meerkat mobpacks.
//!
//! Domain-agnostic session/turn/event API with surface parity to CLI/RPC/REST/MCP.
//! No application-specific types or logic — behavior comes from mobpack skills.
//! LLM provider calls are delegated to a JS bridge module.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use wasm_bindgen::prelude::*;

// ═══════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const MAX_SYSTEM_PROMPT_BYTES: usize = 100 * 1024;
const FORBIDDEN_CAPABILITIES: &[&str] = &["shell", "mcp_stdio", "process_spawn"];
const SKILL_SEPARATOR: &str = "\n\n---\n\n";

// ═══════════════════════════════════════════════════════════
// JS Bridge Import — stable contract
// ═══════════════════════════════════════════════════════════

#[wasm_bindgen]
extern "C" {
    /// v1 bridge contract: the host JS environment must provide this function.
    ///
    /// Called by the WASM runtime to make an LLM API request.
    /// The JS side routes to the appropriate provider (Anthropic, OpenAI, Gemini, proxy)
    /// based on the model name and options.
    ///
    /// # Arguments
    /// - `model`: model identifier (e.g. "claude-sonnet-4-5", "gpt-5.2")
    /// - `messages_json`: JSON array of `[{role, content}]` messages
    /// - `max_tokens`: maximum response tokens
    /// - `options_json`: JSON object with provider params, proxy_url, etc.
    ///
    /// # Returns (JSON string)
    /// Success: `{ "text": "...", "usage": { "input_tokens": N, "output_tokens": N } }`
    /// Error: rejects with `{ "code": "...", "message": "..." }`
    #[wasm_bindgen(catch)]
    async fn js_llm_call(
        model: &str,
        messages_json: &str,
        max_tokens: u32,
        options_json: &str,
    ) -> Result<JsValue, JsValue>;
}

// ═══════════════════════════════════════════════════════════
// Mobpack Types (generic)
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
// Session Types
// ═══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionConfig {
    model: String,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
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
// Snapshot Schema
// ═══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionSnapshot {
    schema_version: u32,
    handle: u32,
    mob_id: String,
    model: String,
    max_tokens: u32,
    compiled_system_prompt: String,
    skills: BTreeMap<String, String>,
    messages: Vec<ChatMessage>,
    pending_events: Vec<RuntimeEvent>,
    seq: u64,
    run_counter: u64,
    status: SessionStatus,
    usage: Usage,
    // NOTE: no API keys, no auth tokens, no provider headers
}

// ═══════════════════════════════════════════════════════════
// Runtime Session + Registry
// ═══════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct RuntimeSession {
    handle: u32,
    mob_id: String,
    model: String,
    max_tokens: u32,
    compiled_system_prompt: String,
    skills: BTreeMap<String, String>,
    messages: Vec<ChatMessage>,
    events: VecDeque<RuntimeEvent>,
    seq: u64,
    run_counter: u64,
    status: SessionStatus,
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
// Error Type
// ═══════════════════════════════════════════════════════════

fn err_js(code: &str, message: &str) -> JsValue {
    let json = serde_json::json!({ "code": code, "message": message });
    JsValue::from_str(&json.to_string())
}

fn err_str(code: &str, msg: impl std::fmt::Display) -> JsValue {
    err_js(code, &msg.to_string())
}

// ═══════════════════════════════════════════════════════════
// Mobpack Parsing (domain-agnostic — kept from original)
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

    // Extract skills: files under skills/ directory, keyed by filename without extension
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

    // Skills in deterministic BTreeMap order (lexicographic by key)
    for (name, content) in skills {
        parts.push(format!("## Skill: {name}\n\n{content}"));
    }

    if let Some(extra) = user_system_prompt {
        parts.push(extra.to_string());
    }

    let joined = parts.join(SKILL_SEPARATOR);

    if joined.len() > MAX_SYSTEM_PROMPT_BYTES {
        // Find the last complete UTF-8 char boundary at or before the limit.
        // floor_char_boundary is safe — never panics on valid &str.
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
// Registry Helpers
// ═══════════════════════════════════════════════════════════

fn with_session<T>(
    handle: u32,
    f: impl FnOnce(&RuntimeSession) -> Result<T, String>,
) -> Result<T, JsValue> {
    REGISTRY
        .with(|cell| {
            let registry = cell.borrow();
            let session = registry
                .sessions
                .get(&handle)
                .ok_or_else(|| format!("unknown session handle: {handle}"))?;
            f(session)
        })
        .map_err(|e| err_str("session_not_found", e))
}

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
// Bridge Response Parsing
// ═══════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct BridgeResponse {
    text: String,
    #[serde(default)]
    usage: BridgeUsage,
}

#[derive(Deserialize, Default)]
struct BridgeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

fn parse_bridge_error(val: JsValue) -> (String, String) {
    if let Some(s) = val.as_string() {
        // Try parsing as JSON error object
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&s) {
            let code = parsed["code"].as_str().unwrap_or("bridge_error");
            let message = parsed["message"].as_str().unwrap_or(&s);
            return (code.to_string(), message.to_string());
        }
        return ("bridge_error".to_string(), s);
    }
    (
        "bridge_error".to_string(),
        "unknown bridge error".to_string(),
    )
}

// ═══════════════════════════════════════════════════════════
// Archive Extraction (kept from original — domain-agnostic)
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

/// Create a session from a mobpack archive.
///
/// Extracts skills, assembles system prompt, returns a session handle.
/// `config_json`: `{ "model": "...", "system_prompt"?: "...", "max_tokens"?: N }`
#[wasm_bindgen]
pub fn create_session(mobpack_bytes: &[u8], config_json: &str) -> Result<u32, JsValue> {
    let parsed = parse_mobpack(mobpack_bytes).map_err(|e| err_str("invalid_mobpack", e))?;

    let config: SessionConfig =
        serde_json::from_str(config_json).map_err(|e| err_str("invalid_config", e))?;

    if config.model.trim().is_empty() {
        return Err(err_js("invalid_config", "model must not be empty"));
    }

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
            messages: Vec::new(),
            events: VecDeque::new(),
            seq: 0,
            run_counter: 0,
            status: SessionStatus::Idle,
            usage: Usage::default(),
        };

        if let Some(event) = truncation_event {
            push_event(&mut session, event);
        }

        registry.sessions.insert(handle, session);
        handle
    });

    Ok(handle)
}

/// Run a turn: send prompt to LLM via JS bridge, return result.
///
/// `options_json`: `{}` or `{ "proxy_url"?: "...", "temperature"?: N }`
/// Returns JSON: `{ "run_id", "text", "usage", "session_id", "status": "completed"|"failed", "error"? }`
#[wasm_bindgen]
pub async fn start_turn(handle: u32, prompt: &str, options_json: &str) -> Result<JsValue, JsValue> {
    // Validate and prepare — synchronous registry access
    let (model, max_tokens, messages_json, run_id) = with_session_mut(handle, |session| {
        if session.status == SessionStatus::Running {
            return Err(format!("session {} is already running", session.handle));
        }
        session.status = SessionStatus::Running;
        session.run_counter = session.run_counter.saturating_add(1);
        let run_id = session.run_counter;

        // Add user message
        session.messages.push(ChatMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        });

        // Build messages array for the LLM
        let mut llm_messages = Vec::new();
        llm_messages.push(ChatMessage {
            role: "system".to_string(),
            content: session.compiled_system_prompt.clone(),
        });
        llm_messages.extend(session.messages.clone());

        let messages_json = serde_json::to_string(&llm_messages)
            .map_err(|e| format!("failed to serialize messages: {e}"))?;

        push_event(
            session,
            RuntimeEvent::RunStarted {
                session_id: session.handle,
                run_id,
                prompt: prompt.to_string(),
            },
        );

        Ok((
            session.model.clone(),
            session.max_tokens,
            messages_json,
            run_id,
        ))
    })?;

    // Call the JS bridge (async — outside of RefCell borrow)
    let bridge_result = js_llm_call(&model, &messages_json, max_tokens, options_json).await;

    // Process the result — synchronous registry access again
    match bridge_result {
        Ok(response_val) => {
            // Accept both string JSON and serialized JsValue from bridge
            let response_str = if let Some(s) = response_val.as_string() {
                s
            } else {
                // Try JSON.stringify for object values
                js_sys::JSON::stringify(&response_val)
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
            };
            match serde_json::from_str::<BridgeResponse>(&response_str) {
                Ok(response) => {
                    let result_json = with_session_mut(handle, |session| {
                        // Add assistant response to history
                        session.messages.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: response.text.clone(),
                        });

                        // Update usage
                        session.usage.input_tokens = session
                            .usage
                            .input_tokens
                            .saturating_add(response.usage.input_tokens);
                        session.usage.output_tokens = session
                            .usage
                            .output_tokens
                            .saturating_add(response.usage.output_tokens);

                        let turn_usage = Usage {
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                        };

                        push_event(
                            session,
                            RuntimeEvent::TextComplete {
                                content: response.text.clone(),
                            },
                        );
                        push_event(
                            session,
                            RuntimeEvent::TurnCompleted {
                                run_id,
                                usage: turn_usage.clone(),
                            },
                        );
                        push_event(
                            session,
                            RuntimeEvent::RunCompleted {
                                session_id: session.handle,
                                run_id,
                                result: response.text.clone(),
                                usage: turn_usage.clone(),
                            },
                        );

                        session.status = SessionStatus::Idle;

                        Ok(serde_json::json!({
                            "run_id": run_id,
                            "text": response.text,
                            "usage": { "input_tokens": turn_usage.input_tokens, "output_tokens": turn_usage.output_tokens },
                            "session_id": session.handle,
                            "status": "completed"
                        })
                        .to_string())
                    })?;

                    Ok(JsValue::from_str(&result_json))
                }
                Err(parse_err) => {
                    let error_msg = format!("failed to parse bridge response: {parse_err}");
                    with_session_mut(handle, |session| {
                        push_event(
                            session,
                            RuntimeEvent::RunFailed {
                                session_id: session.handle,
                                run_id,
                                error: error_msg.clone(),
                                error_code: "bridge_parse_error".to_string(),
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
                        "error": error_msg
                    });
                    Ok(JsValue::from_str(&result.to_string()))
                }
            }
        }
        Err(err_val) => {
            let (code, message) = parse_bridge_error(err_val);
            let error_msg = format!("{code}: {message}");

            with_session_mut(handle, |session| {
                push_event(
                    session,
                    RuntimeEvent::RunFailed {
                        session_id: session.handle,
                        run_id,
                        error: error_msg.clone(),
                        error_code: code.clone(),
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
                "error": error_msg
            });
            Ok(JsValue::from_str(&result.to_string()))
        }
    }
}

/// Drain and return all pending events for a session.
#[wasm_bindgen]
pub fn poll_events(handle: u32) -> Result<String, JsValue> {
    with_session_mut(handle, |session| {
        let drained: Vec<RuntimeEvent> = session.events.drain(..).collect();
        serde_json::to_string(&drained).map_err(|e| format!("failed encoding events: {e}"))
    })
}

/// Get current session state (no message content — just metadata).
#[wasm_bindgen]
pub fn get_session_state(handle: u32) -> Result<String, JsValue> {
    with_session(handle, |session| {
        let state = serde_json::json!({
            "session_id": session.handle,
            "mob_id": session.mob_id,
            "model": session.model,
            "status": session.status,
            "message_count": session.messages.len(),
            "usage": session.usage,
            "run_counter": session.run_counter,
        });
        Ok(state.to_string())
    })
}

/// Inspect a mobpack without creating a session.
///
/// Returns manifest, definition, and skill inventory.
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
        "definition": {
            "id": parsed.definition.id,
        },
        "skills": skills,
        "capabilities": parsed.manifest.requires.as_ref().map(|r| &r.capabilities),
    });

    serde_json::to_string(&result).map_err(|e| err_str("serialize_error", e))
}

/// Serialize full session state for persistence.
///
/// Contains no API keys, auth tokens, or provider headers.
#[wasm_bindgen]
pub fn snapshot_session(handle: u32) -> Result<String, JsValue> {
    with_session(handle, |session| {
        let snapshot = SessionSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            handle: session.handle,
            mob_id: session.mob_id.clone(),
            model: session.model.clone(),
            max_tokens: session.max_tokens,
            compiled_system_prompt: session.compiled_system_prompt.clone(),
            skills: session.skills.clone(),
            messages: session.messages.clone(),
            pending_events: session.events.iter().cloned().collect(),
            seq: session.seq,
            run_counter: session.run_counter,
            status: session.status,
            usage: session.usage.clone(),
        };
        serde_json::to_string(&snapshot).map_err(|e| format!("failed encoding snapshot: {e}"))
    })
}

/// Restore session from a snapshot.
#[wasm_bindgen]
pub fn restore_session(handle: u32, snapshot_json: &str) -> Result<(), JsValue> {
    let snapshot: SessionSnapshot =
        serde_json::from_str(snapshot_json).map_err(|e| err_str("invalid_snapshot", e))?;

    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Err(err_js(
            "snapshot_version_mismatch",
            &format!(
                "expected schema_version {SNAPSHOT_SCHEMA_VERSION}, got {}",
                snapshot.schema_version
            ),
        ));
    }

    with_session_mut(handle, |session| {
        session.mob_id = snapshot.mob_id;
        session.model = snapshot.model;
        session.max_tokens = snapshot.max_tokens;
        session.compiled_system_prompt = snapshot.compiled_system_prompt;
        session.skills = snapshot.skills;
        session.messages = snapshot.messages;
        session.events = snapshot.pending_events.into_iter().collect();
        session.seq = snapshot.seq;
        session.run_counter = snapshot.run_counter;
        session.status = snapshot.status;
        session.usage = snapshot.usage;
        Ok(())
    })
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

// ═══════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn create_targz(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.mode(tar::HeaderMode::Deterministic);
            for (path, content) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_mtime(0);
                header.set_uid(0);
                header.set_gid(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, path.as_str(), content.as_slice())
                    .expect("append");
            }
            builder.finish().expect("finish tar");
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).expect("write gz");
        encoder.finish().expect("finish gz")
    }

    fn fixture_mobpack(forbidden: bool) -> Vec<u8> {
        let manifest = if forbidden {
            "[mobpack]\nname = \"test\"\nversion = \"1.0.0\"\n\n[requires]\ncapabilities = [\"shell\"]\n"
        } else {
            "[mobpack]\nname = \"test-mob\"\nversion = \"1.0.0\"\ndescription = \"A test mob\"\n"
        };
        let definition = r#"{"id":"test-mob-id"}"#;
        let skill_a = "You are a helpful assistant.";
        let skill_b = "Always respond in JSON.";

        let mut files = BTreeMap::from([
            ("manifest.toml".to_string(), manifest.as_bytes().to_vec()),
            (
                "definition.json".to_string(),
                definition.as_bytes().to_vec(),
            ),
        ]);
        if !forbidden {
            files.insert(
                "skills/assistant.md".to_string(),
                skill_a.as_bytes().to_vec(),
            );
            files.insert(
                "skills/json-output.md".to_string(),
                skill_b.as_bytes().to_vec(),
            );
        }
        create_targz(&files)
    }

    fn create_single_entry_archive(path: &str) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.mode(tar::HeaderMode::Deterministic);
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            builder
                .append_data(&mut header, path, b"x".as_slice())
                .expect("append");
            builder.finish().expect("finish tar");
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).expect("write gz");
        encoder.finish().expect("finish gz")
    }

    #[test]
    fn create_session_extracts_skills() {
        let pack = fixture_mobpack(false);
        let config = r#"{"model":"claude-sonnet-4-5"}"#;
        let handle = create_session(&pack, config).expect("create_session");
        assert!(handle > 0);

        let state_json = get_session_state(handle).expect("get_session_state");
        let state: serde_json::Value = serde_json::from_str(&state_json).expect("parse state");
        assert_eq!(state["mob_id"], "test-mob-id");
        assert_eq!(state["model"], "claude-sonnet-4-5");
        assert_eq!(state["message_count"], 0);
    }

    #[test]
    fn parse_mobpack_rejects_forbidden_capability() {
        let pack = fixture_mobpack(true);
        let err = parse_mobpack(&pack).expect_err("should reject forbidden");
        assert!(err.contains("forbidden capability"), "unexpected: {err}");
    }

    #[test]
    fn session_config_rejects_empty_model() {
        let config: SessionConfig = serde_json::from_str(r#"{"model":""}"#).expect("parse");
        assert!(config.model.trim().is_empty());
    }

    #[test]
    fn inspect_mobpack_returns_skills() {
        let pack = fixture_mobpack(false);
        let json = inspect_mobpack(&pack).expect("inspect");
        let val: serde_json::Value = serde_json::from_str(&json).expect("parse");
        let skills = val["skills"].as_array().expect("skills array");
        assert_eq!(skills.len(), 2);
        // Lexicographic order
        assert_eq!(skills[0]["name"], "assistant");
        assert_eq!(skills[1]["name"], "json-output");
    }

    #[test]
    fn snapshot_restore_roundtrip() {
        let pack = fixture_mobpack(false);
        let config = r#"{"model":"gpt-5.2","max_tokens":1024}"#;
        let handle = create_session(&pack, config).expect("create");

        let snap = snapshot_session(handle).expect("snapshot");
        let snap_val: serde_json::Value = serde_json::from_str(&snap).expect("parse snap");
        assert_eq!(snap_val["schema_version"], SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snap_val["model"], "gpt-5.2");
        assert_eq!(snap_val["max_tokens"], 1024);

        // Restore into same handle
        restore_session(handle, &snap).expect("restore");

        let state_json = get_session_state(handle).expect("state after restore");
        let state: serde_json::Value = serde_json::from_str(&state_json).expect("parse");
        assert_eq!(state["model"], "gpt-5.2");
    }

    #[test]
    fn snapshot_schema_version_recorded() {
        let pack = fixture_mobpack(false);
        let config = r#"{"model":"test"}"#;
        let handle = create_session(&pack, config).expect("create");

        let snap_json = snapshot_session(handle).expect("snap");
        let snap: serde_json::Value = serde_json::from_str(&snap_json).expect("parse");
        assert_eq!(snap["schema_version"], SNAPSHOT_SCHEMA_VERSION);
    }

    #[test]
    fn snapshot_deserialize_rejects_bad_version() {
        let snap = serde_json::json!({
            "schema_version": 999,
            "handle": 1,
            "mob_id": "test",
            "model": "test",
            "max_tokens": 1024,
            "compiled_system_prompt": "",
            "skills": {},
            "messages": [],
            "pending_events": [],
            "seq": 0,
            "run_counter": 0,
            "status": "idle",
            "usage": { "input_tokens": 0, "output_tokens": 0 }
        });
        let snap_str = serde_json::to_string(&snap).expect("ser");
        let parsed: SessionSnapshot = serde_json::from_str(&snap_str).expect("deser");
        assert_ne!(parsed.schema_version, SNAPSHOT_SCHEMA_VERSION);
    }

    #[test]
    fn registry_remove_works() {
        let pack = fixture_mobpack(false);
        let config = r#"{"model":"test"}"#;
        let handle = create_session(&pack, config).expect("create");

        // Session exists
        let state = get_session_state(handle);
        assert!(state.is_ok());

        // Remove it via registry directly
        REGISTRY.with(|cell| {
            let mut reg = cell.borrow_mut();
            assert!(reg.sessions.remove(&handle).is_some());
        });

        // Verify it's gone via internal helper
        let result = REGISTRY.with(|cell| {
            let reg = cell.borrow();
            reg.sessions.contains_key(&handle)
        });
        assert!(!result);
    }

    #[test]
    fn normalize_rejects_parent_traversal() {
        let err = normalize_for_archive("../etc/passwd").expect_err("should fail");
        assert!(err.contains("parent directory traversal"));
    }

    #[test]
    fn normalize_rejects_embedded_traversal() {
        let err = normalize_for_archive("foo/../bar").expect_err("should fail");
        assert!(err.contains("parent directory traversal"));
    }

    #[test]
    fn extract_rejects_windows_absolute_paths() {
        let archive = create_single_entry_archive("C:/temp/evil.txt");
        let err = extract_targz_safe(&archive).expect_err("should fail");
        assert!(err.contains("absolute path"));
    }

    #[test]
    fn system_prompt_deterministic_ordering() {
        let mut skills = BTreeMap::new();
        skills.insert("zulu".to_string(), "last".to_string());
        skills.insert("alpha".to_string(), "first".to_string());
        skills.insert("mike".to_string(), "middle".to_string());

        let (prompt, _) = compile_system_prompt("id", "name", &skills, None);
        let alpha_pos = prompt.find("## Skill: alpha").expect("alpha");
        let mike_pos = prompt.find("## Skill: mike").expect("mike");
        let zulu_pos = prompt.find("## Skill: zulu").expect("zulu");
        assert!(alpha_pos < mike_pos);
        assert!(mike_pos < zulu_pos);
    }

    #[test]
    fn system_prompt_truncation() {
        let mut skills = BTreeMap::new();
        let large = "x".repeat(MAX_SYSTEM_PROMPT_BYTES + 1000);
        skills.insert("big".to_string(), large);

        let (prompt, event) = compile_system_prompt("id", "name", &skills, None);
        assert!(prompt.len() <= MAX_SYSTEM_PROMPT_BYTES);
        assert!(event.is_some());
        if let Some(RuntimeEvent::SystemPromptTruncated {
            original_bytes,
            truncated_to,
        }) = event
        {
            assert!(original_bytes > MAX_SYSTEM_PROMPT_BYTES);
            assert!(truncated_to <= MAX_SYSTEM_PROMPT_BYTES);
        }
    }

    #[test]
    fn poll_events_drains() {
        let pack = fixture_mobpack(false);
        let config = r#"{"model":"test"}"#;
        let handle = create_session(&pack, config).expect("create");

        // First poll should be empty (or just truncation if skills are huge)
        let events_json = poll_events(handle).expect("poll");
        let events: Vec<serde_json::Value> =
            serde_json::from_str(&events_json).expect("parse events");
        // Second poll should also be empty (drained)
        let events_json2 = poll_events(handle).expect("poll2");
        let events2: Vec<serde_json::Value> =
            serde_json::from_str(&events_json2).expect("parse events2");
        assert!(events2.is_empty(), "should be drained: {events:?}");
    }
}
