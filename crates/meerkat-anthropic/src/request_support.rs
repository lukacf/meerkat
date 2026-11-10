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
    if tag.is_some_and(turns_thinking_on) {
        return Some(meerkat_llm_core::ToolChoiceRefusal::ForcedToolWithThinking);
    }
    meerkat_models::capabilities_for(Provider::Anthropic, model)
        .is_some_and(|caps| !caps.supports_forced_tool_choice)
        .then_some(meerkat_llm_core::ToolChoiceRefusal::ModelDoesNotSupportForcedToolChoice)
}

/// Whether `tag` turns thinking on explicitly, as the lowering sends it: a
/// thinking type other than `disabled`, or, with no type, the legacy flat
/// budget.
fn turns_thinking_on(tag: &meerkat_core::lifecycle::run_primitive::AnthropicProviderTag) -> bool {
    use meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig as Thinking;
    match tag.thinking {
        Some(Thinking::Disabled) => false,
        Some(Thinking::Adaptive | Thinking::Enabled { .. } | Thinking::BetweenTools) => true,
        None => tag.thinking_budget_tokens.is_some(),
    }
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
        assert!(!supports_temperature("claude-haiku-5-5"));
        assert!(supports_temperature("claude-sonnet-4-6"));
    }

    #[test]
    fn supports_temperature_unknown_model_is_conservative() {
        assert!(!supports_temperature("claude-opus-4-8-20260501-preview"));
        assert!(!supports_temperature("claude-future-5"));
    }

    #[test]
    fn mid_conversation_system_messages_use_catalog_rows() {
        assert!(supports_mid_conversation_system_messages(
            "claude-fable-5-2"
        ));
        assert!(supports_mid_conversation_system_messages("claude-fable-5"));
        assert!(supports_mid_conversation_system_messages("claude-opus-5"));
        assert!(supports_mid_conversation_system_messages("claude-opus-4-8"));
        assert!(supports_mid_conversation_system_messages(
            "claude-sonnet-5-5"
        ));
        assert!(supports_mid_conversation_system_messages(
            "claude-haiku-5-5"
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

    /// The model's own default, adaptive, named beside the thinking-block
    /// binding control (Sonnet 5.5 and Haiku 5.5 think by default and check
    /// replayed thinking blocks against their request prefix).
    fn adaptive_with_binding() -> Value {
        serde_json::json!({
            "type": "adaptive",
            "block_binding": {"prefix_mismatch_behavior": "drop_block"}
        })
    }

    #[test]
    fn default_config_sends_no_thinking_override_and_no_sampling() {
        let body = body(MODEL, None, None).unwrap();
        assert_clean(&body);
        assert_eq!(
            body["thinking"],
            adaptive_with_binding(),
            "the provider default (adaptive) is named beside the binding control"
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
        assert_eq!(adaptive["thinking"], adaptive_with_binding());

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
    fn disabled_thinking_is_refused_locally_never_substituted() {
        for effort in [None, Some(AnthropicEffort::Low)] {
            let message = refusal(body(
                MODEL,
                None,
                Some(AnthropicProviderTag {
                    thinking: Some(AnthropicThinkingConfig::Disabled),
                    effort,
                    ..Default::default()
                }),
            ));
            assert!(
                message.contains(
                    "does not accept thinking type 'disabled'; accepted: [adaptive, between_tools]"
                ),
                "{message}"
            );
        }
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
        for model in [
            "claude-opus-5-5",
            "claude-fable-5-2",
            "claude-fable-5-1",
            "claude-sonnet-4-6",
        ] {
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

    /// Claude Haiku 5.5 shares the adaptive-only request surface of its
    /// catalog row: no sampling parameters, no thinking budget, no
    /// `between_tools`, no `top_k`.
    #[test]
    fn haiku_5_5_request_shaping_follows_its_catalog_row() {
        let model = "claude-haiku-5-5";
        let default = body(model, Some(0.7), None).unwrap();
        assert_clean(&default);
        assert_eq!(
            default["thinking"],
            adaptive_with_binding(),
            "the provider default (adaptive) is named beside the binding control"
        );

        let message = refusal(body(
            model,
            None,
            Some(AnthropicProviderTag {
                thinking_budget_tokens: Some(4096),
                ..Default::default()
            }),
        ));
        assert!(message.contains("thinking_budget_tokens"), "{message}");

        let message = refusal(body(
            model,
            None,
            Some(AnthropicProviderTag {
                thinking: Some(AnthropicThinkingConfig::BetweenTools),
                ..Default::default()
            }),
        ));
        assert!(message.contains("'between_tools'"), "{message}");

        let message = refusal(body(
            model,
            None,
            Some(AnthropicProviderTag {
                top_k: Some(40),
                ..Default::default()
            }),
        ));
        assert!(message.contains("top_k"), "{message}");
    }

    /// Claude Fable 5.2 follows its catalog row: thinking omitted names the
    /// always-on adaptive default beside the binding control, a generic
    /// temperature is omitted, each of the five effort levels lowers to
    /// `output_config.effort`, and a thinking budget, `enabled`, `disabled`,
    /// `between_tools` and `top_k` are refused locally.
    #[test]
    fn fable_5_2_request_shaping_follows_its_catalog_row() {
        let model = "claude-fable-5-2";
        let default = body(model, Some(0.7), None).unwrap();
        assert_clean(&default);
        assert_eq!(
            default["thinking"],
            adaptive_with_binding(),
            "the always-on adaptive default is named beside the binding control"
        );
        assert!(
            default.get("output_config").is_none(),
            "no effort set, the API default applies: {default}"
        );

        for (effort, wire) in [
            (AnthropicEffort::Low, "low"),
            (AnthropicEffort::Medium, "medium"),
            (AnthropicEffort::High, "high"),
            (AnthropicEffort::XHigh, "xhigh"),
            (AnthropicEffort::Max, "max"),
        ] {
            let sent = body(
                model,
                None,
                Some(AnthropicProviderTag {
                    effort: Some(effort),
                    ..Default::default()
                }),
            )
            .unwrap();
            assert_clean(&sent);
            assert_eq!(sent["output_config"]["effort"], wire, "{sent}");
            assert_eq!(sent["thinking"], adaptive_with_binding(), "{sent}");
        }

        for (tag, refused) in [
            (
                AnthropicProviderTag {
                    thinking_budget_tokens: Some(4096),
                    ..Default::default()
                },
                "thinking_budget_tokens",
            ),
            (
                AnthropicProviderTag {
                    thinking: Some(AnthropicThinkingConfig::Enabled {
                        budget_tokens: 4096,
                    }),
                    ..Default::default()
                },
                "thinking type 'enabled'",
            ),
            (
                AnthropicProviderTag {
                    thinking: Some(AnthropicThinkingConfig::Disabled),
                    ..Default::default()
                },
                "thinking type 'disabled'",
            ),
            (
                AnthropicProviderTag {
                    thinking: Some(AnthropicThinkingConfig::BetweenTools),
                    ..Default::default()
                },
                "thinking type 'between_tools'",
            ),
            (
                AnthropicProviderTag {
                    top_k: Some(40),
                    ..Default::default()
                },
                "top_k",
            ),
        ] {
            let message = refusal(body(model, None, Some(tag)));
            assert!(
                message.contains(model) && message.contains(refused),
                "{message}"
            );
        }
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

/// `thinking: {"type": "disabled"}` across every cataloged Anthropic row:
/// lowered as is where the row accepts it, refused locally (never sent)
/// where it does not, and refused above `high` effort on rows with that
/// ceiling.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod disabled_thinking_request_shaping {
    use meerkat_core::lifecycle::run_primitive::{
        AnthropicEffort, AnthropicProviderTag, AnthropicThinkingConfig, ProviderTag,
    };
    use meerkat_core::model_profile::capabilities::{AnthropicDisabledThinking, EffortLevel};
    use meerkat_core::{Message, Provider, UserMessage};
    use meerkat_llm_core::{LlmError, LlmRequest};
    use serde_json::{Value, json};

    use crate::AnthropicClient;

    fn body(model: &str, effort: Option<AnthropicEffort>) -> Result<Value, LlmError> {
        let client = AnthropicClient::new("test-key".to_string()).unwrap();
        let mut request = LlmRequest::new(model, vec![Message::User(UserMessage::text("hello"))]);
        request.provider_params = Some(ProviderTag::Anthropic(AnthropicProviderTag {
            thinking: Some(AnthropicThinkingConfig::Disabled),
            effort,
            ..Default::default()
        }));
        client.build_request_body(&request)
    }

    fn refusal(model: &str, result: Result<Value, LlmError>) -> String {
        match result {
            Err(LlmError::InvalidRequest { message }) => message,
            other => panic!("{model}: expected a typed local refusal, got {other:?}"),
        }
    }

    #[test]
    fn every_anthropic_row_lowers_or_refuses_disabled_thinking_as_cataloged() {
        let rows: Vec<_> = meerkat_models::capabilities::all_capabilities()
            .filter(|caps| caps.provider == Provider::Anthropic)
            .collect();
        assert!(rows.len() >= 14, "{} Anthropic rows", rows.len());
        for caps in rows {
            let model = caps.id;
            let level = |level: EffortLevel, effort: AnthropicEffort| {
                caps.effort_levels.contains(&level).then_some(effort)
            };
            let within: Vec<Option<AnthropicEffort>> = std::iter::once(None)
                .chain(level(EffortLevel::Low, AnthropicEffort::Low).map(Some))
                .chain(level(EffortLevel::High, AnthropicEffort::High).map(Some))
                .collect();
            let above: Vec<AnthropicEffort> = [
                level(EffortLevel::Xhigh, AnthropicEffort::XHigh),
                level(EffortLevel::Max, AnthropicEffort::Max),
            ]
            .into_iter()
            .flatten()
            .collect();
            match caps.anthropic_disabled_thinking {
                AnthropicDisabledThinking::Refused => {
                    for effort in within.into_iter().chain(above.into_iter().map(Some)) {
                        let message = refusal(model, body(model, effort));
                        assert!(
                            message.contains("does not accept thinking type 'disabled'"),
                            "{model}: {message}"
                        );
                    }
                }
                AnthropicDisabledThinking::AtOrBelowHighEffort
                | AnthropicDisabledThinking::AnyEffort => {
                    for effort in within {
                        let body = body(model, effort).unwrap();
                        assert_eq!(
                            body["thinking"],
                            json!({"type": "disabled"}),
                            "{model} {effort:?}: disabled takes no other field"
                        );
                    }
                    for effort in above {
                        let result = body(model, Some(effort));
                        if caps.anthropic_disabled_thinking
                            == AnthropicDisabledThinking::AtOrBelowHighEffort
                        {
                            let message = refusal(model, result);
                            assert!(
                                message.contains(
                                    "accepts thinking type 'disabled' only at 'high' effort or below"
                                ),
                                "{model}: {message}"
                            );
                        } else {
                            assert_eq!(
                                result.unwrap()["thinking"],
                                json!({"type": "disabled"}),
                                "{model} {effort:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}
