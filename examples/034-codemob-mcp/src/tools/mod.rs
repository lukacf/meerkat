pub mod consult;
pub mod deliberate;
pub mod mobs;

use std::sync::Arc;

use meerkat::surface::RequestContext;
use serde_json::{Value, json};

use crate::state::ForceState;

pub type ProgressNotifier = Arc<dyn Fn(Value, usize, usize, String) + Send + Sync>;

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
            "description": "List all available deliberation packs (built-in and user-created) with their descriptions and agent counts.",
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
                a different team composition optimized for specific tasks. Use list_packs to see all \
                available packs including user-created ones. Returns structured results after agents \
                deliberate. {MODEL_DESCRIPTION}"
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pack": {
                        "type": "string",
                        "description": "Team pack to use. Built-in: advisor, review, architect, brainstorm, red-team, panel, implement, rct. User-created packs also accepted — use list_packs to see all."
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
        json!({
            "name": "create_mob",
            "description": "Create a custom mob definition. Saved to .codemob-mcp/mobs/ and immediately available in deliberate without restart.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "definition": {
                        "type": "object",
                        "description": "Mob definition",
                        "properties": {
                            "name": { "type": "string", "description": "Unique mob name (alphanumeric, hyphens, underscores)" },
                            "description": { "type": "string", "description": "Human-readable description" },
                            "mode": { "type": "string", "enum": ["comms", "flow"], "description": "comms=autonomous agents communicate freely (good for iterative loops), flow=structured DAG steps" },
                            "orchestrator": { "type": "string", "description": "For comms mode: agent whose output is captured as the result and who receives the initial message" },
                            "agents": {
                                "type": "object",
                                "description": "Map of agent_name → {model, skill, peer_description}",
                                "additionalProperties": {
                                    "type": "object",
                                    "properties": {
                                        "model": { "type": "string", "description": "LLM model name" },
                                        "skill": { "type": "string", "description": "System prompt / role instructions for this agent" },
                                        "peer_description": { "type": "string", "description": "Short description visible to peer agents" }
                                    },
                                    "required": ["model", "skill"]
                                }
                            },
                            "wiring": {
                                "type": "array",
                                "description": "Pairs of agent names to wire for communication, e.g. [[\"coder\", \"reviewer\"]]",
                                "items": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 2 }
                            },
                            "flows": {
                                "type": "object",
                                "description": "For flow mode: map of flow_name → steps array. Use 'main' as the flow name.",
                                "additionalProperties": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "id": { "type": "string" },
                                            "role": { "type": "string", "description": "Agent name that executes this step" },
                                            "message": { "type": "string", "description": "Prompt. Use {{ task }} for the user's task, {{ steps.<id> }} for prior step output" },
                                            "depends_on": { "type": "array", "items": { "type": "string" } },
                                            "timeout_ms": { "type": "integer" }
                                        },
                                        "required": ["id", "role", "message"]
                                    }
                                }
                            }
                        },
                        "required": ["name", "description", "agents"]
                    }
                },
                "required": ["definition"]
            }
        }),
        json!({
            "name": "get_mob",
            "description": "Read a user-created mob definition.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Mob name" }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "update_mob",
            "description": "Update an existing user-created mob definition. Same schema as create_mob.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "definition": {
                        "type": "object",
                        "description": "Complete mob definition (replaces the existing one)",
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "mode": { "type": "string", "enum": ["comms", "flow"] },
                            "orchestrator": { "type": "string" },
                            "agents": { "type": "object", "additionalProperties": true },
                            "wiring": { "type": "array" },
                            "flows": { "type": "object" }
                        },
                        "required": ["name", "description", "agents"]
                    }
                },
                "required": ["definition"]
            }
        }),
        json!({
            "name": "delete_mob",
            "description": "Delete a user-created mob definition.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Mob name to delete" }
                },
                "required": ["name"]
            }
        }),
    ]
}

pub async fn handle_tool_call(
    state: &ForceState,
    name: &str,
    arguments: &Value,
    progress_token: Option<Value>,
    progress_notifier: Option<ProgressNotifier>,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    match name {
        "list_packs" => {
            // Reload user packs from disk on every list call (no restart needed)
            state.reload_user_packs();
            let packs: Vec<Value> = state
                .pack_registry()
                .all()
                .map(|p| json!({"name": p.name(), "description": p.description(), "agents": p.agent_count(), "flow_steps": p.flow_step_count()}))
                .collect();
            Ok(json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&packs).unwrap_or_default()}]}))
        }
        "consult" => consult::handle(state, arguments, request_context).await,
        "deliberate" => {
            // Reload user packs so newly created mobs are available
            state.reload_user_packs();
            deliberate::handle(
                state,
                arguments,
                progress_token,
                progress_notifier,
                request_context,
            )
            .await
        }
        "create_mob" => {
            let result = mobs::handle_create(arguments).await?;
            state.reload_user_packs();
            Ok(result)
        }
        "get_mob" => mobs::handle_get(arguments).await,
        "update_mob" => {
            let result = mobs::handle_update(arguments).await?;
            state.reload_user_packs();
            Ok(result)
        }
        "delete_mob" => {
            let result = mobs::handle_delete(arguments).await?;
            state.reload_user_packs();
            Ok(result)
        }
        _ => Err(ToolCallError::method_not_found(format!(
            "Unknown tool: {name}"
        ))),
    }
}
