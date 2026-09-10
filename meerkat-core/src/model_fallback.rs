//! Conservative admission for an optional model migration, not a dispatch gate.
//!
//! Forecasts may veto an automatic migration. They never refuse ordinary
//! dispatch on an operator-selected identity.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::{ModelFallbackPolicy, ModelFallbackTrigger};
use crate::error::{AgentError, LlmFailureReason, LlmProviderErrorKind};
use crate::lifecycle::run_primitive::{ProviderParamsOverride, ProviderTag};
use crate::{
    ContextBudgetFact, Message, ModelProfileWitness, OutputSchema, SessionLlmIdentity, ToolDef,
};

/// Exact materialized invocation at the generated recovery boundary.
#[derive(Clone, Copy)]
pub struct ModelFallbackRequest<'a> {
    pub messages: &'a [Message],
    pub tools: &'a [Arc<ToolDef>],
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub provider_params: Option<&'a ProviderParamsOverride>,
    pub output_schema: Option<&'a OutputSchema>,
    /// One-based failed attempt, from the machine-accepted retry schedule.
    pub attempt: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ModelFallbackSkipReason {
    ProviderBoundary,
    AuthUnavailable,
    ContextFit,
    ContextUnknown,
    OutputBudget,
    ToolParity,
    ModalityParity,
    RequestUnsupported,
    AdmissionUnavailable,
}

/// Evidence attached even when every configured candidate is rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelFallbackSkippedTarget {
    pub identity: SessionLlmIdentity,
    pub reason: ModelFallbackSkipReason,
    pub context: Option<ContextBudgetFact>,
}

impl ModelFallbackSkippedTarget {
    pub fn new(identity: SessionLlmIdentity, reason: ModelFallbackSkipReason) -> Self {
        Self {
            identity,
            reason,
            context: None,
        }
    }
}

/// Classify policy triggers only after generated recovery permits retry.
pub fn model_fallback_trigger(error: &AgentError) -> Option<ModelFallbackTrigger> {
    let AgentError::Llm { reason, .. } = error else {
        return None;
    };
    match reason {
        LlmFailureReason::RateLimited { .. } => Some(ModelFallbackTrigger::Capacity),
        LlmFailureReason::AuthError | LlmFailureReason::InvalidModel(_) => {
            Some(ModelFallbackTrigger::ProviderUnavailable)
        }
        LlmFailureReason::NetworkTimeout { .. }
        | LlmFailureReason::CallTimeout { .. }
        | LlmFailureReason::StreamStalled { .. } => Some(ModelFallbackTrigger::Transport),
        LlmFailureReason::ProviderError(error) => match error.kind {
            LlmProviderErrorKind::ServerOverloaded => Some(ModelFallbackTrigger::Capacity),
            LlmProviderErrorKind::ServerError => Some(ModelFallbackTrigger::ProviderUnavailable),
            LlmProviderErrorKind::ConnectionReset => Some(ModelFallbackTrigger::Transport),
            LlmProviderErrorKind::IncompleteResponse => Some(ModelFallbackTrigger::EmptyOutput),
            _ => None,
        },
        _ => None,
    }
}

/// Shared decision/commit/resume admission. The caller supplies the target's
/// exact lowered pressure when available; byte counts are never token counts.
pub fn admit_model_fallback(
    previous: &SessionLlmIdentity,
    target: &SessionLlmIdentity,
    profile: &ModelProfileWitness,
    policy: &ModelFallbackPolicy,
    request: &ModelFallbackRequest<'_>,
    pressure: Option<crate::ProviderRequestPressure>,
) -> Result<ContextBudgetFact, Box<ModelFallbackSkippedTarget>> {
    let skip = |reason| Box::new(ModelFallbackSkippedTarget::new(target.clone(), reason));
    if previous.provider != target.provider && !policy.cross_provider {
        return Err(skip(ModelFallbackSkipReason::ProviderBoundary));
    }
    if !profile.matches_identity(target) {
        return Err(skip(ModelFallbackSkipReason::AdmissionUnavailable));
    }
    let reserve = request
        .provider_params
        .and_then(|params| params.max_output_tokens)
        .unwrap_or(request.max_tokens);
    if profile
        .max_output_tokens()
        .is_none_or(|limit| reserve > limit)
    {
        return Err(skip(ModelFallbackSkipReason::OutputBudget));
    }
    let fact = match pressure {
        Some(pressure) => crate::context_budget_fact_for_provider_request(
            request.messages,
            request.tools,
            reserve,
            profile,
            pressure,
        ),
        None => crate::context_budget_fact_for_messages(
            request.messages,
            request.tools,
            reserve,
            profile,
        ),
    }
    .map_err(|_| skip(ModelFallbackSkipReason::ContextUnknown))?;
    let available = f64::from(fact.context_window_tokens) * (1.0 - policy.min_context_headroom);
    if !policy.min_context_headroom.is_finite()
        || !(0.0..1.0).contains(&policy.min_context_headroom)
        || fact.estimated_total_tokens as f64 > available
        || fact
            .max_input_tokens
            .is_some_and(|limit| fact.effective_input_tokens() > u64::from(limit))
    {
        return Err(Box::new(ModelFallbackSkippedTarget {
            identity: target.clone(),
            reason: ModelFallbackSkipReason::ContextFit,
            context: Some(fact),
        }));
    }
    let capabilities = profile.profile();
    for message in request.messages {
        match message {
            Message::User(user) => {
                if (crate::types::has_images(&user.content) && !capabilities.image_input)
                    || (crate::types::has_video(&user.content) && !capabilities.inline_video)
                {
                    return Err(skip(ModelFallbackSkipReason::ModalityParity));
                }
            }
            Message::ToolResults { results, .. } => {
                for result in results {
                    if (crate::types::has_images(&result.content)
                        && !capabilities.image_tool_results)
                        || (crate::types::has_video(&result.content) && !capabilities.inline_video)
                    {
                        return Err(skip(ModelFallbackSkipReason::ModalityParity));
                    }
                }
            }
            Message::SystemNotice(notice) => {
                for block in &notice.blocks {
                    if let crate::SystemNoticeBlock::Comms { content, .. }
                    | crate::SystemNoticeBlock::ExternalEvent { content, .. } = block
                        && ((crate::types::has_images(content) && !capabilities.image_input)
                            || (crate::types::has_video(content) && !capabilities.inline_video))
                    {
                        return Err(skip(ModelFallbackSkipReason::ModalityParity));
                    }
                }
            }
            Message::BlockAssistant(assistant) => {
                if !capabilities.image_input
                    && assistant
                        .blocks
                        .iter()
                        .any(|block| matches!(block, crate::AssistantBlock::Image { .. }))
                {
                    return Err(skip(ModelFallbackSkipReason::ModalityParity));
                }
            }
            Message::System(_) => {}
        }
    }
    if policy.require_tool_parity {
        let filter =
            crate::capability_base_filter_for_image_tool_results(capabilities.image_tool_results);
        if request.tools.iter().any(|tool| match &filter {
            crate::ToolFilter::All => false,
            crate::ToolFilter::Allow(names) => !names.contains(tool.name.as_ref()),
            crate::ToolFilter::Deny(names) => names.contains(tool.name.as_ref()),
        }) {
            return Err(skip(ModelFallbackSkipReason::ToolParity));
        }
        if request.provider_params.is_some_and(has_native_search)
            && !capabilities.supports_web_search
        {
            return Err(skip(ModelFallbackSkipReason::ToolParity));
        }
    }
    Ok(fact)
}

pub fn has_native_search(params: &ProviderParamsOverride) -> bool {
    match params.provider_tag.as_ref() {
        Some(ProviderTag::Anthropic(tag)) => tag.web_search.is_some(),
        Some(ProviderTag::OpenAi(tag)) => tag.web_search.is_some(),
        Some(ProviderTag::Gemini(tag)) => tag.google_search.is_some(),
        _ => false,
    }
}

pub fn structured_output(params: &ProviderParamsOverride) -> Option<&OutputSchema> {
    match params.provider_tag.as_ref() {
        Some(ProviderTag::Anthropic(tag)) => tag.structured_output.as_ref(),
        Some(ProviderTag::OpenAi(tag)) => tag.structured_output.as_ref(),
        Some(ProviderTag::Gemini(tag)) => tag.structured_output.as_ref(),
        _ => None,
    }
}

/// Consume the exact credential identity resolved by the factory; never infer
/// a binding or create an absent lease to make an automatic route admissible.
pub fn fallback_credential_authorized(
    authority: Option<&crate::handles::GeneratedAuthLeaseHandle>,
    identity: Option<&crate::AuthCredentialIdentity>,
) -> Result<bool, AgentError> {
    let Some(identity) = identity else {
        return Ok(true);
    };
    let Some(authority) = authority else {
        return Ok(false);
    };
    let key = crate::handles::LeaseKey::from_credential_identity(identity);
    authority
        .as_handle()
        .resolve_credential_use_admission(&key, crate::handles::CredentialUseIntent::UseCredential)
        .map(|disposition| {
            matches!(
                disposition,
                crate::handles::CredentialUseDisposition::Authorized
            )
        })
        .map_err(|error| {
            AgentError::ConfigError(format!(
                "fallback credential-use authority rejected {key}: {error}"
            ))
        })
}

/// History of a newly committed fallback, not a second serving identity.
/// Applies only while `target` exactly equals the canonical active identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelFallbackProvenance {
    pub previous: SessionLlmIdentity,
    pub target: SessionLlmIdentity,
    pub policy: ModelFallbackPolicy,
}
