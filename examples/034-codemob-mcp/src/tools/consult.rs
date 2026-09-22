use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

use meerkat::surface::RequestContext;
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, StartTurnRequest,
    StartTurnRuntimeSemantics,
};
use meerkat_core::types::SessionId;

use super::ToolCallError;
use crate::state::ForceState;

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
    let provider_params = input
        .provider_params
        .map(serde_json::from_value::<meerkat_core::ProviderParamsOverride>)
        .transpose()
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid provider_params: {e}")))?;
    if input.session_id.is_some() && provider_params.is_some() {
        return Err(ToolCallError::invalid_params(
            "provider_params is supported only for new consult sessions; continuation inherits the original configuration",
        ));
    }

    let prompt = match &input.context {
        Some(ctx) if !ctx.is_empty() => format!("{}\n\n## Context\n\n{ctx}", input.question),
        _ => input.question,
    };

    let model = input.model.unwrap_or_else(|| "gpt-5.5".to_string());

    // If session_id is provided, continue the existing session with start_turn.
    // Model, system_prompt, and shell are inherited from the original session.
    if let Some(ref sid) = input.session_id {
        let session_id = SessionId::parse(sid)
            .map_err(|e| ToolCallError::invalid_params(format!("Invalid session_id: {e}")))?;

        let signal = super::cancellation::cancellation_signal(request_context.as_ref()).await?;

        let req = StartTurnRequest {
            injected_context: Vec::new(),
            prompt: prompt.into(),
            system_prompt: None,
            event_tx: None,
            runtime: StartTurnRuntimeSemantics::default(),
        };

        let result =
            super::cancellation::run_turn(&state.session_service, &session_id, req, &signal)
                .await?;

        return Ok(json!({
            "content": [
                {"type": "text", "text": result.text},
                {"type": "text", "text": format!("\n\n---\nsession_id: {session_id}")}
            ]
        }));
    }

    // New session path.
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
        override_shell: meerkat_core::ToolCategoryOverride::from_override(input.shell),
        additional_instructions,
        runtime_build_mode: meerkat_core::RuntimeBuildMode::StandaloneEphemeral,
        initial_turn_metadata: None,
        ..SessionBuildOptions::default()
    };
    build.provider_params = provider_params;

    let req = CreateSessionRequest {
        injected_context: Vec::new(),
        model,
        prompt: "".into(),
        system_prompt: meerkat_core::SystemPromptOverride::Set(system_prompt),
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(build),
        labels: Some(labels),
    };

    let turn = StartTurnRequest {
        injected_context: Vec::new(),
        prompt: prompt.into(),
        system_prompt: None,
        event_tx: None,
        runtime: StartTurnRuntimeSemantics::default(),
    };
    let result = super::cancellation::create_and_run(
        &state.session_service,
        req,
        turn,
        request_context.as_ref(),
    )
    .await?;
    let session_id = &result.session_id;

    Ok(json!({
        "content": [
            {"type": "text", "text": result.text},
            {"type": "text", "text": format!("\n\n---\nsession_id: {session_id}")}
        ]
    }))
}
