use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};

use meerkat::surface::{RequestContext, request_action};
use meerkat::SessionService;
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, StartTurnRequest,
};
use meerkat_core::types::{HandlingMode, SessionId};
use meerkat_core::Session;

use crate::state::ForceState;
use super::ToolCallError;

const DEFAULT_SYSTEM_PROMPT: &str =
    "You are a helpful technical advisor. Give clear, concise opinions. \
     Be direct about trade-offs and risks. If you disagree with the approach, say so.";

#[derive(Deserialize)]
struct ConsultInput {
    question: String,
    context: Option<String>,
    model: Option<String>,
    /// Custom system prompt / persona for this agent.
    system_prompt: Option<String>,
    /// Enable/disable shell access for this agent.
    shell: Option<bool>,
    provider_params: Option<Value>,
    /// Continue an existing session instead of starting fresh.
    session_id: Option<String>,
    /// Skill names to load into this agent's context.
    skills: Option<Vec<String>>,
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
        .unwrap_or_else(|| "gpt-5.4".to_string());

    // If session_id is provided, continue the existing session with start_turn.
    // Model, system_prompt, and shell are inherited from the original session.
    if let Some(ref sid) = input.session_id {
        let session_id = SessionId::parse(sid)
            .map_err(|e| ToolCallError::invalid_params(format!("Invalid session_id: {e}")))?;

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
            let _ = context.run_cancel_if_requested().await;
        }

        let req = StartTurnRequest {
            prompt: prompt.into(),
            system_prompt: None,
            render_metadata: None,
            handling_mode: HandlingMode::Queue,
            event_tx: None,
            skill_references: None,
            flow_tool_overlay: None,
            additional_instructions: None,
        };

        let result = state
            .session_service
            .start_turn(&session_id, req)
            .await
            .map_err(|e| ToolCallError::internal(format!("Session error: {e}")))?;

        return Ok(json!({
            "content": [
                {"type": "text", "text": result.text},
                {"type": "text", "text": format!("\n\n---\nsession_id: {session_id}")}
            ]
        }));
    }

    // New session path.
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
        // No archive cleanup — session stays alive for continuation via session_id.
        let _ = context.run_cancel_if_requested().await;
    }

    let system_prompt = input
        .system_prompt
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());

    let mut labels = BTreeMap::new();
    labels.insert("source".into(), "consult".into());
    labels.insert("model".into(), model.clone());

    let additional_instructions = input
        .skills
        .as_deref()
        .map(crate::state::resolve_skills)
        .filter(|v| !v.is_empty());

    let mut build = SessionBuildOptions {
        resume_session: Some(session),
        override_shell: meerkat_core::ToolCategoryOverride::from_override(input.shell),
        additional_instructions,
        runtime_build_mode: meerkat_core::RuntimeBuildMode::StandaloneEphemeral,
        ..SessionBuildOptions::default()
    };
    build.provider_params = input.provider_params;

    let req = CreateSessionRequest {
        model,
        prompt: prompt.into(),
        render_metadata: None,
        system_prompt: Some(system_prompt),
        max_tokens: None,
        event_tx: None,
        skill_references: None,
        initial_turn: InitialTurnPolicy::RunImmediately,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(build),
        labels: Some(labels),
    };

    let result = state
        .session_service
        .create_session(req)
        .await
        .map_err(|e| ToolCallError::internal(format!("Session error: {e}")))?;

    Ok(json!({
        "content": [
            {"type": "text", "text": result.text},
            {"type": "text", "text": format!("\n\n---\nsession_id: {session_id}")}
        ]
    }))
}
