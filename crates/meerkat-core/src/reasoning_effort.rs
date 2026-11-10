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
}
