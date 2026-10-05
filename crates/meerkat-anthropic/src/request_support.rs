//! Anthropic catalog-backed request-shaping helpers.
//!
//! Capability facts for Anthropic models live in the typed capability
//! catalog (`meerkat-models`). This module only exposes request-shaping
//! helpers for the Anthropic client; uncatalogued model IDs do not
//! synthesize semantic capabilities from name prefixes.

use meerkat_core::Provider;

/// Whether the model accepts a non-default `temperature`.
///
/// Catalog rows are authoritative. Unknown model IDs return `false` so callers
/// do not send optional provider parameters based on model-name folklore.
pub(crate) fn supports_temperature(model: &str) -> bool {
    meerkat_models::capabilities_for(Provider::Anthropic, model)
        .is_some_and(|caps| caps.supports_temperature)
}

/// Whether the model accepts System messages inside conversation history.
///
/// Catalog rows are authoritative. Unknown and older model IDs return
/// `false`, preserving the leading-system-prefix-only contract.
pub(crate) fn supports_mid_conversation_system_messages(model: &str) -> bool {
    meerkat_models::capabilities_for(Provider::Anthropic, model)
        .is_some_and(|caps| caps.supports_mid_conversation_system_messages)
}

/// Why the cataloged model refuses the knobs in `tag`, or `None` when it
/// accepts them. Uncatalogued model IDs return `None` (pass-through).
pub(crate) fn provider_tag_rejection(
    model: &str,
    tag: &meerkat_core::lifecycle::run_primitive::AnthropicProviderTag,
) -> Option<String> {
    meerkat_models::capabilities_for(Provider::Anthropic, model)?
        .anthropic_provider_tag_rejection(tag)
}

/// Why a forced tool choice (`any` or a named tool) is refused locally for
/// this request, or `None` to send it: explicit thinking (Anthropic rejects
/// a forced call under extended thinking), or a cataloged model proven to
/// reject forced choices. Unproven and uncatalogued models send it.
pub(crate) fn forced_tool_choice_refusal(
    model: &str,
    tag: Option<&meerkat_core::lifecycle::run_primitive::AnthropicProviderTag>,
) -> Option<meerkat_llm_core::ToolChoiceRefusal> {
    if tag.is_some_and(|tag| tag.thinking.is_some() || tag.thinking_budget_tokens.is_some()) {
        return Some(meerkat_llm_core::ToolChoiceRefusal::ForcedToolWithThinking);
    }
    meerkat_models::capabilities_for(Provider::Anthropic, model)
        .is_some_and(|caps| !caps.supports_forced_tool_choice)
        .then_some(meerkat_llm_core::ToolChoiceRefusal::ModelDoesNotSupportForcedToolChoice)
}

/// The typed refusal for Anthropic's own rejection of a forced tool choice:
/// a 400 `invalid_request_error` whose message starts `tool_choice:` and says
/// the choice is not supported, on a request that forced a call. Anything
/// else stays the generic HTTP classification.
pub(crate) fn provider_forced_tool_choice_rejection(
    request: &meerkat_llm_core::LlmRequest,
    status: u16,
    body: &str,
) -> Option<meerkat_llm_core::LlmError> {
    if status != 400 || !request.tool_choice.forces_a_tool_call() {
        return None;
    }
    let body: serde_json::Value = serde_json::from_str(body).ok()?;
    let error = body.get("error")?;
    let message = error.get("message")?.as_str()?;
    (error.get("type")?.as_str()? == "invalid_request_error"
        && message.starts_with("tool_choice:")
        && message.contains("not supported"))
    .then(|| meerkat_llm_core::LlmError::ToolChoiceUnsupported {
        provider: "anthropic".to_owned(),
        choice: request.tool_choice.clone(),
        reason: meerkat_llm_core::ToolChoiceRefusal::ModelDoesNotSupportForcedToolChoice,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn supports_temperature_uses_catalog_rows() {
        assert!(!supports_temperature("claude-opus-4-8"));
        assert!(!supports_temperature("claude-sonnet-5-5"));
        assert!(!supports_temperature("claude-sonnet-5"));
        assert!(supports_temperature("claude-sonnet-4-6"));
    }

    #[test]
    fn supports_temperature_unknown_model_is_conservative() {
        assert!(!supports_temperature("claude-opus-4-8-20260501-preview"));
        assert!(!supports_temperature("claude-future-5"));
    }

    #[test]
    fn mid_conversation_system_messages_use_catalog_rows() {
        assert!(supports_mid_conversation_system_messages("claude-fable-5"));
        assert!(supports_mid_conversation_system_messages("claude-opus-5"));
        assert!(supports_mid_conversation_system_messages("claude-opus-4-8"));
        assert!(supports_mid_conversation_system_messages(
            "claude-sonnet-5-5"
        ));
        // Sonnet 5 keeps instructions in the top-level system field.
        assert!(!supports_mid_conversation_system_messages(
            "claude-sonnet-5"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-haiku-4-5-20251001"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-haiku-4-5"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-opus-4-7"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-sonnet-4-6"
        ));
    }

    #[test]
    fn mid_conversation_system_messages_unknown_model_is_conservative() {
        assert!(!supports_mid_conversation_system_messages(
            "claude-opus-4-8-20260501-preview"
        ));
        assert!(!supports_mid_conversation_system_messages(
            "claude-future-5"
        ));
    }
}

/// Request shaping for Claude Sonnet 5.5, which answers 400 to thinking
/// `disabled`, thinking budgets, and non-default sampling parameters. Real
/// request bodies are built through the client's lowering (and, for the
/// portable per-turn overrides, through the adapter a session uses) and must
/// carry none of those, or fail with a typed local refusal.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod sonnet_5_5_request_shaping {
    use std::sync::Arc;

    use meerkat_core::lifecycle::run_primitive::{
        AnthropicEffort, AnthropicProviderTag, AnthropicThinkingConfig, ProviderParamsOverride,
        ProviderTag,
    };
    use meerkat_core::{AgentLlmClient, Message, UserMessage};
    use meerkat_llm_core::{LlmClientAdapter, LlmError, LlmRequest};
    use serde_json::Value;

    use crate::AnthropicClient;

    const MODEL: &str = "claude-sonnet-5-5";

    fn messages() -> Vec<Message> {
        vec![Message::User(UserMessage::text("hello"))]
    }

    fn body(
        model: &str,
        temperature: Option<f32>,
        tag: Option<AnthropicProviderTag>,
    ) -> Result<Value, LlmError> {
        let client = AnthropicClient::new("test-key".to_string()).unwrap();
        let mut request = LlmRequest::new(model, messages());
        request.temperature = temperature;
        request.provider_params = tag.map(ProviderTag::Anthropic);
        client.build_request_body(&request)
    }

    fn refusal(result: Result<Value, LlmError>) -> String {
        match result {
            Err(LlmError::InvalidRequest { message }) => message,
            other => panic!("expected a typed local refusal, got {other:?}"),
        }
    }

    /// No disabled thinking, no budget, no sampling parameters.
    fn assert_clean(body: &Value) {
        assert_ne!(body["thinking"]["type"], "disabled", "{body}");
        assert_ne!(body["thinking"]["type"], "enabled", "{body}");
        assert!(body["thinking"].get("budget_tokens").is_none(), "{body}");
        for key in ["temperature", "top_p", "top_k"] {
            assert!(body.get(key).is_none(), "{key} must not be sent: {body}");
        }
    }

    #[test]
    fn default_config_sends_no_thinking_override_and_no_sampling() {
        let body = body(MODEL, None, None).unwrap();
        assert_clean(&body);
        assert!(
            body.get("thinking").is_none(),
            "the provider default (adaptive) applies"
        );
    }

    #[test]
    fn generic_temperature_is_omitted_not_sent() {
        let body = body(MODEL, Some(0.7), None).unwrap();
        assert_clean(&body);
    }

    #[test]
    fn adaptive_and_between_tools_lower_to_their_wire_types() {
        let adaptive = body(
            MODEL,
            None,
            Some(AnthropicProviderTag {
                thinking: Some(AnthropicThinkingConfig::Adaptive),
                ..Default::default()
            }),
        )
        .unwrap();
        assert_clean(&adaptive);
        assert_eq!(
            adaptive["thinking"],
            serde_json::json!({"type": "adaptive"})
        );

        for effort in [
            None,
            Some(AnthropicEffort::Low),
            Some(AnthropicEffort::High),
        ] {
            let between = body(
                MODEL,
                None,
                Some(AnthropicProviderTag {
                    thinking: Some(AnthropicThinkingConfig::BetweenTools),
                    effort,
                    ..Default::default()
                }),
            )
            .unwrap();
            assert_clean(&between);
            assert_eq!(
                between["thinking"],
                serde_json::json!({"type": "between_tools"}),
                "between_tools takes no other field"
            );
        }
    }

    #[test]
    fn between_tools_above_high_effort_is_refused_locally() {
        for effort in [AnthropicEffort::XHigh, AnthropicEffort::Max] {
            let message = refusal(body(
                MODEL,
                None,
                Some(AnthropicProviderTag {
                    thinking: Some(AnthropicThinkingConfig::BetweenTools),
                    effort: Some(effort),
                    ..Default::default()
                }),
            ));
            assert!(message.contains("'high' effort or below"), "{message}");
        }
    }

    #[test]
    fn explicit_thinking_budget_is_refused_locally() {
        let message = refusal(body(
            MODEL,
            None,
            Some(AnthropicProviderTag {
                thinking: Some(AnthropicThinkingConfig::Enabled {
                    budget_tokens: 4096,
                }),
                ..Default::default()
            }),
        ));
        assert!(message.contains("thinking type 'enabled'"), "{message}");
        assert!(message.contains("adaptive, between_tools"), "{message}");

        let message = refusal(body(
            MODEL,
            None,
            Some(AnthropicProviderTag {
                thinking_budget_tokens: Some(4096),
                ..Default::default()
            }),
        ));
        assert!(message.contains("thinking_budget_tokens"), "{message}");
    }

    #[test]
    fn top_k_is_refused_locally() {
        let message = refusal(body(
            MODEL,
            None,
            Some(AnthropicProviderTag {
                top_k: Some(40),
                ..Default::default()
            }),
        ));
        assert!(message.contains("top_k"), "{message}");
    }

    #[test]
    fn between_tools_is_refused_for_models_that_do_not_accept_it() {
        for model in ["claude-opus-5-5", "claude-fable-5-1", "claude-sonnet-4-6"] {
            let message = refusal(body(
                model,
                None,
                Some(AnthropicProviderTag {
                    thinking: Some(AnthropicThinkingConfig::BetweenTools),
                    ..Default::default()
                }),
            ));
            assert!(message.contains("'between_tools'"), "{model}: {message}");
        }
        // Opus 5.5 carries the same adaptive-only constraints as before.
        assert!(
            body(
                "claude-opus-5-5",
                None,
                Some(AnthropicProviderTag {
                    thinking_budget_tokens: Some(4096),
                    ..Default::default()
                }),
            )
            .is_err()
        );
    }

    /// The portable per-turn overrides (what mob profiles, skills, and turn
    /// requests carry) reach the Anthropic lowering through the session
    /// adapter. `top_p` and `reasoning` have no Anthropic mapping and are
    /// never sent; a generic thinking budget is refused locally.
    #[test]
    fn generic_overrides_through_the_adapter_never_reach_the_provider_unsupported() {
        let adapter = LlmClientAdapter::try_for_provider_identity(
            Arc::new(AnthropicClient::new("test-key".to_string()).unwrap()),
            MODEL.to_string(),
            meerkat_core::Provider::Anthropic,
        )
        .unwrap();
        let accepted = ProviderParamsOverride {
            temperature: Some(0.7),
            top_p: Some(0.9),
            reasoning: Some(meerkat_core::lifecycle::run_primitive::ReasoningMode::Off),
            ..Default::default()
        };
        adapter
            .request_pressure(&messages(), &[], 1024, None, Some(&accepted))
            .expect("unmapped portable knobs are omitted, not refused");

        let budget = ProviderParamsOverride {
            thinking_budget_tokens: Some(4096),
            ..Default::default()
        };
        let error = adapter
            .request_pressure(&messages(), &[], 1024, None, Some(&budget))
            .expect_err("a generic thinking budget is refused before the provider call");
        assert!(
            error.to_string().contains("thinking_budget_tokens"),
            "{error}"
        );
    }
}
