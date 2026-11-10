//! The resolver of the provider-neutral `reasoning_effort` option: which
//! source states the levels a route accepts, which typed field each provider
//! API takes a level in, and the one function that decides, for a requested
//! level, a model and a route observation, what to write on a provider
//! request or the typed refusal.
//!
//! Pure and bounded: no I/O, no clock. The level judged is always the level
//! requested; resolution never picks another level, model or route.

use super::{
    AccountLevelStatement, ModelApi, ReasoningBackend, ReasoningEffortDecidedBy,
    ReasoningEffortRefusal, ReasoningEffortRefusalReason, ReasoningEffortResolution,
    ReasoningFactsSource, ReasoningRoute, ReasoningRouteObservation,
};
use crate::Provider;
use crate::lifecycle::run_primitive::{
    AnthropicEffort, AnthropicProviderTag, AnthropicThinkingConfig, GeminiProviderTag,
    GeminiThinkingLevel, OpenAiProviderTag, ProviderParamsOverride, ProviderTag, ReasoningEffort,
    ReasoningMode,
};
use crate::model_profile::capabilities::{EffortLevel, ModelCapabilities, ThinkingSupport};
use crate::provider_matrix::{
    AnthropicBackendKind, GoogleBackendKind, OpenAiBackendKind, SelfHostedBackendKind,
};

/// How a requested level is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReasoningResolveMode {
    /// The level a request names: every conflict with another reasoning
    /// setting of the request is refused.
    Strict,
    /// A reasoning preference: it replaces a provider-specific effort the
    /// request carries, and any refusal leaves the request as it was.
    Preference,
}

/// What a requested level is resolved against. Built with
/// [`ReasoningResolveInput::new`]; the fields stay readable.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ReasoningResolveInput<'a> {
    /// The level to resolve.
    pub requested: EffortLevel,
    /// The provider family of the model.
    pub provider: Provider,
    /// The model the request uses, as the session names it.
    pub model: &'a str,
    /// What the client that serves the request observes about its route;
    /// `None` for a client that reports no route.
    pub observation: Option<&'a ReasoningRouteObservation>,
    /// The model's catalog row, when the catalog has one.
    pub catalog_row: Option<&'a ModelCapabilities>,
    /// Who decided the level, as the resolution reports it.
    pub decided_by: ReasoningEffortDecidedBy,
}

impl<'a> ReasoningResolveInput<'a> {
    /// The input for resolving `requested` for `model` of `provider`:
    /// against `observation`, what the serving client observes about its
    /// route (`None` when it reports none), and `catalog_row`, the model's
    /// catalog row (`None` when the catalog has none). `decided_by` is
    /// reported back in the resolution.
    pub fn new(
        requested: EffortLevel,
        provider: Provider,
        model: &'a str,
        observation: Option<&'a ReasoningRouteObservation>,
        catalog_row: Option<&'a ModelCapabilities>,
        decided_by: ReasoningEffortDecidedBy,
    ) -> Self {
        Self {
            requested,
            provider,
            model,
            observation,
            catalog_row,
            decided_by,
        }
    }
}

/// What the source of a route's levels states for a model: the facts step
/// of the resolution, on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasoningLevelFacts {
    /// The levels the route accepts for the model: the levels the source
    /// states that the route can also express, lowest first and without
    /// repeats. Empty when the route or the model takes no effort.
    Stated {
        levels: Vec<EffortLevel>,
        source: ReasoningFactsSource,
    },
    /// No source states the levels.
    Unknown,
    /// The source exists and cannot be read now: the account's model list is
    /// not loaded.
    Unavailable,
}

impl ReasoningLevelFacts {
    /// The supported levels as a refusal reports them: `None` when unknown
    /// or unavailable.
    pub fn supported(&self) -> Option<&[EffortLevel]> {
        match self {
            Self::Stated { levels, .. } => Some(levels),
            Self::Unknown | Self::Unavailable => None,
        }
    }

    /// The source of the supported levels: `None` exactly when
    /// [`Self::supported`] is.
    pub fn source(&self) -> Option<ReasoningFactsSource> {
        match self {
            Self::Stated { source, .. } => Some(*source),
            Self::Unknown | Self::Unavailable => None,
        }
    }
}

/// Every level, lowest first: the order `supported` is reported in.
const LEVEL_LADDER: [EffortLevel; 7] = [
    EffortLevel::None,
    EffortLevel::Minimal,
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
    EffortLevel::Max,
];

const PARAM_REASONING: &str = "provider_params.reasoning";
const PARAM_THINKING_BUDGET_TOKENS: &str = "provider_params.thinking_budget_tokens";
const PARAM_TAG: &str = "provider_params.provider_tag";
const PARAM_OPENAI_REASONING_EFFORT: &str = "provider_params.provider_tag.reasoning_effort";
const PARAM_OPENAI_REASONING: &str = "provider_params.provider_tag.reasoning";
const PARAM_ANTHROPIC_EFFORT: &str = "provider_params.provider_tag.effort";
const PARAM_ANTHROPIC_THINKING: &str = "provider_params.provider_tag.thinking";
const PARAM_ANTHROPIC_THINKING_BUDGET_TOKENS: &str =
    "provider_params.provider_tag.thinking_budget_tokens";
const PARAM_GEMINI_THINKING_LEVEL: &str = "provider_params.provider_tag.thinking_level";
const PARAM_GEMINI_NESTED_THINKING_LEVEL: &str =
    "provider_params.provider_tag.thinking.thinking_level";
const PARAM_GEMINI_THINKING_BUDGET: &str = "provider_params.provider_tag.thinking_budget";
const PARAM_GEMINI_NESTED_THINKING_BUDGET: &str =
    "provider_params.provider_tag.thinking.thinking_budget";

/// One row of the translation table: the typed field a provider API takes a
/// level in, for the family that speaks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Translation {
    /// Responses, OpenAI family: `reasoning.effort`.
    OpenAiResponses,
    /// Chat Completions, OpenAI family: `reasoning_effort` and
    /// `reasoning.effort`.
    OpenAiChatCompletions,
    /// Chat Completions, Gemini family (the Copilot Gemini route): the same
    /// two wire fields, written from the Gemini thinking level.
    GeminiChatCompletions,
    /// Messages, Anthropic family on a direct backend: `output_config.effort`.
    AnthropicMessages,
    /// Messages, Anthropic family on Copilot: `thinking: {type: adaptive}`
    /// and `output_config.effort`, always as a pair.
    AnthropicMessagesWithAdaptiveThinking,
    /// generateContent, Gemini family:
    /// `generationConfig.thinkingConfig.thinkingLevel`.
    GeminiGenerateContent,
}

/// The typed value one level is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LevelWrite {
    /// `OpenAiProviderTag.reasoning_effort`, with the resolved marker.
    OpenAi(ReasoningEffort),
    /// `AnthropicProviderTag.effort`.
    Anthropic(AnthropicEffort),
    /// `AnthropicProviderTag.effort` and `thinking = Adaptive`.
    AnthropicWithAdaptiveThinking(AnthropicEffort),
    /// `GeminiProviderTag.thinking_level`: the nested config's level when
    /// the request has a nested config, else the flat one.
    Gemini(GeminiThinkingLevel),
}

impl Translation {
    /// The table row of a route. `None` for a route that takes no level
    /// Meerkat can write: the realtime API, and an API the backend's family
    /// does not speak.
    fn of(backend: ReasoningBackend, api: ModelApi) -> Option<Self> {
        match (backend, api) {
            (
                ReasoningBackend::OpenAi(_) | ReasoningBackend::SelfHosted(_),
                ModelApi::Responses,
            ) => Some(Self::OpenAiResponses),
            (
                ReasoningBackend::OpenAi(_) | ReasoningBackend::SelfHosted(_),
                ModelApi::ChatCompletions,
            ) => Some(Self::OpenAiChatCompletions),
            (ReasoningBackend::Google(_), ModelApi::ChatCompletions) => {
                Some(Self::GeminiChatCompletions)
            }
            (ReasoningBackend::Google(_), ModelApi::GenerateContent) => {
                Some(Self::GeminiGenerateContent)
            }
            (ReasoningBackend::Anthropic(kind), ModelApi::Messages) => Some(match kind {
                AnthropicBackendKind::AnthropicApi
                | AnthropicBackendKind::Bedrock
                | AnthropicBackendKind::Vertex
                | AnthropicBackendKind::Foundry => Self::AnthropicMessages,
                AnthropicBackendKind::Copilot => Self::AnthropicMessagesWithAdaptiveThinking,
            }),
            (
                ReasoningBackend::OpenAi(_) | ReasoningBackend::SelfHosted(_),
                ModelApi::Messages | ModelApi::GenerateContent | ModelApi::Realtime,
            )
            | (
                ReasoningBackend::Anthropic(_),
                ModelApi::Responses
                | ModelApi::ChatCompletions
                | ModelApi::GenerateContent
                | ModelApi::Realtime,
            )
            | (
                ReasoningBackend::Google(_),
                ModelApi::Responses | ModelApi::Messages | ModelApi::Realtime,
            ) => None,
        }
    }

    /// The typed value `level` is written as on this row; `None` for a level
    /// the row cannot express.
    fn write_for(self, level: EffortLevel) -> Option<LevelWrite> {
        match self {
            Self::OpenAiResponses | Self::OpenAiChatCompletions => {
                openai_effort(level).map(LevelWrite::OpenAi)
            }
            Self::AnthropicMessages => anthropic_effort(level).map(LevelWrite::Anthropic),
            Self::AnthropicMessagesWithAdaptiveThinking => {
                anthropic_effort(level).map(LevelWrite::AnthropicWithAdaptiveThinking)
            }
            Self::GeminiGenerateContent => gemini_thinking_level(level).map(LevelWrite::Gemini),
            // The Chat Completions effort field has no `minimal`.
            Self::GeminiChatCompletions => match level {
                EffortLevel::Low | EffortLevel::Medium | EffortLevel::High => {
                    gemini_thinking_level(level).map(LevelWrite::Gemini)
                }
                EffortLevel::None
                | EffortLevel::Minimal
                | EffortLevel::Xhigh
                | EffortLevel::Max => None,
            },
        }
    }

    /// The wire paths a level is written to on this row.
    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::OpenAiResponses => &["reasoning.effort"],
            Self::OpenAiChatCompletions | Self::GeminiChatCompletions => {
                &["reasoning_effort", "reasoning.effort"]
            }
            Self::AnthropicMessages => &["output_config.effort"],
            Self::AnthropicMessagesWithAdaptiveThinking => {
                &["thinking.type", "output_config.effort"]
            }
            Self::GeminiGenerateContent => &["generationConfig.thinkingConfig.thinkingLevel"],
        }
    }

    /// Whether the generic `thinking_budget_tokens` conflicts on this row. A
    /// strict request refuses it everywhere; the preference keeps what its
    /// lowering did before, which never read it on the OpenAI rows.
    fn reads_generic_budget(self, mode: ReasoningResolveMode) -> bool {
        match (mode, self) {
            (ReasoningResolveMode::Strict, _) => true,
            (
                ReasoningResolveMode::Preference,
                Self::OpenAiResponses | Self::OpenAiChatCompletions,
            ) => false,
            (
                ReasoningResolveMode::Preference,
                Self::GeminiChatCompletions
                | Self::AnthropicMessages
                | Self::AnthropicMessagesWithAdaptiveThinking
                | Self::GeminiGenerateContent,
            ) => true,
        }
    }

    /// The first setting of the request that conflicts with a level on this
    /// row: a provider effort that differs, a thinking budget, a thinking
    /// type the Copilot Messages pair excludes, an opaque reasoning body.
    /// The generic reasoning mode, the catalog row's thinking-type cap and a
    /// tag of another family have their own steps.
    fn conflict(
        self,
        params: Option<&ProviderParamsOverride>,
        requested: EffortLevel,
        mode: ReasoningResolveMode,
    ) -> Option<Conflict> {
        let params = params?;
        let tag = params.provider_tag.as_ref();

        // A provider effort that differs from the level. The preference
        // replaces it.
        if mode == ReasoningResolveMode::Strict
            && let Some((param, level)) = tag
                .into_iter()
                .flat_map(provider_efforts)
                .find(|(_, level)| *level != requested)
        {
            return Some(Conflict::ProviderEffort { param, level });
        }

        // A thinking budget, generic or the family's own: a budget and a
        // level are two controls.
        if params.thinking_budget_tokens.is_some() && self.reads_generic_budget(mode) {
            return Some(Conflict::ThinkingBudget {
                param: PARAM_THINKING_BUDGET_TOKENS,
            });
        }
        match (self, tag) {
            (
                Self::AnthropicMessages | Self::AnthropicMessagesWithAdaptiveThinking,
                Some(ProviderTag::Anthropic(tag)),
            ) => {
                if matches!(tag.thinking, Some(AnthropicThinkingConfig::Enabled { .. })) {
                    return Some(Conflict::ThinkingBudget {
                        param: PARAM_ANTHROPIC_THINKING,
                    });
                }
                if tag.thinking_budget_tokens.is_some() {
                    return Some(Conflict::ThinkingBudget {
                        param: PARAM_ANTHROPIC_THINKING_BUDGET_TOKENS,
                    });
                }
                // The pair is the only body the Copilot Messages route takes
                // a level in: any other thinking type conflicts, at every
                // level.
                match (self, tag.thinking.as_ref()) {
                    (
                        Self::AnthropicMessagesWithAdaptiveThinking,
                        Some(
                            AnthropicThinkingConfig::Disabled
                            | AnthropicThinkingConfig::BetweenTools,
                        ),
                    ) => Some(Conflict::ThinkingType),
                    _ => None,
                }
            }
            (
                Self::GeminiChatCompletions | Self::GeminiGenerateContent,
                Some(ProviderTag::Gemini(tag)),
            ) => {
                if tag.thinking_budget.is_some() {
                    return Some(Conflict::ThinkingBudget {
                        param: PARAM_GEMINI_THINKING_BUDGET,
                    });
                }
                tag.thinking
                    .as_ref()
                    .is_some_and(|thinking| thinking.thinking_budget.is_some())
                    .then_some(Conflict::ThinkingBudget {
                        param: PARAM_GEMINI_NESTED_THINKING_BUDGET,
                    })
            }
            // An opaque reasoning body may carry an effort of its own.
            (
                Self::OpenAiResponses | Self::OpenAiChatCompletions,
                Some(ProviderTag::OpenAi(tag)),
            ) => tag.reasoning.is_some().then_some(Conflict::ReasoningBody {
                param: PARAM_OPENAI_REASONING,
            }),
            // No tag, or a tag of another family: nothing of this row's to
            // read. The write step refuses the other family's tag.
            _ => None,
        }
    }

    /// The route constraint of the Messages rows: a thinking type the
    /// catalog row accepts only at `high` effort or below, with a higher
    /// level. Judged only where a catalog row of the model's provider
    /// exists.
    fn thinking_type_cap(
        self,
        input: &ReasoningResolveInput<'_>,
        params: Option<&ProviderParamsOverride>,
    ) -> Option<Conflict> {
        match self {
            Self::AnthropicMessages | Self::AnthropicMessagesWithAdaptiveThinking => {}
            Self::OpenAiResponses
            | Self::OpenAiChatCompletions
            | Self::GeminiChatCompletions
            | Self::GeminiGenerateContent => return None,
        }
        let row = input
            .catalog_row
            .filter(|row| row.provider == input.provider)?;
        let Some(ProviderTag::Anthropic(tag)) = params?.provider_tag.as_ref() else {
            return None;
        };
        let thinking = tag.thinking.as_ref()?;
        (row.anthropic_thinking_capped_at_high_effort(thinking)
            && input.requested > EffortLevel::High)
            .then_some(Conflict::ThinkingType)
    }
}

/// A setting of the request that conflicts with a level. Every conflict
/// names its param; only a conflicting provider effort is itself a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Conflict {
    ProviderEffort {
        param: &'static str,
        level: EffortLevel,
    },
    ThinkingBudget {
        param: &'static str,
    },
    ThinkingType,
    ReasoningBody {
        param: &'static str,
    },
    ReasoningMode,
}

impl Conflict {
    /// The refusal reason, the conflicting param and, for a level, its
    /// value.
    fn parts(
        self,
    ) -> (
        ReasoningEffortRefusalReason,
        &'static str,
        Option<EffortLevel>,
    ) {
        use ReasoningEffortRefusalReason as Reason;
        match self {
            Self::ProviderEffort { param, level } => {
                (Reason::ConflictingProviderEffort, param, Some(level))
            }
            Self::ThinkingBudget { param } => (Reason::ConflictingThinkingBudget, param, None),
            Self::ThinkingType => (
                Reason::ConflictingThinkingType,
                PARAM_ANTHROPIC_THINKING,
                None,
            ),
            Self::ReasoningBody { param } => (Reason::ConflictingReasoningBody, param, None),
            Self::ReasoningMode => (Reason::ConflictingReasoningMode, PARAM_REASONING, None),
        }
    }
}

/// Why a level is refused: the facts do not admit it, or a setting of the
/// request conflicts with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cause {
    /// The reason follows from the facts: an unsupported level where a
    /// source states the levels, unknown or unavailable where none does.
    Facts,
    Conflict(Conflict),
}

/// The refusal of `input.requested` for `cause`, with the supported levels
/// as `facts` states them. Built here only, so an unsupported level always
/// lists the supported set and a conflict always names its param.
fn refusal(
    input: &ReasoningResolveInput<'_>,
    facts: &ReasoningLevelFacts,
    cause: Cause,
) -> ReasoningEffortRefusal {
    use ReasoningEffortRefusalReason as Reason;
    let (reason, conflicting_param, conflicting_level) = match cause {
        Cause::Facts => (
            match facts {
                ReasoningLevelFacts::Stated { .. } => Reason::UnsupportedLevel,
                ReasoningLevelFacts::Unknown => Reason::SupportedLevelsUnknown,
                ReasoningLevelFacts::Unavailable => Reason::SupportedLevelsUnavailable,
            },
            None,
            None,
        ),
        Cause::Conflict(conflict) => {
            let (reason, param, level) = conflict.parts();
            (reason, Some(param.to_owned()), level)
        }
    };
    ReasoningEffortRefusal {
        requested: input.requested,
        provider: input.provider,
        model: input.model.to_owned(),
        route: match input.observation {
            Some(observation) => Some(observation.route()),
            None => unobserved_route(input.provider),
        },
        reason,
        supported: facts.supported().map(<[EffortLevel]>::to_vec),
        facts: facts.source(),
        conflicting_param,
        conflicting_level,
    }
}

/// The route of a provider family when no client observes one: the direct
/// backend of the OpenAI, Anthropic and Gemini families on its own API, and
/// the self-hosted backend on Chat Completions. `None` for a provider
/// outside the families, which has no route of its own.
fn family_route(provider: Provider) -> Option<(ReasoningBackend, ModelApi)> {
    match provider {
        Provider::OpenAI => Some((
            ReasoningBackend::OpenAi(OpenAiBackendKind::OpenAiApi),
            ModelApi::Responses,
        )),
        Provider::Anthropic => Some((
            ReasoningBackend::Anthropic(AnthropicBackendKind::AnthropicApi),
            ModelApi::Messages,
        )),
        Provider::Gemini => Some((
            ReasoningBackend::Google(GoogleBackendKind::GoogleGenAi),
            ModelApi::GenerateContent,
        )),
        Provider::SelfHosted => Some((
            ReasoningBackend::SelfHosted(SelfHostedBackendKind::SelfHosted),
            ModelApi::ChatCompletions,
        )),
        Provider::Other => None,
    }
}

/// The route a refusal names when the client reports none: the family's
/// own route. `None` for a provider outside the families: a route nobody
/// observed is never guessed.
fn unobserved_route(provider: Provider) -> Option<ReasoningRoute> {
    family_route(provider).map(|(backend, api)| ReasoningRoute::new(backend, api))
}

/// The observation of a provider family's direct route
/// (`openai_api/responses`, `anthropic_api/messages`,
/// `google_genai/generate_content`), stating nothing beyond the route.
/// `None` for the self-hosted family, whose levels only its own client can
/// report (the operator declares them), and for a provider outside the
/// families.
pub fn direct_route_observation(provider: Provider) -> Option<ReasoningRouteObservation> {
    let (backend, api) = match provider {
        Provider::OpenAI | Provider::Anthropic | Provider::Gemini => family_route(provider)?,
        Provider::SelfHosted | Provider::Other => return None,
    };
    Some(ReasoningRouteObservation {
        backend,
        api,
        account: AccountLevelStatement::NotApplicable,
        operator_levels: None,
    })
}

/// What a source states, before the route's expressible levels are applied.
enum Statement<'a> {
    Levels(&'a [EffortLevel]),
    Unknown,
    Unavailable,
}

/// What the catalog row states for a model on a backend of `provider`'s
/// family. A row of another provider is no row.
fn catalog_statement(
    backend: ReasoningBackend,
    provider: Provider,
    catalog_row: Option<&ModelCapabilities>,
) -> Statement<'_> {
    let family = match backend {
        ReasoningBackend::OpenAi(_) => Provider::OpenAI,
        ReasoningBackend::Anthropic(_) => Provider::Anthropic,
        ReasoningBackend::Google(_) => Provider::Gemini,
        ReasoningBackend::SelfHosted(_) => Provider::SelfHosted,
    };
    let Some(row) = catalog_row.filter(|row| row.provider == provider && provider == family) else {
        return Statement::Unknown;
    };
    match backend {
        ReasoningBackend::OpenAi(_) => {
            if row.supports_reasoning {
                Statement::Levels(row.effort_levels)
            } else {
                Statement::Levels(&[])
            }
        }
        ReasoningBackend::Anthropic(_) => Statement::Levels(row.effort_levels),
        // A thinking-level row lists the levels it accepts; one that lists
        // none records nothing to check a level against.
        ReasoningBackend::Google(_) => match row.thinking {
            ThinkingSupport::GeminiThinkingLevel if row.effort_levels.is_empty() => {
                Statement::Unknown
            }
            ThinkingSupport::GeminiThinkingLevel => Statement::Levels(row.effort_levels),
            ThinkingSupport::None
            | ThinkingSupport::AnthropicEnabledOnly
            | ThinkingSupport::AnthropicAdaptiveOnly
            | ThinkingSupport::AnthropicAdaptiveAndEnabled
            | ThinkingSupport::AnthropicAdaptiveOrBetweenTools => Statement::Levels(&[]),
        },
        // The self-hosted family has no catalog; its source is the operator.
        ReasoningBackend::SelfHosted(_) => Statement::Unknown,
    }
}

/// The levels a route accepts for a model, and their source: the facts step
/// of [`resolve_reasoning_effort`] on its own.
///
/// The backend kind names the source ([`ReasoningBackend::levels_source`]):
/// the catalog row, the account's model list, or the operator's
/// configuration. Only that source is read; on a Copilot backend the catalog
/// row is never a source of levels, whatever it lists. The stated levels are
/// intersected with the levels the route's API can express. With no
/// observation the levels are unknown; a route that takes no level (the
/// realtime API) states the empty set.
pub fn supported_reasoning_levels(
    provider: Provider,
    observation: Option<&ReasoningRouteObservation>,
    catalog_row: Option<&ModelCapabilities>,
) -> ReasoningLevelFacts {
    let Some(observation) = observation else {
        return ReasoningLevelFacts::Unknown;
    };
    let source = observation.backend.levels_source();
    let Some(translation) = Translation::of(observation.backend, observation.api) else {
        return ReasoningLevelFacts::Stated {
            levels: Vec::new(),
            source,
        };
    };
    let statement = match source {
        ReasoningFactsSource::Catalog => {
            catalog_statement(observation.backend, provider, catalog_row)
        }
        ReasoningFactsSource::Account => match &observation.account {
            AccountLevelStatement::Stated(levels) => Statement::Levels(levels),
            AccountLevelStatement::NotStated | AccountLevelStatement::NotApplicable => {
                Statement::Unknown
            }
            AccountLevelStatement::ListUnavailable => Statement::Unavailable,
        },
        ReasoningFactsSource::Operator => match &observation.operator_levels {
            Some(levels) => Statement::Levels(levels),
            None => Statement::Unknown,
        },
    };
    match statement {
        Statement::Levels(stated) => ReasoningLevelFacts::Stated {
            levels: LEVEL_LADDER
                .into_iter()
                .filter(|level| stated.contains(level) && translation.write_for(*level).is_some())
                .collect(),
            source,
        },
        Statement::Unknown => ReasoningLevelFacts::Unknown,
        Statement::Unavailable => ReasoningLevelFacts::Unavailable,
    }
}

/// Resolve `input.requested` for the model and the observed route: write the
/// route's typed field on `params`, the request's own copy, or refuse.
///
/// In order: the conflicts with the request's other reasoning settings; the
/// facts ([`supported_reasoning_levels`]); the level against the supported
/// set; the route's constraints; the write. A strict request reports a
/// conflict before the facts; a preference keeps the order of the lowering
/// it replaces (reasoning off, the facts, then the other conflicts) and
/// replaces a provider-specific effort instead of refusing it.
///
/// On success `params` carries the level in the route's typed field (and,
/// on an OpenAI-family route, the request-local resolved marker), and the
/// resolution's `level` equals `requested`. On refusal `params` is
/// untouched.
pub fn resolve_reasoning_effort(
    input: ReasoningResolveInput<'_>,
    params: &mut Option<ProviderParamsOverride>,
    mode: ReasoningResolveMode,
) -> Result<ReasoningEffortResolution, ReasoningEffortRefusal> {
    let facts = supported_reasoning_levels(input.provider, input.observation, input.catalog_row);
    let refuse = |cause: Cause| refusal(&input, &facts, cause);

    // The generic reasoning mode needs no route to judge.
    let reasoning_off = params
        .as_ref()
        .is_some_and(|params| params.reasoning == Some(ReasoningMode::Off));
    let off_conflicts = match mode {
        // `none` beside reasoning off says the same thing twice.
        ReasoningResolveMode::Strict => input.requested != EffortLevel::None,
        ReasoningResolveMode::Preference => true,
    };
    if reasoning_off && off_conflicts {
        return Err(refuse(Cause::Conflict(Conflict::ReasoningMode)));
    }

    // No observation: unknown. A route with no row in the translation
    // table: the empty set. Either way the facts say it.
    let Some((observation, translation)) = input.observation.and_then(|observation| {
        Translation::of(observation.backend, observation.api)
            .map(|translation| (observation, translation))
    }) else {
        return Err(refuse(Cause::Facts));
    };

    let conflict = translation.conflict(params.as_ref(), input.requested, mode);
    let accepted = match &facts {
        ReasoningLevelFacts::Stated { levels, source } if levels.contains(&input.requested) => {
            translation
                .write_for(input.requested)
                .map(|write| (write, *source))
        }
        ReasoningLevelFacts::Stated { .. }
        | ReasoningLevelFacts::Unknown
        | ReasoningLevelFacts::Unavailable => None,
    };
    let (write, source) = match (accepted, conflict, mode) {
        (Some(accepted), None, _) => accepted,
        (None, None, _) | (None, Some(_), ReasoningResolveMode::Preference) => {
            return Err(refuse(Cause::Facts));
        }
        (Some(_), Some(conflict), _) | (None, Some(conflict), ReasoningResolveMode::Strict) => {
            return Err(refuse(Cause::Conflict(conflict)));
        }
    };

    if let Some(conflict) = translation.thinking_type_cap(&input, params.as_ref()) {
        return Err(refuse(Cause::Conflict(conflict)));
    }

    if let Err(conflict) = write_level(write, params) {
        return Err(refuse(Cause::Conflict(conflict)));
    }
    Ok(ReasoningEffortResolution {
        requested: input.requested,
        level: input.requested,
        provider: input.provider,
        model: input.model.to_owned(),
        route: observation.route(),
        fields: translation
            .fields()
            .iter()
            .map(|field| (*field).to_owned())
            .collect(),
        facts: source,
        decided_by: input.decided_by,
    })
}

/// Write one level onto the request copy. A copy that carries another
/// family's tag (or an untyped tag bag) has no place for the typed field:
/// it is refused, untouched, as a raw provider body that may carry an effort
/// of its own.
fn write_level(
    write: LevelWrite,
    params: &mut Option<ProviderParamsOverride>,
) -> Result<(), Conflict> {
    const OTHER_FAMILY_TAG: Conflict = Conflict::ReasoningBody { param: PARAM_TAG };
    match write {
        LevelWrite::OpenAi(effort) => {
            let tag = openai_tag(params).ok_or(OTHER_FAMILY_TAG)?;
            tag.reasoning_effort = Some(effort);
            tag.reasoning_effort_resolved = true;
        }
        LevelWrite::Anthropic(effort) => {
            let tag = anthropic_tag(params).ok_or(OTHER_FAMILY_TAG)?;
            tag.effort = Some(effort);
        }
        LevelWrite::AnthropicWithAdaptiveThinking(effort) => {
            let tag = anthropic_tag(params).ok_or(OTHER_FAMILY_TAG)?;
            tag.effort = Some(effort);
            tag.thinking = Some(AnthropicThinkingConfig::Adaptive);
        }
        LevelWrite::Gemini(level) => {
            let tag = gemini_tag(params).ok_or(OTHER_FAMILY_TAG)?;
            // Write the level the request sends: the nested config's level
            // wins over the flat field on the wire.
            match tag.thinking.as_mut() {
                Some(thinking) => thinking.thinking_level = Some(level),
                None => tag.thinking_level = Some(level),
            }
        }
    }
    Ok(())
}

/// The OpenAI tag of the request copy, created when absent. `None`, with the
/// copy untouched, when it carries another provider's tag.
fn openai_tag(params: &mut Option<ProviderParamsOverride>) -> Option<&mut OpenAiProviderTag> {
    let params = params.get_or_insert_with(ProviderParamsOverride::default);
    let tag = params
        .provider_tag
        .get_or_insert_with(|| ProviderTag::OpenAi(Default::default()));
    match tag {
        ProviderTag::OpenAi(tag) => Some(tag),
        ProviderTag::Anthropic(_) | ProviderTag::Gemini(_) | ProviderTag::Unknown { .. } => None,
    }
}

/// The Anthropic tag of the request copy, created when absent. `None`, with
/// the copy untouched, when it carries another provider's tag.
fn anthropic_tag(params: &mut Option<ProviderParamsOverride>) -> Option<&mut AnthropicProviderTag> {
    let params = params.get_or_insert_with(ProviderParamsOverride::default);
    let tag = params
        .provider_tag
        .get_or_insert_with(|| ProviderTag::Anthropic(Default::default()));
    match tag {
        ProviderTag::Anthropic(tag) => Some(tag),
        ProviderTag::OpenAi(_) | ProviderTag::Gemini(_) | ProviderTag::Unknown { .. } => None,
    }
}

/// The Gemini tag of the request copy, created when absent. `None`, with the
/// copy untouched, when it carries another provider's tag.
fn gemini_tag(params: &mut Option<ProviderParamsOverride>) -> Option<&mut GeminiProviderTag> {
    let params = params.get_or_insert_with(ProviderParamsOverride::default);
    let tag = params
        .provider_tag
        .get_or_insert_with(|| ProviderTag::Gemini(Default::default()));
    match tag {
        ProviderTag::Gemini(tag) => Some(tag),
        ProviderTag::Anthropic(_) | ProviderTag::OpenAi(_) | ProviderTag::Unknown { .. } => None,
    }
}

fn openai_effort(level: EffortLevel) -> Option<ReasoningEffort> {
    Some(match level {
        EffortLevel::None => ReasoningEffort::None,
        EffortLevel::Low => ReasoningEffort::Low,
        EffortLevel::Medium => ReasoningEffort::Medium,
        EffortLevel::High => ReasoningEffort::High,
        EffortLevel::Xhigh => ReasoningEffort::XHigh,
        EffortLevel::Max => ReasoningEffort::Max,
        EffortLevel::Minimal => return None,
    })
}

fn anthropic_effort(level: EffortLevel) -> Option<AnthropicEffort> {
    Some(match level {
        EffortLevel::Low => AnthropicEffort::Low,
        EffortLevel::Medium => AnthropicEffort::Medium,
        EffortLevel::High => AnthropicEffort::High,
        EffortLevel::Xhigh => AnthropicEffort::XHigh,
        EffortLevel::Max => AnthropicEffort::Max,
        EffortLevel::None | EffortLevel::Minimal => return None,
    })
}

fn gemini_thinking_level(level: EffortLevel) -> Option<GeminiThinkingLevel> {
    Some(match level {
        EffortLevel::Minimal => GeminiThinkingLevel::Minimal,
        EffortLevel::Low => GeminiThinkingLevel::Low,
        EffortLevel::Medium => GeminiThinkingLevel::Medium,
        EffortLevel::High => GeminiThinkingLevel::High,
        EffortLevel::None | EffortLevel::Xhigh | EffortLevel::Max => return None,
    })
}

/// The provider-specific efforts a tag carries, each with its param path,
/// in the order the wire reads them (Gemini's nested level wins over the
/// flat one).
fn provider_efforts(tag: &ProviderTag) -> impl Iterator<Item = (&'static str, EffortLevel)> {
    let efforts: [Option<(&'static str, EffortLevel)>; 2] = match tag {
        ProviderTag::OpenAi(tag) => [
            tag.reasoning_effort.map(|effort| {
                (
                    PARAM_OPENAI_REASONING_EFFORT,
                    match effort {
                        ReasoningEffort::None => EffortLevel::None,
                        ReasoningEffort::Low => EffortLevel::Low,
                        ReasoningEffort::Medium => EffortLevel::Medium,
                        ReasoningEffort::High => EffortLevel::High,
                        ReasoningEffort::XHigh => EffortLevel::Xhigh,
                        ReasoningEffort::Max => EffortLevel::Max,
                    },
                )
            }),
            None,
        ],
        ProviderTag::Anthropic(tag) => [
            tag.effort.map(|effort| {
                (
                    PARAM_ANTHROPIC_EFFORT,
                    match effort {
                        AnthropicEffort::Low => EffortLevel::Low,
                        AnthropicEffort::Medium => EffortLevel::Medium,
                        AnthropicEffort::High => EffortLevel::High,
                        AnthropicEffort::XHigh => EffortLevel::Xhigh,
                        AnthropicEffort::Max => EffortLevel::Max,
                    },
                )
            }),
            None,
        ],
        ProviderTag::Gemini(tag) => [
            tag.thinking
                .as_ref()
                .and_then(|thinking| thinking.thinking_level)
                .map(|level| (PARAM_GEMINI_NESTED_THINKING_LEVEL, level.as_effort_level())),
            tag.thinking_level
                .map(|level| (PARAM_GEMINI_THINKING_LEVEL, level.as_effort_level())),
        ],
        ProviderTag::Unknown { .. } => [None, None],
    };
    efforts.into_iter().flatten()
}

/// The effort the request's params carry explicitly in a provider tag, if
/// any: the one the wire sends.
pub(crate) fn explicit_provider_effort(
    params: Option<&ProviderParamsOverride>,
) -> Option<EffortLevel> {
    params
        .and_then(|params| params.provider_tag.as_ref())
        .into_iter()
        .flat_map(provider_efforts)
        .map(|(_, level)| level)
        .next()
}

#[cfg(test)]
mod tests;
