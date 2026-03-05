pub mod consult;
pub mod deliberate;

use serde_json::{Value, json};

use crate::state::ForceState;

pub struct ToolCallError {
    pub code: i32,
    pub message: String,
}

impl ToolCallError {
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
        }
    }

    pub fn method_not_found(msg: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: msg.into(),
        }
    }
}

const MODEL_DESCRIPTION: &str = "\
Available models: \
claude-opus-4-6 (Anthropic, strongest reasoning), \
claude-sonnet-4-6 (Anthropic, fast + capable), \
gpt-5.3-codex (OpenAI, code-specialized), \
gpt-5.2-pro (OpenAI, deep reasoning — slow, use sparingly), \
gemini-3.1-pro-preview (Google, strong general), \
gemini-3-flash-preview (Google, fastest). \
Default: gpt-5.3-codex. \
Guidance: use opus/gpt-5.2-pro for complex reasoning (architecture, judging). \
Use sonnet/gpt-5.3-codex for code tasks. \
Use gemini-3-flash for speed-sensitive roles. \
Mix providers in multi-agent packs for perspective diversity";

pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "list_packs",
            "description": "List all available deliberation packs with their descriptions and agent counts.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "consult",
            "description": format!(
                "Get a quick opinion from a single AI agent. No coordination overhead \
                — like asking a colleague. Use this when you want a second opinion, \
                a sanity check, or a fresh perspective on something specific. {MODEL_DESCRIPTION}"
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question or topic to get an opinion on"
                    },
                    "context": {
                        "type": "string",
                        "description": "Optional context: code snippets, file contents, background information"
                    },
                    "model": {
                        "type": "string",
                        "description": format!("Model to use (default: gpt-5.3-codex). {MODEL_DESCRIPTION}")
                    },
                    "provider_params": {
                        "type": "object",
                        "description": "Provider-specific parameters. Examples: {\"reasoning_effort\": \"high\"} for deep thinking (OpenAI o-series, Anthropic extended thinking), {\"temperature\": 0.2} for more deterministic output",
                        "additionalProperties": true
                    }
                },
                "required": ["question"]
            }
        }),
        json!({
            "name": "deliberate",
            "description": format!(
                "Spawn a team of AI agents to collaboratively solve a problem. Each pack defines \
                a different team composition optimized for specific tasks. Returns structured results \
                after agents deliberate. {MODEL_DESCRIPTION}"
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pack": {
                        "type": "string",
                        "enum": ["advisor", "review", "architect", "brainstorm", "red-team", "panel", "rct"],
                        "description": "Team pack to use. advisor=quick opinion, review=parallel code review, architect=design deliberation, brainstorm=multi-model ideation, red-team=balanced risk assessment, panel=free-form review panel with moderator, rct=full RCT implementation pipeline"
                    },
                    "task": {
                        "type": "string",
                        "description": "The task or question for the team to work on"
                    },
                    "context": {
                        "type": "string",
                        "description": "Optional context: code, file contents, specifications, background"
                    },
                    "model_overrides": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": format!(
                            "Override models per role, e.g. {{\"critic\": \"claude-opus-4-6\", \"planner\": \"gpt-5.3-codex\"}}. \
                            Role names depend on the pack. {MODEL_DESCRIPTION}"
                        )
                    },
                    "provider_params": {
                        "type": "object",
                        "description": "Provider-specific parameters applied to ALL agents in the pack. Examples: {\"reasoning_effort\": \"high\"} for deep thinking, {\"temperature\": 0.2} for deterministic output. Per-agent overrides not yet supported.",
                        "additionalProperties": true
                    }
                },
                "required": ["pack", "task"]
            }
        }),
    ]
}

pub async fn handle_tool_call(
    state: &ForceState,
    name: &str,
    arguments: &Value,
    progress_token: Option<Value>,
) -> Result<Value, ToolCallError> {
    match name {
        "list_packs" => {
            let packs: Vec<Value> = state
                .pack_registry
                .all()
                .map(|p| json!({"name": p.name(), "description": p.description(), "agents": p.agent_count(), "flow_steps": p.flow_step_count()}))
                .collect();
            Ok(json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&packs).unwrap_or_default()}]}))
        }
        "consult" => consult::handle(state, arguments).await,
        "deliberate" => deliberate::handle(state, arguments, progress_token).await,
        _ => Err(ToolCallError::method_not_found(format!(
            "Unknown tool: {name}"
        ))),
    }
}
