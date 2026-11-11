//! The provider-neutral `reasoning_effort` option: its vocabulary and its
//! resolver.
//!
//! A request names one level in the [`EffortLevel`] vocabulary; whether a
//! provider request can carry it depends on the model and on the route that
//! serves the request: the backend (a provider-matrix backend kind) and the
//! provider API the request is sent on. This module names the route, what a
//! client observes about it, the typed refusal of a level, and the reports
//! of the level a request was sent with. Its resolver
//! ([`resolve_reasoning_effort`]) decides, for a requested level, a model
//! and a route observation, what to write on a provider request or the
//! typed refusal; [`supported_reasoning_levels`] is its facts step alone.

mod resolve;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Provider;
use crate::model_profile::capabilities::EffortLevel;
use crate::provider_matrix::{
    AnthropicBackendKind, GoogleBackendKind, OpenAiBackendKind, SelfHostedBackendKind,
};

pub(crate) use resolve::explicit_provider_effort;
pub use resolve::{
    ReasoningLevelFacts, ReasoningResolveInput, ReasoningResolveMode, direct_route_observation,
    resolve_reasoning_effort, supported_reasoning_levels,
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

    /// The source that states the effort levels this backend's routes
    /// accept: the kind's own `reasoning_levels_source()`, the facts policy
    /// of the provider matrix, decided per kind.
    pub fn levels_source(self) -> ReasoningFactsSource {
        match self {
            Self::OpenAi(kind) => kind.reasoning_levels_source(),
            Self::Anthropic(kind) => kind.reasoning_levels_source(),
            Self::Google(kind) => kind.reasoning_levels_source(),
            Self::SelfHosted(kind) => kind.reasoning_levels_source(),
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
    /// A source states the levels the route accepts for the model and the
    /// requested level is not among them (an empty set: the route or the
    /// model takes no effort).
    UnsupportedLevel,
    /// No source states the levels the route accepts for the model.
    SupportedLevelsUnknown,
    /// The source of the levels exists and cannot be read now (an account's
    /// model list that is not loaded). The one reason an identical later
    /// request can succeed without a configuration change.
    SupportedLevelsUnavailable,
    /// The request also sets a provider-specific effort that differs from
    /// the level.
    ConflictingProviderEffort,
    /// The request also sets a thinking budget: a budget and a level are two
    /// controls.
    ConflictingThinkingBudget,
    /// The request also sets a thinking type the model does not accept
    /// together with the level on the route.
    ConflictingThinkingType,
    /// The request also carries a raw provider reasoning body, which may
    /// carry an effort of its own.
    ConflictingReasoningBody,
    /// The request also turns reasoning off.
    ConflictingReasoningMode,
}

/// The source of the effort levels a route accepts for a model. Each
/// backend kind names its own (`reasoning_levels_source()` on the
/// provider-matrix kinds, [`ReasoningBackend::levels_source`]); a refusal
/// and a resolution report it as `facts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReasoningFactsSource {
    /// The model's catalog row: the backend forwards the provider's own API
    /// to the provider's own validation.
    Catalog,
    /// The account's model list entry for the model: the backend answers
    /// for itself, and the catalog row is never a source of its levels.
    Account,
    /// The levels the operator declared in the model's configuration.
    Operator,
}

/// A refused `reasoning_effort` level: what was requested, for which model
/// and route, and why. Raised before any provider request is sent.
///
/// `route`, `supported` and `facts` are always serialized, as `null` when
/// absent (`facts` is absent exactly when `supported` is). The two
/// `conflicting_*` fields are serialized only when present. The `Display`
/// text is for people and is not a contract; read the fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReasoningEffortRefusal {
    /// The level the request named.
    pub requested: EffortLevel,
    /// The provider family of the model.
    pub provider: Provider,
    /// The model the request would use.
    pub model: String,
    /// The route the request would be served on. `None` when the client
    /// that serves the request reports no route: a route nobody observed
    /// is never guessed.
    pub route: Option<ReasoningRoute>,
    /// Why the level is refused.
    pub reason: ReasoningEffortRefusalReason,
    /// The levels the route accepts for the model: `None` when unknown or
    /// unavailable, empty when the route or the model takes no effort.
    pub supported: Option<Vec<EffortLevel>>,
    /// The source of `supported`.
    pub facts: Option<ReasoningFactsSource>,
    /// For a conflict: the path of the conflicting setting, for example
    /// `provider_params.provider_tag.effort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicting_param: Option<String>,
    /// For a conflict whose conflicting setting is itself a level: that
    /// level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicting_level: Option<EffortLevel>,
}

/// ` on <backend>/<api>` in a refusal's text, or nothing for a refusal that
/// names no route.
struct OnRoute<'a>(Option<&'a ReasoningRoute>);

impl fmt::Display for OnRoute<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(route) => write!(f, " on {route}"),
            None => Ok(()),
        }
    }
}

impl fmt::Display for ReasoningEffortRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ReasoningEffortRefusalReason as Reason;

        let requested = self.requested.as_wire_str();
        let provider = self.provider.as_str();
        let model = self.model.as_str();
        let on_route = OnRoute(self.route.as_ref());
        let param = self
            .conflicting_param
            .as_deref()
            .unwrap_or("another reasoning setting");
        match self.reason {
            Reason::UnsupportedLevel => {
                write!(
                    f,
                    "reasoning effort '{requested}' is not supported by {provider} model \
                     '{model}'{on_route}"
                )?;
                match self.supported.as_deref() {
                    None => Ok(()),
                    Some([]) => f.write_str("; this route accepts no reasoning effort"),
                    Some(levels) => {
                        f.write_str("; supported: ")?;
                        for (index, level) in levels.iter().enumerate() {
                            if index > 0 {
                                f.write_str(", ")?;
                            }
                            f.write_str(level.as_wire_str())?;
                        }
                        Ok(())
                    }
                }
            }
            Reason::SupportedLevelsUnknown => write!(
                f,
                "reasoning effort '{requested}' cannot be checked for {provider} model \
                 '{model}'{on_route}: no source states the levels this route accepts; \
                 provider_params can set a provider-specific effort"
            ),
            Reason::SupportedLevelsUnavailable => write!(
                f,
                "reasoning effort '{requested}' cannot be checked for {provider} model \
                 '{model}'{on_route}: the account's model list is not loaded; try again once \
                 it is"
            ),
            Reason::ConflictingProviderEffort => {
                write!(f, "reasoning effort '{requested}' conflicts with {param}")?;
                if let Some(level) = self.conflicting_level {
                    write!(f, " = '{}'", level.as_wire_str())?;
                }
                f.write_str("; set one of them")
            }
            Reason::ConflictingThinkingBudget => write!(
                f,
                "reasoning effort '{requested}' conflicts with {param}: a thinking budget and a \
                 level are two controls; remove one"
            ),
            Reason::ConflictingThinkingType => write!(
                f,
                "reasoning effort '{requested}' conflicts with {param}: {provider} model \
                 '{model}' does not accept that thinking type with this level{on_route}"
            ),
            Reason::ConflictingReasoningBody => write!(
                f,
                "reasoning effort '{requested}' conflicts with {param}: a raw reasoning body may \
                 carry its own effort; remove one"
            ),
            Reason::ConflictingReasoningMode => write!(
                f,
                "reasoning effort '{requested}' conflicts with {param} = 'off'"
            ),
        }
    }
}

/// Who decided the level a request was sent with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffortDecidedBy {
    /// The level the request named.
    Request,
    /// A reasoning preference replaced the level.
    Preference,
}

/// A resolved `reasoning_effort` level: what one provider request is sent
/// with, for which model and route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReasoningEffortResolution {
    /// The level the request named.
    pub requested: EffortLevel,
    /// The level sent.
    pub level: EffortLevel,
    /// The provider family of the model.
    pub provider: Provider,
    /// The model the request uses.
    pub model: String,
    /// The route the request is served on.
    pub route: ReasoningRoute,
    /// The wire paths the level is written to, for example
    /// `reasoning.effort`.
    pub fields: Vec<String>,
    /// The source of the levels the route accepts for the model.
    pub facts: ReasoningFactsSource,
    /// Who decided `level`.
    pub decided_by: ReasoningEffortDecidedBy,
}

/// One distinct resolution a run sent, with the number of provider requests
/// that carried it. It has no `requested` of its own: the report holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReasoningEffortSent {
    /// The level sent.
    pub level: EffortLevel,
    /// The provider family of the model.
    pub provider: Provider,
    /// The model the requests used.
    pub model: String,
    /// The route the requests were served on.
    pub route: ReasoningRoute,
    /// The wire paths the level was written to.
    pub fields: Vec<String>,
    /// The source of the levels the route accepts for the model.
    pub facts: ReasoningFactsSource,
    /// Who decided `level`.
    pub decided_by: ReasoningEffortDecidedBy,
    /// The number of provider requests sent with this resolution.
    pub requests: u32,
}

/// The run result block: the level a run's requests named and the distinct
/// resolutions the run sent, in first-use order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReasoningEffortReport {
    /// The level the run's requests named.
    pub requested: EffortLevel,
    /// The distinct resolutions sent, in first-use order.
    pub sent: Vec<ReasoningEffortSent>,
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
            model: "test-anthropic-default".to_owned(),
            route: Some(anthropic_messages()),
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
            model: "test-openai-default".to_owned(),
            route: Some(route),
            reason,
            supported: None,
            facts: None,
            conflicting_param: None,
            conflicting_level: None,
        }
    }

    /// The refusal serializes as the documented shape, byte for byte: the
    /// fields in the documented order, `route`, `supported` and `facts` as
    /// explicit `null` when absent, and the two conflict fields left out when
    /// absent.
    #[test]
    fn refusal_serializes_the_documented_shape() {
        // The two documented examples, as documented but for the first one's
        // model: a synthetic id stands in for the model name the document
        // gives, since core holds no provider model names (the second one's
        // model is the document's own placeholder).
        const UNSUPPORTED_LEVEL: &str = r#"{
      "requested": "max", "provider": "anthropic", "model": "test-anthropic-default",
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
  "requested": "high", "provider": "openai", "model": "test-openai-default",
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

        // A refusal that names no route (no client observation) writes
        // `route` as an explicit `null`, in its place, and reads back. A
        // document that leaves the key out reads as the same refusal.
        let unrouted = ReasoningEffortRefusal {
            provider: Provider::Other,
            model: "<model id>".to_owned(),
            route: None,
            ..openai_refusal(
                ReasoningEffortRefusalReason::SupportedLevelsUnknown,
                copilot(ModelApi::ChatCompletions),
            )
        };
        let serialized = serde_json::to_string(&unrouted).expect("a refusal serializes");
        assert_eq!(
            serialized,
            compact(
                r#"{
  "requested": "high", "provider": "other", "model": "<model id>",
  "route": null,
  "reason": "supported_levels_unknown", "supported": null, "facts": null }"#
            )
        );
        assert_eq!(
            serde_json::from_str::<ReasoningEffortRefusal>(&serialized)
                .expect("a refusal with no route reads back"),
            unrouted
        );
        assert_eq!(
            serde_json::from_str::<ReasoningEffortRefusal>(
                r#"{"requested": "high", "provider": "other", "model": "<model id>",
  "reason": "supported_levels_unknown", "supported": null, "facts": null }"#
            )
            .expect("a document without the route key reads back"),
            unrouted
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
            "reasoning effort 'max' is not supported by anthropic model 'test-anthropic-default' \
             on anthropic_api/messages; supported: low, medium, high"
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
            "reasoning effort 'high' is not supported by openai model 'test-openai-default' on \
             openai_api/realtime; this route accepts no reasoning effort"
        );

        assert_eq!(
            openai_refusal(
                Reason::SupportedLevelsUnknown,
                copilot(ModelApi::ChatCompletions)
            )
            .to_string(),
            "reasoning effort 'high' cannot be checked for openai model 'test-openai-default' on \
             copilot/chat_completions: no source states the levels this route accepts; \
             provider_params can set a provider-specific effort"
        );

        assert_eq!(
            openai_refusal(
                Reason::SupportedLevelsUnavailable,
                copilot(ModelApi::Responses)
            )
            .to_string(),
            "reasoning effort 'high' cannot be checked for openai model 'test-openai-default' on \
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
        // The text states the conflict, not a ceiling: it holds on a route
        // that takes the thinking type up to some level and on a route that
        // takes it with no level at all.
        assert_eq!(
            thinking_type.to_string(),
            "reasoning effort 'max' conflicts with provider_params.provider_tag.thinking: \
             anthropic model 'test-anthropic-default' does not accept that thinking type with this \
             level on anthropic_api/messages"
        );
        let thinking_type_at_low = ReasoningEffortRefusal {
            requested: EffortLevel::Low,
            route: Some(ReasoningRoute::new(
                ReasoningBackend::Anthropic(AnthropicBackendKind::Copilot),
                ModelApi::Messages,
            )),
            ..thinking_type.clone()
        };
        assert_eq!(
            thinking_type_at_low.to_string(),
            "reasoning effort 'low' conflicts with provider_params.provider_tag.thinking: \
             anthropic model 'test-anthropic-default' does not accept that thinking type with this \
             level on copilot/messages"
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

        // A refusal that names no route leaves the route out of its text.
        let unrouted = |refusal: ReasoningEffortRefusal| ReasoningEffortRefusal {
            route: None,
            ..refusal
        };
        assert_eq!(
            unrouted(unsupported_level_refusal()).to_string(),
            "reasoning effort 'max' is not supported by anthropic model 'test-anthropic-default'; \
             supported: low, medium, high"
        );
        assert_eq!(
            unrouted(openai_refusal(
                Reason::SupportedLevelsUnknown,
                copilot(ModelApi::ChatCompletions)
            ))
            .to_string(),
            "reasoning effort 'high' cannot be checked for openai model 'test-openai-default': no \
             source states the levels this route accepts; provider_params can set a \
             provider-specific effort"
        );
        assert_eq!(
            unrouted(openai_refusal(
                Reason::SupportedLevelsUnavailable,
                copilot(ModelApi::Responses)
            ))
            .to_string(),
            "reasoning effort 'high' cannot be checked for openai model 'test-openai-default': the \
             account's model list is not loaded; try again once it is"
        );
        assert_eq!(
            unrouted(thinking_type).to_string(),
            "reasoning effort 'max' conflicts with provider_params.provider_tag.thinking: \
             anthropic model 'test-anthropic-default' does not accept that thinking type with this \
             level"
        );
    }

    /// The run result block serializes as the documented shape, byte for
    /// byte, and a resolution carries the same fields plus its own
    /// `requested`.
    #[test]
    fn report_serializes_the_documented_shape() {
        // The documented example, with a synthetic id for its model name.
        const REPORT: &str = r#"{"requested": "high", "sent": [
  {"level": "high", "provider": "openai", "model": "test-openai-default",
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
                model: "test-openai-default".to_owned(),
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
            model: "test-openai-default".to_owned(),
            route,
            fields: vec!["reasoning_effort".to_owned(), "reasoning.effort".to_owned()],
            facts: ReasoningFactsSource::Operator,
            decided_by: ReasoningEffortDecidedBy::Preference,
        };
        let serialized = serde_json::to_string(&resolution).expect("a resolution serializes");
        assert_eq!(
            serialized,
            compact(
                r#"{"requested": "high", "level": "low", "provider": "openai",
  "model": "test-openai-default",
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
