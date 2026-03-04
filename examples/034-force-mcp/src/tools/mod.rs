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

pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "list_packs",
            "description": "List all available deliberation packs with their descriptions and agent counts.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "consult",
            "description": "Get a quick opinion from a single AI agent. No coordination overhead — like asking a colleague. Use this when you want a second opinion, a sanity check, or a fresh perspective on something specific.",
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
                        "description": "Model to use (default: claude-sonnet-4-5)"
                    }
                },
                "required": ["question"]
            }
        }),
        json!({
            "name": "deliberate",
            "description": "Spawn a team of AI agents to collaboratively solve a problem. Each pack defines a different team composition optimized for specific tasks. Returns structured results after agents deliberate.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pack": {
                        "type": "string",
                        "enum": ["advisor", "review", "architect", "brainstorm", "red-team", "rct"],
                        "description": "Team pack to use. advisor=quick opinion, review=parallel code review, architect=design deliberation, brainstorm=multi-model ideation, red-team=balanced risk assessment, rct=full RCT implementation pipeline"
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
                        "description": "Override models per role, e.g. {\"critic\": \"claude-opus-4-6\", \"planner\": \"gpt-5.2\"}"
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
