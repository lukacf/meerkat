use serde::Deserialize;
use serde_json::{Value, json};

use meerkat::surface::{RequestContext, request_action};
use meerkat::SessionService;
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::Session;

use crate::state::ForceState;
use super::ToolCallError;

#[derive(Deserialize)]
struct ConsultInput {
    question: String,
    context: Option<String>,
    model: Option<String>,
    provider_params: Option<Value>,
}

pub async fn handle(
    state: &ForceState,
    arguments: &Value,
    request_context: Option<RequestContext>,
) -> Result<Value, ToolCallError> {
    let input: ConsultInput = serde_json::from_value(arguments.clone())
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;

    let prompt = match &input.context {
        Some(ctx) if !ctx.is_empty() => format!("{}\n\n## Context\n\n{ctx}", input.question),
        _ => input.question,
    };

    let model = input
        .model
        .unwrap_or_else(|| "gpt-5.3-codex".to_string());

    let session = Session::new();
    let session_id = session.id().clone();

    if let Some(context) = request_context.as_ref() {
        let service = state.session_service.clone();
        let session_id_for_cancel = session_id.clone();
        context.replace_cancel_action(request_action(move || {
            let service = service.clone();
            let session_id = session_id_for_cancel.clone();
            async move {
                let _ = service.interrupt(&session_id).await;
            }
        }));
        let service = state.session_service.clone();
        let session_id_for_cleanup = session_id.clone();
        context.set_unpublished_cleanup(request_action(move || {
            let service = service.clone();
            let session_id = session_id_for_cleanup.clone();
            async move {
                let _ = service.archive(&session_id).await;
            }
        }));
        let _ = context.run_cancel_if_requested().await;
    }

    let mut build = SessionBuildOptions {
        resume_session: Some(session),
        ..SessionBuildOptions::default()
    };
    build.provider_params = input.provider_params;

    let req = CreateSessionRequest {
        model,
        prompt: prompt.into(),
        render_metadata: None,
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
        build: Some(build),
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
