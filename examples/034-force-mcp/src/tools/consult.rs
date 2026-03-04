use serde::Deserialize;
use serde_json::{Value, json};

use meerkat::SessionService;
use meerkat_core::service::{CreateSessionRequest, InitialTurnPolicy};

use crate::state::ForceState;
use super::ToolCallError;

#[derive(Deserialize)]
struct ConsultInput {
    question: String,
    context: Option<String>,
    model: Option<String>,
}

pub async fn handle(state: &ForceState, arguments: &Value) -> Result<Value, ToolCallError> {
    let input: ConsultInput = serde_json::from_value(arguments.clone())
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;

    let prompt = match &input.context {
        Some(ctx) if !ctx.is_empty() => format!("{}\n\n## Context\n\n{ctx}", input.question),
        _ => input.question,
    };

    let model = input
        .model
        .unwrap_or_else(|| "claude-sonnet-4-5".to_string());

    let req = CreateSessionRequest {
        model,
        prompt,
        system_prompt: Some(
            "You are a helpful technical advisor. Give clear, concise opinions. \
             Be direct about trade-offs and risks. If you disagree with the approach, say so."
                .to_string(),
        ),
        max_tokens: None,
        event_tx: None,
        host_mode: false,
        skill_references: None,
        initial_turn: InitialTurnPolicy::RunImmediately,
        build: None,
        labels: None,
    };

    let result = state
        .session_service
        .create_session(req)
        .await
        .map_err(|e| ToolCallError::internal(format!("Session error: {e}")))?;

    Ok(json!({
        "content": [{"type": "text", "text": result.text}]
    }))
}
