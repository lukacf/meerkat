//! Vocabulary of the provider-neutral `reasoning_effort` option.
//!
//! Types only. A request names one level in the [`EffortLevel`] vocabulary;
//! whether a provider request can carry it depends on the model and on the
//! route that serves the request: the backend (a provider-matrix backend
//! kind) and the provider API the request is sent on. This module names the
//! route, what a client observes about it, and the reasons a level is
//! refused. It decides nothing.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::model_profile::capabilities::EffortLevel;
use crate::provider_matrix::{
    AnthropicBackendKind, GoogleBackendKind, OpenAiBackendKind, SelfHostedBackendKind,
};

/// The provider API a request is sent on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ModelApi {
    /// The OpenAI Responses API.
    Responses,
    /// The OpenAI Chat Completions API and endpoints compatible with it.
    ChatCompletions,
    /// The Anthropic Messages API.
    Messages,
    /// The Gemini `generateContent` API.
    GenerateContent,
    /// The OpenAI realtime API.
    Realtime,
}

impl ModelApi {
    /// The serialized name of this API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletions => "chat_completions",
            Self::Messages => "messages",
            Self::GenerateContent => "generate_content",
            Self::Realtime => "realtime",
        }
    }
}

impl fmt::Display for ModelApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The backend that serves a request: a provider-matrix backend kind of one
/// of the four provider families.
///
/// Typed and never serialized. The reported form is [`ReasoningRoute`], which
/// carries the kind's provider-matrix string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReasoningBackend {
    OpenAi(OpenAiBackendKind),
    Anthropic(AnthropicBackendKind),
    Google(GoogleBackendKind),
    SelfHosted(SelfHostedBackendKind),
}

impl ReasoningBackend {
    /// The provider-matrix string of the backend kind (`openai_api`,
    /// `copilot`, `bedrock`, ...): the kind's own `as_str()`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi(kind) => kind.as_str(),
            Self::Anthropic(kind) => kind.as_str(),
            Self::Google(kind) => kind.as_str(),
            Self::SelfHosted(kind) => kind.as_str(),
        }
    }

    /// Whether this is the Copilot backend kind of its family.
    pub fn is_copilot(self) -> bool {
        match self {
            Self::OpenAi(kind) => kind == OpenAiBackendKind::Copilot,
            Self::Anthropic(kind) => kind == AnthropicBackendKind::Copilot,
            Self::Google(kind) => kind == GoogleBackendKind::Copilot,
            Self::SelfHosted(_) => false,
        }
    }
}

/// The route a request is served on, in its reported form:
/// `{"backend": "<provider-matrix string>", "api": "<snake_case API>"}`.
///
/// Built only by [`ReasoningRoute::new`] from a typed backend, so `backend`
/// is always a provider-matrix backend string. Displays as `backend/api`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct ReasoningRoute {
    /// The provider-matrix backend string (`openai_api`, `copilot`,
    /// `bedrock`, ...).
    pub backend: String,
    /// The provider API of the route.
    pub api: ModelApi,
}

impl ReasoningRoute {
    /// The reported form of a typed backend and an API.
    pub fn new(backend: ReasoningBackend, api: ModelApi) -> Self {
        Self {
            backend: backend.as_str().to_owned(),
            api,
        }
    }
}

impl fmt::Display for ReasoningRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.backend, self.api)
    }
}

/// What the account behind a route states about the effort levels of the
/// selected model.
///
/// Typed and never serialized: it is what a client observes, not a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountLevelStatement {
    /// The route has no account list.
    NotApplicable,
    /// The route has an account list and it is not loaded.
    ListUnavailable,
    /// The account's list is loaded and the model's entry states no levels.
    NotStated,
    /// The levels the model's entry states.
    Stated(Vec<EffortLevel>),
}

/// What a client observes, without I/O, about the route it serves requests
/// on: the backend, the API, the account's statement about the selected
/// model's effort levels, and the levels the operator declared for the model.
///
/// Typed and never serialized. [`ReasoningRouteObservation::route`] gives the
/// reported form of the route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningRouteObservation {
    /// The backend that serves the client's requests.
    pub backend: ReasoningBackend,
    /// The provider API the client sends its requests on.
    pub api: ModelApi,
    /// What the account's model list states for the selected model.
    pub account: AccountLevelStatement,
    /// The effort levels the operator declared for the selected model, when
    /// the operator's configuration declares any.
    pub operator_levels: Option<Vec<EffortLevel>>,
}

impl ReasoningRouteObservation {
    /// The reported form of the observed route.
    pub fn route(&self) -> ReasoningRoute {
        ReasoningRoute::new(self.backend, self.api)
    }
}

/// Why a requested `reasoning_effort` level is refused.
///
/// A closed set: every reader matches it exhaustively, so adding a variant
/// is a breaking change. The variants serialize as the snake_case strings
/// below and keep this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffortRefusalReason {
    /// The route's stated levels for the model do not include the requested
    /// level.
    UnsupportedLevel,
    /// No source states the levels the route accepts for the model.
    SupportedLevelsUnknown,
    /// The source of the levels exists and cannot be read now (an account
    /// list that is not loaded).
    SupportedLevelsUnavailable,
    /// The request also sets a provider-specific effort.
    ConflictingProviderEffort,
    /// The request also sets a thinking budget.
    ConflictingThinkingBudget,
    /// The request also sets a thinking type that the level cannot be
    /// combined with.
    ConflictingThinkingType,
    /// The request also carries a raw provider reasoning body.
    ConflictingReasoningBody,
    /// The request also sets a reasoning mode that the level cannot be
    /// combined with.
    ConflictingReasoningMode,
}

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

    /// A documented JSON example without the whitespace between its tokens:
    /// the bytes `serde_json::to_string` produces for the same document.
    fn compact(document: &str) -> String {
        let mut out = String::with_capacity(document.len());
        let mut in_string = false;
        let mut escaped = false;
        for ch in document.chars() {
            if in_string {
                out.push(ch);
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
            } else if ch == '"' {
                in_string = true;
                out.push(ch);
            } else if !ch.is_whitespace() {
                out.push(ch);
            }
        }
        out
    }

    fn anthropic_messages() -> ReasoningRoute {
        ReasoningRoute::new(
            ReasoningBackend::Anthropic(AnthropicBackendKind::AnthropicApi),
            ModelApi::Messages,
        )
    }

    fn copilot(api: ModelApi) -> ReasoningRoute {
        ReasoningRoute::new(ReasoningBackend::OpenAi(OpenAiBackendKind::Copilot), api)
    }

    fn unsupported_level_refusal() -> ReasoningEffortRefusal {
        ReasoningEffortRefusal {
            requested: EffortLevel::Max,
            provider: Provider::Anthropic,
            model: "claude-opus-4-5".to_owned(),
            route: anthropic_messages(),
            reason: ReasoningEffortRefusalReason::UnsupportedLevel,
            supported: Some(vec![
                EffortLevel::Low,
                EffortLevel::Medium,
                EffortLevel::High,
            ]),
            facts: Some(ReasoningFactsSource::Catalog),
            conflicting_param: None,
            conflicting_level: None,
        }
    }

    fn openai_refusal(
        reason: ReasoningEffortRefusalReason,
        route: ReasoningRoute,
    ) -> ReasoningEffortRefusal {
        ReasoningEffortRefusal {
            requested: EffortLevel::High,
            provider: Provider::OpenAI,
            model: "gpt-5.5".to_owned(),
            route,
            reason,
            supported: None,
            facts: None,
            conflicting_param: None,
            conflicting_level: None,
        }
    }

    /// The refusal serializes as the documented shape, byte for byte: the
    /// fields in the documented order, `supported` and `facts` as explicit
    /// `null` when absent, and the two conflict fields left out when absent.
    #[test]
    fn refusal_serializes_the_documented_shape() {
        // The two documented examples, as documented (the second one's model
        // is the document's own placeholder).
        const UNSUPPORTED_LEVEL: &str = r#"{
      "requested": "max", "provider": "anthropic", "model": "claude-opus-4-5",
      "route": {"backend": "anthropic_api", "api": "messages"},
      "reason": "unsupported_level", "supported": ["low", "medium", "high"], "facts": "catalog"
    }"#;
        const SUPPORTED_LEVELS_UNKNOWN: &str = r#"{
  "requested": "high", "provider": "openai", "model": "<model id>",
  "route": {"backend": "copilot", "api": "chat_completions"},
  "reason": "supported_levels_unknown", "supported": null, "facts": null }"#;

        let unsupported = unsupported_level_refusal();
        let unknown = ReasoningEffortRefusal {
            model: "<model id>".to_owned(),
            ..openai_refusal(
                ReasoningEffortRefusalReason::SupportedLevelsUnknown,
                copilot(ModelApi::ChatCompletions),
            )
        };
        for (document, refusal) in [
            (UNSUPPORTED_LEVEL, &unsupported),
            (SUPPORTED_LEVELS_UNKNOWN, &unknown),
        ] {
            assert_eq!(
                serde_json::to_string(refusal).expect("a refusal serializes"),
                compact(document)
            );
            assert_eq!(
                &serde_json::from_str::<ReasoningEffortRefusal>(document)
                    .expect("the documented example reads back"),
                refusal
            );
        }

        // A conflict names the conflicting param, and its level when the
        // conflicting setting is itself a level, after `facts`.
        let conflict = ReasoningEffortRefusal {
            reason: ReasoningEffortRefusalReason::ConflictingProviderEffort,
            supported: Some(vec![
                EffortLevel::Low,
                EffortLevel::Medium,
                EffortLevel::High,
            ]),
            facts: Some(ReasoningFactsSource::Account),
            conflicting_param: Some("provider_params.provider_tag.reasoning_effort".to_owned()),
            conflicting_level: Some(EffortLevel::Low),
            ..openai_refusal(
                ReasoningEffortRefusalReason::ConflictingProviderEffort,
                copilot(ModelApi::Responses),
            )
        };
        let serialized = serde_json::to_string(&conflict).expect("a refusal serializes");
        assert_eq!(
            serialized,
            compact(
                r#"{
  "requested": "high", "provider": "openai", "model": "gpt-5.5",
  "route": {"backend": "copilot", "api": "responses"},
  "reason": "conflicting_provider_effort", "supported": ["low", "medium", "high"],
  "facts": "account",
  "conflicting_param": "provider_params.provider_tag.reasoning_effort",
  "conflicting_level": "low" }"#
            )
        );
        assert_eq!(
            serde_json::from_str::<ReasoningEffortRefusal>(&serialized)
                .expect("a conflict reads back"),
            conflict
        );

        for (source, wire) in [
            (ReasoningFactsSource::Catalog, "catalog"),
            (ReasoningFactsSource::Account, "account"),
            (ReasoningFactsSource::Operator, "operator"),
        ] {
            assert_eq!(
                serde_json::to_value(source).expect("a facts source serializes"),
                serde_json::json!(wire)
            );
        }
    }

    /// One message per reason, as documented. The messages are for people; no
    /// surface parses them.
    #[test]
    fn refusal_messages_are_the_documented_texts() {
        use ReasoningEffortRefusalReason as Reason;

        assert_eq!(
            unsupported_level_refusal().to_string(),
            "reasoning effort 'max' is not supported by anthropic model 'claude-opus-4-5' on \
             anthropic_api/messages; supported: low, medium, high"
        );

        let no_effort = ReasoningEffortRefusal {
            supported: Some(Vec::new()),
            facts: Some(ReasoningFactsSource::Catalog),
            ..openai_refusal(
                Reason::UnsupportedLevel,
                ReasoningRoute::new(
                    ReasoningBackend::OpenAi(OpenAiBackendKind::OpenAiApi),
                    ModelApi::Realtime,
                ),
            )
        };
        assert_eq!(
            no_effort.to_string(),
            "reasoning effort 'high' is not supported by openai model 'gpt-5.5' on \
             openai_api/realtime; this route accepts no reasoning effort"
        );

        assert_eq!(
            openai_refusal(
                Reason::SupportedLevelsUnknown,
                copilot(ModelApi::ChatCompletions)
            )
            .to_string(),
            "reasoning effort 'high' cannot be checked for openai model 'gpt-5.5' on \
             copilot/chat_completions: no source states the levels this route accepts; \
             provider_params can set a provider-specific effort"
        );

        assert_eq!(
            openai_refusal(
                Reason::SupportedLevelsUnavailable,
                copilot(ModelApi::Responses)
            )
            .to_string(),
            "reasoning effort 'high' cannot be checked for openai model 'gpt-5.5' on \
             copilot/responses: the account's model list is not loaded; try again once it is"
        );

        let conflict =
            |reason: Reason, param: &str, level: Option<EffortLevel>| ReasoningEffortRefusal {
                conflicting_param: Some(param.to_owned()),
                conflicting_level: level,
                ..openai_refusal(reason, copilot(ModelApi::Responses))
            };
        assert_eq!(
            conflict(
                Reason::ConflictingProviderEffort,
                "provider_params.provider_tag.reasoning_effort",
                Some(EffortLevel::Low),
            )
            .to_string(),
            "reasoning effort 'high' conflicts with \
             provider_params.provider_tag.reasoning_effort = 'low'; set one of them"
        );
        assert_eq!(
            conflict(
                Reason::ConflictingThinkingBudget,
                "provider_params.thinking_budget_tokens",
                None,
            )
            .to_string(),
            "reasoning effort 'high' conflicts with provider_params.thinking_budget_tokens: a \
             thinking budget and a level are two controls; remove one"
        );
        let thinking_type = ReasoningEffortRefusal {
            reason: Reason::ConflictingThinkingType,
            conflicting_param: Some("provider_params.provider_tag.thinking".to_owned()),
            ..unsupported_level_refusal()
        };
        assert_eq!(
            thinking_type.to_string(),
            "reasoning effort 'max' conflicts with provider_params.provider_tag.thinking: \
             anthropic model 'claude-opus-4-5' accepts that thinking type only up to 'high'"
        );
        assert_eq!(
            conflict(
                Reason::ConflictingReasoningBody,
                "provider_params.provider_tag.reasoning",
                None,
            )
            .to_string(),
            "reasoning effort 'high' conflicts with provider_params.provider_tag.reasoning: a \
             raw reasoning body may carry its own effort; remove one"
        );
        assert_eq!(
            conflict(
                Reason::ConflictingReasoningMode,
                "provider_params.reasoning",
                None,
            )
            .to_string(),
            "reasoning effort 'high' conflicts with provider_params.reasoning = 'off'"
        );
    }

    /// The run result block serializes as the documented shape, byte for
    /// byte, and a resolution carries the same fields plus its own
    /// `requested`.
    #[test]
    fn report_serializes_the_documented_shape() {
        const REPORT: &str = r#"{"requested": "high", "sent": [
  {"level": "high", "provider": "openai", "model": "gpt-5.5",
   "route": {"backend": "openai_api", "api": "responses"},
   "fields": ["reasoning.effort"], "facts": "catalog", "decided_by": "request", "requests": 3}]}"#;

        let route = ReasoningRoute::new(
            ReasoningBackend::OpenAi(OpenAiBackendKind::OpenAiApi),
            ModelApi::Responses,
        );
        let report = ReasoningEffortReport {
            requested: EffortLevel::High,
            sent: vec![ReasoningEffortSent {
                level: EffortLevel::High,
                provider: Provider::OpenAI,
                model: "gpt-5.5".to_owned(),
                route: route.clone(),
                fields: vec!["reasoning.effort".to_owned()],
                facts: ReasoningFactsSource::Catalog,
                decided_by: ReasoningEffortDecidedBy::Request,
                requests: 3,
            }],
        };
        assert_eq!(
            serde_json::to_string(&report).expect("a report serializes"),
            compact(REPORT)
        );
        assert_eq!(
            serde_json::from_str::<ReasoningEffortReport>(REPORT)
                .expect("the documented example reads back"),
            report
        );

        let resolution = ReasoningEffortResolution {
            requested: EffortLevel::High,
            level: EffortLevel::Low,
            provider: Provider::OpenAI,
            model: "gpt-5.5".to_owned(),
            route,
            fields: vec!["reasoning_effort".to_owned(), "reasoning.effort".to_owned()],
            facts: ReasoningFactsSource::Operator,
            decided_by: ReasoningEffortDecidedBy::Preference,
        };
        let serialized = serde_json::to_string(&resolution).expect("a resolution serializes");
        assert_eq!(
            serialized,
            compact(
                r#"{"requested": "high", "level": "low", "provider": "openai", "model": "gpt-5.5",
  "route": {"backend": "openai_api", "api": "responses"},
  "fields": ["reasoning_effort", "reasoning.effort"], "facts": "operator",
  "decided_by": "preference"}"#
            )
        );
        assert_eq!(
            serde_json::from_str::<ReasoningEffortResolution>(&serialized)
                .expect("a resolution reads back"),
            resolution
        );
    }
}
