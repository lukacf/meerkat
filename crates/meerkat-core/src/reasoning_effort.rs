//! Vocabulary of the provider-neutral `reasoning_effort` option.

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::provider_matrix::{
        AnthropicBackendKind, GoogleBackendKind, OpenAiBackendKind, SelfHostedBackendKind,
    };

    const ALL_APIS: [ModelApi; 5] = [
        ModelApi::Responses,
        ModelApi::ChatCompletions,
        ModelApi::Messages,
        ModelApi::GenerateContent,
        ModelApi::Realtime,
    ];

    /// The refusal reasons are a closed set: the wire strings and their order
    /// are the contract. A new variant fails the exhaustive match below until
    /// this row (and every reader of the closed set) is updated with it.
    #[test]
    fn refusal_reasons_are_the_closed_set_in_order() {
        use ReasoningEffortRefusalReason as Reason;

        fn position(reason: Reason) -> usize {
            match reason {
                Reason::UnsupportedLevel => 0,
                Reason::SupportedLevelsUnknown => 1,
                Reason::SupportedLevelsUnavailable => 2,
                Reason::ConflictingProviderEffort => 3,
                Reason::ConflictingThinkingBudget => 4,
                Reason::ConflictingThinkingType => 5,
                Reason::ConflictingReasoningBody => 6,
                Reason::ConflictingReasoningMode => 7,
            }
        }

        let expected = [
            (Reason::UnsupportedLevel, "unsupported_level"),
            (Reason::SupportedLevelsUnknown, "supported_levels_unknown"),
            (
                Reason::SupportedLevelsUnavailable,
                "supported_levels_unavailable",
            ),
            (
                Reason::ConflictingProviderEffort,
                "conflicting_provider_effort",
            ),
            (
                Reason::ConflictingThinkingBudget,
                "conflicting_thinking_budget",
            ),
            (Reason::ConflictingThinkingType, "conflicting_thinking_type"),
            (
                Reason::ConflictingReasoningBody,
                "conflicting_reasoning_body",
            ),
            (
                Reason::ConflictingReasoningMode,
                "conflicting_reasoning_mode",
            ),
        ];
        assert_eq!(expected.len(), 8);
        for (index, (reason, wire)) in expected.into_iter().enumerate() {
            assert_eq!(position(reason), index, "{wire} is listed out of order");
            assert_eq!(
                reason as usize, index,
                "{wire} is declared out of order in the enum"
            );
            assert_eq!(
                serde_json::to_value(reason).expect("a reason serializes"),
                serde_json::json!(wire)
            );
            assert_eq!(
                serde_json::from_value::<Reason>(serde_json::json!(wire))
                    .expect("a reason's wire string reads back"),
                reason
            );
        }
        assert!(
            serde_json::from_value::<Reason>(serde_json::json!("some_other_reason")).is_err(),
            "a string outside the closed set is refused"
        );
    }

    /// A reported route names its backend with the provider matrix's own
    /// string for the kind, in every family and for every API.
    #[test]
    fn route_backend_strings_are_the_provider_matrix_strings() {
        let mut backends: Vec<(ReasoningBackend, &'static str, bool)> = Vec::new();
        for kind in OpenAiBackendKind::ALL {
            backends.push((
                ReasoningBackend::OpenAi(*kind),
                kind.as_str(),
                *kind == OpenAiBackendKind::Copilot,
            ));
        }
        for kind in AnthropicBackendKind::ALL {
            backends.push((
                ReasoningBackend::Anthropic(*kind),
                kind.as_str(),
                *kind == AnthropicBackendKind::Copilot,
            ));
        }
        for kind in GoogleBackendKind::ALL {
            backends.push((
                ReasoningBackend::Google(*kind),
                kind.as_str(),
                *kind == GoogleBackendKind::Copilot,
            ));
        }
        for kind in SelfHostedBackendKind::ALL {
            backends.push((ReasoningBackend::SelfHosted(*kind), kind.as_str(), false));
        }
        assert_eq!(backends.len(), 15, "every kind of the four families");

        for (backend, matrix_string, copilot) in backends {
            assert_eq!(backend.as_str(), matrix_string);
            assert_eq!(backend.is_copilot(), copilot, "{matrix_string}");
            for api in ALL_APIS {
                let route = ReasoningRoute::new(backend, api);
                assert_eq!(route.backend, matrix_string);
                assert_eq!(route.api, api);

                let observation = ReasoningRouteObservation {
                    backend,
                    api,
                    account: AccountLevelStatement::NotApplicable,
                    operator_levels: None,
                };
                assert_eq!(observation.route(), route);
            }
        }
    }

    /// A route serializes as the provider-matrix backend string and the
    /// snake_case API name, and reads back.
    #[test]
    fn route_serializes_backend_and_api() {
        let expected = [
            (ModelApi::Responses, "responses"),
            (ModelApi::ChatCompletions, "chat_completions"),
            (ModelApi::Messages, "messages"),
            (ModelApi::GenerateContent, "generate_content"),
            (ModelApi::Realtime, "realtime"),
        ];
        for (api, wire) in expected {
            assert_eq!(api.as_str(), wire);
            let route =
                ReasoningRoute::new(ReasoningBackend::OpenAi(OpenAiBackendKind::Copilot), api);
            let value = serde_json::to_value(&route).expect("a route serializes");
            assert_eq!(
                value,
                serde_json::json!({"backend": "copilot", "api": wire})
            );
            assert_eq!(
                serde_json::from_value::<ReasoningRoute>(value).expect("a route reads back"),
                route
            );
        }
    }
}
