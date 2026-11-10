//! Rows for the resolver: the facts policy, the translation table, the
//! conflicts, and the preference wrapper measured against the lowering it
//! replaces.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::lifecycle::run_primitive::{
    AnthropicEffort, AnthropicProviderTag, AnthropicThinkingConfig, GeminiProviderTag,
    GeminiThinkingConfig, GeminiThinkingLevel, OpaqueProviderBody, OpenAiProviderTag,
    ProviderParamsOverride, ProviderTag, ReasoningEffort, ReasoningLoweringBaseline,
    ReasoningLoweringOutcome, ReasoningMode, ReasoningNotAppliedReason,
    StructuredProviderExtension,
};
use crate::model_profile::capabilities::{
    AnthropicDisabledThinking, EffortLevel, ModelCapabilities, OpenAiReasoningContext,
    OpenAiReasoningMode, OpenAiTextVerbosity, ThinkingSupport,
};
use crate::model_profile::test_catalog::{
    ANTHROPIC_MODEL, OPENAI_MODEL, TEST_CATALOG, VIDEO_MODEL,
};
use crate::provider_matrix::{
    AnthropicBackendKind, GoogleBackendKind, OpenAiBackendKind, SelfHostedBackendKind,
};
use crate::reasoning_effort::{
    AccountLevelStatement, ModelApi, ReasoningBackend, ReasoningEffortDecidedBy,
    ReasoningEffortRefusal, ReasoningEffortRefusalReason as Reason, ReasoningEffortResolution,
    ReasoningFactsSource, ReasoningRoute, ReasoningRouteObservation,
};

use EffortLevel::{High, Low, Max, Medium, Minimal, Xhigh};

/// Every level, lowest first.
const LADDER: [EffortLevel; 7] = [EffortLevel::None, Minimal, Low, Medium, High, Xhigh, Max];
const ALL_LEVELS: &[EffortLevel] = &LADDER;

const ALL_APIS: [ModelApi; 5] = [
    ModelApi::Responses,
    ModelApi::ChatCompletions,
    ModelApi::Messages,
    ModelApi::GenerateContent,
    ModelApi::Realtime,
];

const MODEL: &str = "test-model";

const OPENAI_API: ReasoningBackend = ReasoningBackend::OpenAi(OpenAiBackendKind::OpenAiApi);
const OPENAI_COPILOT: ReasoningBackend = ReasoningBackend::OpenAi(OpenAiBackendKind::Copilot);
const ANTHROPIC_API: ReasoningBackend =
    ReasoningBackend::Anthropic(AnthropicBackendKind::AnthropicApi);
const ANTHROPIC_COPILOT: ReasoningBackend =
    ReasoningBackend::Anthropic(AnthropicBackendKind::Copilot);
const GOOGLE_GENAI: ReasoningBackend = ReasoningBackend::Google(GoogleBackendKind::GoogleGenAi);
const GOOGLE_COPILOT: ReasoningBackend = ReasoningBackend::Google(GoogleBackendKind::Copilot);

/// Every backend kind of the four families, in `ALL` order.
fn backends() -> Vec<ReasoningBackend> {
    let mut backends = Vec::new();
    backends.extend(
        OpenAiBackendKind::ALL
            .iter()
            .map(|kind| ReasoningBackend::OpenAi(*kind)),
    );
    backends.extend(
        AnthropicBackendKind::ALL
            .iter()
            .map(|kind| ReasoningBackend::Anthropic(*kind)),
    );
    backends.extend(
        GoogleBackendKind::ALL
            .iter()
            .map(|kind| ReasoningBackend::Google(*kind)),
    );
    backends.extend(
        SelfHostedBackendKind::ALL
            .iter()
            .map(|kind| ReasoningBackend::SelfHosted(*kind)),
    );
    assert_eq!(backends.len(), 15, "every kind of the four families");
    backends
}

/// The provider family a backend's models belong to.
fn family(backend: ReasoningBackend) -> Provider {
    match backend {
        ReasoningBackend::OpenAi(_) => Provider::OpenAI,
        ReasoningBackend::Anthropic(_) => Provider::Anthropic,
        ReasoningBackend::Google(_) => Provider::Gemini,
        ReasoningBackend::SelfHosted(_) => Provider::SelfHosted,
    }
}

/// A hand-written catalog row of `provider` that records `levels`: an OpenAI
/// row with reasoning, an Anthropic row, a Gemini thinking-level row.
fn row(provider: Provider, levels: &'static [EffortLevel]) -> ModelCapabilities {
    let model = match provider {
        Provider::OpenAI => OPENAI_MODEL,
        Provider::Anthropic => ANTHROPIC_MODEL,
        _ => VIDEO_MODEL,
    };
    let base = TEST_CATALOG
        .capabilities_for(
            match provider {
                Provider::OpenAI | Provider::Anthropic => provider,
                _ => Provider::Gemini,
            },
            model,
        )
        .expect("test catalog row");
    ModelCapabilities {
        provider,
        supports_reasoning: provider == Provider::OpenAI,
        thinking: match provider {
            Provider::Gemini => ThinkingSupport::GeminiThinkingLevel,
            Provider::Anthropic => ThinkingSupport::AnthropicAdaptiveOnly,
            _ => ThinkingSupport::None,
        },
        effort_levels: levels,
        ..*base
    }
}

/// The catalog row of a backend's family recording `levels`; none for the
/// self-hosted family, which has no catalog.
fn family_row(
    backend: ReasoningBackend,
    levels: &'static [EffortLevel],
) -> Option<ModelCapabilities> {
    match backend {
        ReasoningBackend::SelfHosted(_) => None,
        _ => Some(row(family(backend), levels)),
    }
}

/// An observation of `backend` on `api` that states nothing.
fn observation(backend: ReasoningBackend, api: ModelApi) -> ReasoningRouteObservation {
    ReasoningRouteObservation {
        backend,
        api,
        account: AccountLevelStatement::NotApplicable,
        operator_levels: None,
    }
}

/// An observation whose account list states `levels`.
fn account(
    backend: ReasoningBackend,
    api: ModelApi,
    levels: &[EffortLevel],
) -> ReasoningRouteObservation {
    ReasoningRouteObservation {
        account: AccountLevelStatement::Stated(levels.to_vec()),
        ..observation(backend, api)
    }
}

/// An observation whose operator configuration declares `levels`.
fn operator(
    backend: ReasoningBackend,
    api: ModelApi,
    levels: &[EffortLevel],
) -> ReasoningRouteObservation {
    ReasoningRouteObservation {
        operator_levels: Some(levels.to_vec()),
        ..observation(backend, api)
    }
}

/// An observation and a catalog row that state `levels` through every
/// source at once: whichever source the backend's policy names, it reads
/// `levels`.
fn stating_everywhere(
    backend: ReasoningBackend,
    api: ModelApi,
    levels: &'static [EffortLevel],
) -> (ReasoningRouteObservation, Option<ModelCapabilities>) {
    (
        ReasoningRouteObservation {
            backend,
            api,
            account: AccountLevelStatement::Stated(levels.to_vec()),
            operator_levels: Some(levels.to_vec()),
        },
        family_row(backend, levels),
    )
}

fn resolve(
    mode: ReasoningResolveMode,
    requested: EffortLevel,
    observation: Option<&ReasoningRouteObservation>,
    provider: Provider,
    catalog_row: Option<&ModelCapabilities>,
    params: &mut Option<ProviderParamsOverride>,
) -> Result<ReasoningEffortResolution, ReasoningEffortRefusal> {
    resolve_reasoning_effort(
        ReasoningResolveInput::new(
            requested,
            provider,
            MODEL,
            observation,
            catalog_row,
            match mode {
                ReasoningResolveMode::Strict => ReasoningEffortDecidedBy::Request,
                ReasoningResolveMode::Preference => ReasoningEffortDecidedBy::Preference,
            },
        ),
        params,
        mode,
    )
}

/// Strict resolution on an observed route of the backend's own family.
fn strict(
    requested: EffortLevel,
    observation: &ReasoningRouteObservation,
    catalog_row: Option<&ModelCapabilities>,
    params: &mut Option<ProviderParamsOverride>,
) -> Result<ReasoningEffortResolution, ReasoningEffortRefusal> {
    resolve(
        ReasoningResolveMode::Strict,
        requested,
        Some(observation),
        family(observation.backend),
        catalog_row,
        params,
    )
}

/// Preference resolution on an observed route of the backend's own family.
fn preference(
    requested: EffortLevel,
    observation: &ReasoningRouteObservation,
    catalog_row: Option<&ModelCapabilities>,
    params: &mut Option<ProviderParamsOverride>,
) -> Result<ReasoningEffortResolution, ReasoningEffortRefusal> {
    resolve(
        ReasoningResolveMode::Preference,
        requested,
        Some(observation),
        family(observation.backend),
        catalog_row,
        params,
    )
}

fn tagged(tag: ProviderTag) -> Option<ProviderParamsOverride> {
    Some(ProviderParamsOverride {
        provider_tag: Some(tag),
        ..Default::default()
    })
}

fn openai(tag: OpenAiProviderTag) -> Option<ProviderParamsOverride> {
    tagged(ProviderTag::OpenAi(tag))
}

fn anthropic(tag: AnthropicProviderTag) -> Option<ProviderParamsOverride> {
    tagged(ProviderTag::Anthropic(tag))
}

fn gemini(tag: GeminiProviderTag) -> Option<ProviderParamsOverride> {
    tagged(ProviderTag::Gemini(tag))
}

fn thinking(config: AnthropicThinkingConfig) -> Option<ProviderParamsOverride> {
    anthropic(AnthropicProviderTag {
        thinking: Some(config),
        ..Default::default()
    })
}

fn openai_tag_of(params: &Option<ProviderParamsOverride>) -> &OpenAiProviderTag {
    match params
        .as_ref()
        .and_then(|params| params.provider_tag.as_ref())
    {
        Some(ProviderTag::OpenAi(tag)) => tag,
        other => panic!("expected an openai tag, got {other:?}"),
    }
}

fn anthropic_tag_of(params: &Option<ProviderParamsOverride>) -> &AnthropicProviderTag {
    match params
        .as_ref()
        .and_then(|params| params.provider_tag.as_ref())
    {
        Some(ProviderTag::Anthropic(tag)) => tag,
        other => panic!("expected an anthropic tag, got {other:?}"),
    }
}

fn gemini_tag_of(params: &Option<ProviderParamsOverride>) -> &GeminiProviderTag {
    match params
        .as_ref()
        .and_then(|params| params.provider_tag.as_ref())
    {
        Some(ProviderTag::Gemini(tag)) => tag,
        other => panic!("expected a gemini tag, got {other:?}"),
    }
}

fn typed_openai(level: EffortLevel) -> ReasoningEffort {
    match level {
        EffortLevel::None => ReasoningEffort::None,
        Low => ReasoningEffort::Low,
        Medium => ReasoningEffort::Medium,
        High => ReasoningEffort::High,
        Xhigh => ReasoningEffort::XHigh,
        Max => ReasoningEffort::Max,
        Minimal => panic!("the typed OpenAI effort has no minimal"),
    }
}

fn typed_anthropic(level: EffortLevel) -> AnthropicEffort {
    match level {
        Low => AnthropicEffort::Low,
        Medium => AnthropicEffort::Medium,
        High => AnthropicEffort::High,
        Xhigh => AnthropicEffort::XHigh,
        Max => AnthropicEffort::Max,
        EffortLevel::None | Minimal => panic!("the typed Anthropic effort has no {level:?}"),
    }
}

fn typed_gemini(level: EffortLevel) -> GeminiThinkingLevel {
    match level {
        Minimal => GeminiThinkingLevel::Minimal,
        Low => GeminiThinkingLevel::Low,
        Medium => GeminiThinkingLevel::Medium,
        High => GeminiThinkingLevel::High,
        EffortLevel::None | Xhigh | Max => panic!("Gemini has no thinking level {level:?}"),
    }
}

fn owned(fields: &[&str]) -> Vec<String> {
    fields.iter().map(|field| (*field).to_owned()).collect()
}

// ---------------------------------------------------------------------
// R1.9
// ---------------------------------------------------------------------

/// The facts policy, kind by kind: which source states the levels of a
/// route on each backend kind. Every kind of every family is listed, so a
/// new kind fails here until it has decided.
#[test]
fn policy_methods_are_exhaustive_per_kind() {
    use ReasoningFactsSource::{Account, Catalog, Operator};

    let openai = [
        (OpenAiBackendKind::OpenAiApi, Catalog),
        (OpenAiBackendKind::ChatGptBackend, Catalog),
        (OpenAiBackendKind::AzureOpenAi, Catalog),
        (OpenAiBackendKind::Copilot, Account),
    ];
    assert_eq!(openai.len(), OpenAiBackendKind::ALL.len());
    for (kind, source) in openai {
        assert!(OpenAiBackendKind::ALL.contains(&kind));
        assert_eq!(kind.reasoning_levels_source(), source, "{}", kind.as_str());
        assert_eq!(ReasoningBackend::OpenAi(kind).levels_source(), source);
    }

    let anthropic = [
        (AnthropicBackendKind::AnthropicApi, Catalog),
        (AnthropicBackendKind::Bedrock, Catalog),
        (AnthropicBackendKind::Vertex, Catalog),
        (AnthropicBackendKind::Foundry, Catalog),
        (AnthropicBackendKind::Copilot, Account),
    ];
    assert_eq!(anthropic.len(), AnthropicBackendKind::ALL.len());
    for (kind, source) in anthropic {
        assert!(AnthropicBackendKind::ALL.contains(&kind));
        assert_eq!(kind.reasoning_levels_source(), source, "{}", kind.as_str());
        assert_eq!(ReasoningBackend::Anthropic(kind).levels_source(), source);
    }

    let google = [
        (GoogleBackendKind::GoogleGenAi, Catalog),
        (GoogleBackendKind::VertexAi, Catalog),
        (GoogleBackendKind::GoogleCodeAssist, Catalog),
        (GoogleBackendKind::Copilot, Account),
    ];
    assert_eq!(google.len(), GoogleBackendKind::ALL.len());
    for (kind, source) in google {
        assert!(GoogleBackendKind::ALL.contains(&kind));
        assert_eq!(kind.reasoning_levels_source(), source, "{}", kind.as_str());
        assert_eq!(ReasoningBackend::Google(kind).levels_source(), source);
    }

    let self_hosted = [
        (SelfHostedBackendKind::SelfHosted, Operator),
        (SelfHostedBackendKind::OpenAiCompatible, Operator),
    ];
    assert_eq!(self_hosted.len(), SelfHostedBackendKind::ALL.len());
    for (kind, source) in self_hosted {
        assert!(SelfHostedBackendKind::ALL.contains(&kind));
        assert_eq!(kind.reasoning_levels_source(), source, "{}", kind.as_str());
        assert_eq!(ReasoningBackend::SelfHosted(kind).levels_source(), source);
    }

    // The policy answers in the type a refusal and a resolution report as
    // `facts`: there is one source vocabulary, not two.
    for (source, wire) in [
        (Catalog, "catalog"),
        (Account, "account"),
        (Operator, "operator"),
    ] {
        assert_eq!(
            serde_json::to_value(source).expect("a facts source serializes"),
            serde_json::json!(wire)
        );
    }

    // The account's list is the source on exactly the Copilot kinds.
    for backend in backends() {
        assert_eq!(
            backend.levels_source() == Account,
            backend.is_copilot(),
            "{}",
            backend.as_str()
        );
    }
}

// ---------------------------------------------------------------------
// R1.1
// ---------------------------------------------------------------------

/// What the translation table gives one route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Golden {
    /// `OpenAiProviderTag.reasoning_effort` and the resolved marker.
    OpenAi { fields: &'static [&'static str] },
    /// `GeminiProviderTag.thinking_level`.
    Gemini {
        expressible: &'static [EffortLevel],
        fields: &'static [&'static str],
    },
    /// `AnthropicProviderTag.effort` alone.
    Anthropic,
    /// `AnthropicProviderTag.effort` and `thinking = Adaptive`.
    AnthropicPair,
    /// The route takes no level Meerkat can write.
    NoEffort,
}

const OPENAI_EXPRESSIBLE: &[EffortLevel] = &[EffortLevel::None, Low, Medium, High, Xhigh, Max];
const ANTHROPIC_EXPRESSIBLE: &[EffortLevel] = &[Low, Medium, High, Xhigh, Max];
const GEMINI_CHAT_EXPRESSIBLE: &[EffortLevel] = &[Low, Medium, High];
const GEMINI_GENERATE_EXPRESSIBLE: &[EffortLevel] = &[Minimal, Low, Medium, High];

const RESPONSES_FIELDS: &[&str] = &["reasoning.effort"];
const CHAT_FIELDS: &[&str] = &["reasoning_effort", "reasoning.effort"];
const MESSAGES_FIELDS: &[&str] = &["output_config.effort"];
const MESSAGES_PAIR_FIELDS: &[&str] = &["thinking.type", "output_config.effort"];
const GENERATE_FIELDS: &[&str] = &["generationConfig.thinkingConfig.thinkingLevel"];

/// The golden table: every API for the OpenAI family and the self-hosted
/// family (which speaks the OpenAI APIs).
const OPENAI_FAMILY: [(ModelApi, Golden); 5] = [
    (
        ModelApi::Responses,
        Golden::OpenAi {
            fields: RESPONSES_FIELDS,
        },
    ),
    (
        ModelApi::ChatCompletions,
        Golden::OpenAi {
            fields: CHAT_FIELDS,
        },
    ),
    (ModelApi::Messages, Golden::NoEffort),
    (ModelApi::GenerateContent, Golden::NoEffort),
    (ModelApi::Realtime, Golden::NoEffort),
];

/// The golden table: every API for the Anthropic family on a direct backend.
const ANTHROPIC_DIRECT: [(ModelApi, Golden); 5] = [
    (ModelApi::Responses, Golden::NoEffort),
    (ModelApi::ChatCompletions, Golden::NoEffort),
    (ModelApi::Messages, Golden::Anthropic),
    (ModelApi::GenerateContent, Golden::NoEffort),
    (ModelApi::Realtime, Golden::NoEffort),
];

/// The golden table: every API for the Anthropic family on Copilot.
const ANTHROPIC_ON_COPILOT: [(ModelApi, Golden); 5] = [
    (ModelApi::Responses, Golden::NoEffort),
    (ModelApi::ChatCompletions, Golden::NoEffort),
    (ModelApi::Messages, Golden::AnthropicPair),
    (ModelApi::GenerateContent, Golden::NoEffort),
    (ModelApi::Realtime, Golden::NoEffort),
];

/// The golden table: every API for the Gemini family.
const GOOGLE_FAMILY: [(ModelApi, Golden); 5] = [
    (ModelApi::Responses, Golden::NoEffort),
    (
        ModelApi::ChatCompletions,
        Golden::Gemini {
            expressible: GEMINI_CHAT_EXPRESSIBLE,
            fields: CHAT_FIELDS,
        },
    ),
    (ModelApi::Messages, Golden::NoEffort),
    (
        ModelApi::GenerateContent,
        Golden::Gemini {
            expressible: GEMINI_GENERATE_EXPRESSIBLE,
            fields: GENERATE_FIELDS,
        },
    ),
    (ModelApi::Realtime, Golden::NoEffort),
];

fn golden_table(backend: ReasoningBackend) -> [(ModelApi, Golden); 5] {
    match backend {
        ReasoningBackend::OpenAi(_) | ReasoningBackend::SelfHosted(_) => OPENAI_FAMILY,
        ReasoningBackend::Anthropic(AnthropicBackendKind::Copilot) => ANTHROPIC_ON_COPILOT,
        ReasoningBackend::Anthropic(_) => ANTHROPIC_DIRECT,
        ReasoningBackend::Google(_) => GOOGLE_FAMILY,
    }
}

impl Golden {
    fn expressible(self) -> &'static [EffortLevel] {
        match self {
            Self::OpenAi { .. } => OPENAI_EXPRESSIBLE,
            Self::Gemini { expressible, .. } => expressible,
            Self::Anthropic | Self::AnthropicPair => ANTHROPIC_EXPRESSIBLE,
            Self::NoEffort => &[],
        }
    }

    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::OpenAi { fields } | Self::Gemini { fields, .. } => fields,
            Self::Anthropic => MESSAGES_FIELDS,
            Self::AnthropicPair => MESSAGES_PAIR_FIELDS,
            Self::NoEffort => &[],
        }
    }

    /// The whole request copy after `level` was written onto an absent one:
    /// the route's typed field and nothing else.
    fn written(self, level: EffortLevel) -> Option<ProviderParamsOverride> {
        match self {
            Self::OpenAi { .. } => openai(OpenAiProviderTag {
                reasoning_effort: Some(typed_openai(level)),
                reasoning_effort_resolved: true,
                ..Default::default()
            }),
            Self::Gemini { .. } => gemini(GeminiProviderTag {
                thinking_level: Some(typed_gemini(level)),
                ..Default::default()
            }),
            Self::Anthropic => anthropic(AnthropicProviderTag {
                effort: Some(typed_anthropic(level)),
                ..Default::default()
            }),
            Self::AnthropicPair => anthropic(AnthropicProviderTag {
                effort: Some(typed_anthropic(level)),
                thinking: Some(AnthropicThinkingConfig::Adaptive),
                ..Default::default()
            }),
            Self::NoEffort => panic!("a route with no effort writes nothing"),
        }
    }
}

/// For every backend kind, every API and every level, with every source
/// stating every level: exactly the golden typed field is written and
/// reported, or the level is refused as unsupported with the levels the
/// route can express.
#[test]
fn translation_table_covers_every_route_and_level() {
    let mut cases = 0;
    let mut written = 0;
    for backend in backends() {
        let table = golden_table(backend);
        assert_eq!(
            table.map(|(api, _)| api),
            ALL_APIS,
            "the golden table lists every API"
        );
        for (api, golden) in table {
            let (observation, catalog_row) = stating_everywhere(backend, api, &LADDER);
            let facts = backend.levels_source();
            for level in LADDER {
                cases += 1;
                let context = format!("{}/{api} at {level:?}", backend.as_str());
                let mut params = None;
                let outcome = strict(level, &observation, catalog_row.as_ref(), &mut params);
                if golden.expressible().contains(&level) {
                    written += 1;
                    assert_eq!(
                        outcome,
                        Ok(ReasoningEffortResolution {
                            requested: level,
                            level,
                            provider: family(backend),
                            model: MODEL.to_owned(),
                            route: ReasoningRoute::new(backend, api),
                            fields: owned(golden.fields()),
                            facts,
                            decided_by: ReasoningEffortDecidedBy::Request,
                        }),
                        "{context}"
                    );
                    assert_eq!(params, golden.written(level), "{context}");
                } else {
                    assert_eq!(
                        outcome,
                        Err(ReasoningEffortRefusal {
                            requested: level,
                            provider: family(backend),
                            model: MODEL.to_owned(),
                            route: Some(ReasoningRoute::new(backend, api)),
                            reason: Reason::UnsupportedLevel,
                            supported: Some(golden.expressible().to_vec()),
                            facts: Some(facts),
                            conflicting_param: None,
                            conflicting_level: None,
                        }),
                        "{context}"
                    );
                    assert_eq!(params, None, "{context}: a refusal writes nothing");
                }
            }
        }
    }
    assert_eq!(cases, 15 * 5 * 7);
    // OpenAI and self-hosted kinds: 6 kinds, 2 APIs, 6 levels. Gemini: 4
    // kinds, 3 levels on Chat Completions and 4 on generateContent.
    // Anthropic: 5 kinds, 5 levels on Messages.
    assert_eq!(written, 6 * 2 * 6 + 4 * (3 + 4) + 5 * 5);
}

/// The realtime API takes no effort on any backend, whatever the sources
/// state: `supported: []`.
#[test]
fn the_realtime_api_takes_no_effort() {
    for backend in backends() {
        let (observation, catalog_row) = stating_everywhere(backend, ModelApi::Realtime, &LADDER);
        assert_eq!(
            supported_reasoning_levels(family(backend), Some(&observation), catalog_row.as_ref()),
            ReasoningLevelFacts::Stated {
                levels: Vec::new(),
                source: backend.levels_source(),
            },
            "{}",
            backend.as_str()
        );
    }
}

// ---------------------------------------------------------------------
// R1.2, R1.3
// ---------------------------------------------------------------------

/// The direct routes of the three catalog-backed families.
fn direct_routes() -> Vec<(ReasoningBackend, ModelApi, &'static [EffortLevel])> {
    let mut routes = Vec::new();
    for kind in OpenAiBackendKind::ALL {
        if *kind != OpenAiBackendKind::Copilot {
            for api in [ModelApi::Responses, ModelApi::ChatCompletions] {
                routes.push((ReasoningBackend::OpenAi(*kind), api, OPENAI_EXPRESSIBLE));
            }
        }
    }
    for kind in AnthropicBackendKind::ALL {
        if *kind != AnthropicBackendKind::Copilot {
            routes.push((
                ReasoningBackend::Anthropic(*kind),
                ModelApi::Messages,
                ANTHROPIC_EXPRESSIBLE,
            ));
        }
    }
    for kind in GoogleBackendKind::ALL {
        if *kind != GoogleBackendKind::Copilot {
            routes.push((
                ReasoningBackend::Google(*kind),
                ModelApi::GenerateContent,
                GEMINI_GENERATE_EXPRESSIBLE,
            ));
        }
    }
    assert_eq!(routes.len(), 3 * 2 + 4 + 3);
    routes
}

/// What a catalog row states for its family, written out by hand: the
/// levels of an OpenAI row with reasoning, of an Anthropic row, of a Gemini
/// thinking-level row; none (`Some([])`) for a row that takes no effort;
/// unknown (`None`) for a Gemini thinking-level row that records none.
fn stated_by(row: &ModelCapabilities) -> Option<&'static [EffortLevel]> {
    match row.provider {
        Provider::OpenAI if row.supports_reasoning => Some(row.effort_levels),
        Provider::OpenAI => Some(&[]),
        Provider::Anthropic => Some(row.effort_levels),
        Provider::Gemini => match row.thinking {
            ThinkingSupport::GeminiThinkingLevel if row.effort_levels.is_empty() => None,
            ThinkingSupport::GeminiThinkingLevel => Some(row.effort_levels),
            _ => Some(&[]),
        },
        Provider::SelfHosted | Provider::Other => panic!("no catalog rows for {:?}", row.provider),
    }
}

/// On a direct route the catalog row is the source: `supported` is the
/// row's levels intersected with the levels the route can express, for
/// every row of the injected catalog and for hand-written rows of every
/// shape.
#[test]
fn catalog_rows_drive_direct_routes() {
    let mut rows: Vec<ModelCapabilities> = TEST_CATALOG.capabilities.to_vec();
    assert!(rows.len() >= 3, "the injected catalog has a row per family");
    for provider in [Provider::OpenAI, Provider::Anthropic, Provider::Gemini] {
        for levels in [
            ALL_LEVELS,
            &[EffortLevel::None, Low, Medium, High],
            &[Minimal, Low, High],
            &[Low, Medium, High, Xhigh, Max],
            &[Max],
            &[],
        ] {
            rows.push(row(provider, levels));
        }
    }
    // An OpenAI row without reasoning and a Gemini row that is not a
    // thinking-level row take no effort, whatever levels they list.
    rows.push(ModelCapabilities {
        supports_reasoning: false,
        ..row(Provider::OpenAI, &LADDER)
    });
    rows.push(ModelCapabilities {
        thinking: ThinkingSupport::None,
        ..row(Provider::Gemini, &LADDER)
    });

    let mut checked = 0;
    for row in &rows {
        for (backend, api, expressible) in direct_routes() {
            if family(backend) != row.provider {
                continue;
            }
            checked += 1;
            let expected = match stated_by(row) {
                None => ReasoningLevelFacts::Unknown,
                Some(stated) => ReasoningLevelFacts::Stated {
                    levels: LADDER
                        .into_iter()
                        .filter(|level| stated.contains(level) && expressible.contains(level))
                        .collect(),
                    source: ReasoningFactsSource::Catalog,
                },
            };
            let observation = observation(backend, api);
            assert_eq!(
                supported_reasoning_levels(row.provider, Some(&observation), Some(row)),
                expected,
                "{} on {}/{api}",
                row.id,
                backend.as_str()
            );
            // The statements of the other sources change nothing there.
            let (noisy, _) = stating_everywhere(backend, api, &LADDER);
            assert_eq!(
                supported_reasoning_levels(row.provider, Some(&noisy), Some(row)),
                expected,
                "{} on {}/{api}: only the row is read",
                row.id,
                backend.as_str()
            );
        }
    }
    assert!(checked >= 3 * 7 * 3, "every family's rows met its routes");
}

/// A model with no catalog row, or a row of another provider family, on a
/// catalog-backed route: unknown, reported as `supported: null`,
/// `facts: null`.
#[test]
fn uncatalogued_model_on_a_catalog_route_is_unknown() {
    for (backend, api, _) in direct_routes() {
        let observation = observation(backend, api);
        let other_family = row(
            match family(backend) {
                Provider::OpenAI => Provider::Anthropic,
                _ => Provider::OpenAI,
            },
            &LADDER,
        );
        for catalog_row in [None, Some(&other_family)] {
            assert_eq!(
                supported_reasoning_levels(family(backend), Some(&observation), catalog_row),
                ReasoningLevelFacts::Unknown
            );
            for level in LADDER {
                let mut params = None;
                let refusal = strict(level, &observation, catalog_row, &mut params)
                    .expect_err("no source states the levels");
                assert_eq!(
                    refusal,
                    ReasoningEffortRefusal {
                        requested: level,
                        provider: family(backend),
                        model: MODEL.to_owned(),
                        route: Some(ReasoningRoute::new(backend, api)),
                        reason: Reason::SupportedLevelsUnknown,
                        supported: None,
                        facts: None,
                        conflicting_param: None,
                        conflicting_level: None,
                    }
                );
                let value = serde_json::to_value(&refusal).expect("a refusal serializes");
                assert_eq!(value["reason"], "supported_levels_unknown");
                assert!(value["supported"].is_null());
                assert!(value["facts"].is_null());
                assert_eq!(params, None);
            }
        }
    }
}

/// A Gemini thinking-level row that records no levels is unknown, not
/// empty.
#[test]
fn a_gemini_thinking_level_row_without_levels_is_unknown() {
    let unrecorded = row(Provider::Gemini, &[]);
    let observation = observation(GOOGLE_GENAI, ModelApi::GenerateContent);
    assert_eq!(
        supported_reasoning_levels(Provider::Gemini, Some(&observation), Some(&unrecorded)),
        ReasoningLevelFacts::Unknown
    );
    let mut params = None;
    let refusal = strict(Low, &observation, Some(&unrecorded), &mut params)
        .expect_err("the row records no levels");
    assert_eq!(refusal.reason, Reason::SupportedLevelsUnknown);
    assert_eq!((refusal.supported, refusal.facts), (None, None));
}

// ---------------------------------------------------------------------
// R1.6, R1.7
// ---------------------------------------------------------------------

/// The Copilot routes of the three families and the levels each can
/// express.
fn copilot_routes() -> [(ReasoningBackend, ModelApi, &'static [EffortLevel]); 4] {
    [
        (OPENAI_COPILOT, ModelApi::Responses, OPENAI_EXPRESSIBLE),
        (
            OPENAI_COPILOT,
            ModelApi::ChatCompletions,
            OPENAI_EXPRESSIBLE,
        ),
        (ANTHROPIC_COPILOT, ModelApi::Messages, ANTHROPIC_EXPRESSIBLE),
        (
            GOOGLE_COPILOT,
            ModelApi::ChatCompletions,
            GEMINI_CHAT_EXPRESSIBLE,
        ),
    ]
}

/// On the Copilot kind of each family the account's statement is the only
/// source of levels: stated levels (intersected with what the route can
/// express) with facts `account`, unknown for an entry that states none,
/// unavailable for a list that is not loaded. A catalog row for the same
/// id changes none of them.
#[test]
fn copilot_levels_come_from_the_account_statement_only() {
    const STATED: &[EffortLevel] = &[EffortLevel::None, Minimal, Low, High, Max];
    for (backend, api, expressible) in copilot_routes() {
        let provider = family(backend);
        let context = format!("{}/{api} ({provider:?})", backend.as_str());
        let stated = account(backend, api, STATED);
        let expected_levels: Vec<EffortLevel> = LADDER
            .into_iter()
            .filter(|level| STATED.contains(level) && expressible.contains(level))
            .collect();
        let statements = [
            (
                stated.clone(),
                ReasoningLevelFacts::Stated {
                    levels: expected_levels.clone(),
                    source: ReasoningFactsSource::Account,
                },
            ),
            (
                account(backend, api, &[]),
                ReasoningLevelFacts::Stated {
                    levels: Vec::new(),
                    source: ReasoningFactsSource::Account,
                },
            ),
            (
                ReasoningRouteObservation {
                    account: AccountLevelStatement::NotStated,
                    ..observation(backend, api)
                },
                ReasoningLevelFacts::Unknown,
            ),
            (
                ReasoningRouteObservation {
                    account: AccountLevelStatement::ListUnavailable,
                    ..observation(backend, api)
                },
                ReasoningLevelFacts::Unavailable,
            ),
            (observation(backend, api), ReasoningLevelFacts::Unknown),
            // Operator levels are not a Copilot source either.
            (
                operator(backend, api, &LADDER),
                ReasoningLevelFacts::Unknown,
            ),
        ];
        // Catalog rows that disagree with the account in every direction.
        let full_row = row(provider, &LADDER);
        let empty_row = row(provider, &[]);
        let disjoint_row = row(provider, &[Medium, Xhigh]);
        for (observation, expected) in &statements {
            for catalog_row in [None, Some(&full_row), Some(&empty_row), Some(&disjoint_row)] {
                assert_eq!(
                    &supported_reasoning_levels(provider, Some(observation), catalog_row),
                    expected,
                    "{context}: {:?} with row {:?}",
                    observation.account,
                    catalog_row.map(|row| row.effort_levels)
                );
            }
        }

        // The same through the resolver: a stated level is sent with facts
        // `account`, an unstated one is refused with the stated set, and
        // the two other statements refuse every level.
        for catalog_row in [None, Some(&full_row), Some(&empty_row), Some(&disjoint_row)] {
            for level in LADDER {
                let mut params = None;
                let outcome = strict(level, &stated, catalog_row, &mut params);
                if expected_levels.contains(&level) {
                    let resolution = outcome.expect("a stated level resolves");
                    assert_eq!(resolution.facts, ReasoningFactsSource::Account, "{context}");
                    assert_eq!(resolution.level, level);
                    assert!(params.is_some());
                } else {
                    let refusal = outcome.expect_err("an unstated level is refused");
                    assert_eq!(refusal.reason, Reason::UnsupportedLevel, "{context}");
                    assert_eq!(refusal.supported, Some(expected_levels.clone()));
                    assert_eq!(refusal.facts, Some(ReasoningFactsSource::Account));
                    assert_eq!(params, None);
                }

                for (statement, reason) in [
                    (
                        AccountLevelStatement::NotStated,
                        Reason::SupportedLevelsUnknown,
                    ),
                    (
                        AccountLevelStatement::ListUnavailable,
                        Reason::SupportedLevelsUnavailable,
                    ),
                    (
                        AccountLevelStatement::NotApplicable,
                        Reason::SupportedLevelsUnknown,
                    ),
                ] {
                    let observation = ReasoningRouteObservation {
                        account: statement,
                        ..observation(backend, api)
                    };
                    let mut params = None;
                    let refusal = strict(level, &observation, catalog_row, &mut params)
                        .expect_err("no stated levels");
                    assert_eq!(refusal.reason, reason, "{context}");
                    assert_eq!((refusal.supported, refusal.facts), (None, None));
                    assert_eq!(refusal.route, Some(ReasoningRoute::new(backend, api)));
                    assert_eq!(params, None);
                }
            }
        }
    }
}

/// A level the account's list does not state is refused with the stated
/// set and facts `account`, even when a catalog row lists it.
#[test]
fn a_level_the_list_does_not_state_is_refused() {
    let stated = account(
        OPENAI_COPILOT,
        ModelApi::Responses,
        &[Low, Medium, High, Xhigh],
    );
    let catalog_row = row(
        Provider::OpenAI,
        &[EffortLevel::None, Low, Medium, High, Xhigh],
    );
    let mut params = None;
    let refusal = strict(EffortLevel::None, &stated, Some(&catalog_row), &mut params)
        .expect_err("the list does not state none");
    assert_eq!(
        refusal,
        ReasoningEffortRefusal {
            requested: EffortLevel::None,
            provider: Provider::OpenAI,
            model: MODEL.to_owned(),
            route: Some(ReasoningRoute::new(OPENAI_COPILOT, ModelApi::Responses)),
            reason: Reason::UnsupportedLevel,
            supported: Some(vec![Low, Medium, High, Xhigh]),
            facts: Some(ReasoningFactsSource::Account),
            conflicting_param: None,
            conflicting_level: None,
        }
    );
    assert_eq!(
        serde_json::to_value(&refusal).expect("a refusal serializes"),
        serde_json::json!({
            "requested": "none", "provider": "openai", "model": MODEL,
            "route": {"backend": "copilot", "api": "responses"},
            "reason": "unsupported_level", "supported": ["low", "medium", "high", "xhigh"],
            "facts": "account"
        })
    );
    assert_eq!(params, None);
}

/// Operator-declared levels are the source on the self-hosted kinds, and
/// nothing else is.
#[test]
fn operator_levels_drive_self_hosted_routes() {
    for kind in SelfHostedBackendKind::ALL {
        let backend = ReasoningBackend::SelfHosted(*kind);
        for api in [ModelApi::Responses, ModelApi::ChatCompletions] {
            let declared = operator(backend, api, &[Minimal, Low, High]);
            assert_eq!(
                supported_reasoning_levels(Provider::SelfHosted, Some(&declared), None),
                ReasoningLevelFacts::Stated {
                    levels: vec![Low, High],
                    source: ReasoningFactsSource::Operator,
                }
            );
            let mut params = None;
            let resolution =
                strict(High, &declared, None, &mut params).expect("a declared level resolves");
            assert_eq!(resolution.facts, ReasoningFactsSource::Operator);
            assert_eq!(resolution.provider, Provider::SelfHosted);
            assert_eq!(
                openai_tag_of(&params).reasoning_effort,
                Some(ReasoningEffort::High)
            );

            // Nothing declared: unknown, whatever an account or a row says.
            let undeclared = account(backend, api, &LADDER);
            let stray_row = row(Provider::OpenAI, &LADDER);
            for catalog_row in [None, Some(&stray_row)] {
                assert_eq!(
                    supported_reasoning_levels(
                        Provider::SelfHosted,
                        Some(&undeclared),
                        catalog_row
                    ),
                    ReasoningLevelFacts::Unknown
                );
            }
            let mut params = None;
            let refusal =
                strict(High, &undeclared, None, &mut params).expect_err("nothing is declared");
            assert_eq!(refusal.reason, Reason::SupportedLevelsUnknown);
            assert_eq!(params, None);
        }
    }
}

/// `supported` is a set shown lowest level first: a source that repeats or
/// reorders levels states the same set.
#[test]
fn supported_levels_are_in_ladder_order_without_repeats() {
    // The resolver's ladder is every level, lowest first. The match has no
    // wildcard arm: a new level fails here until the ladder lists it.
    fn position(level: EffortLevel) -> usize {
        match level {
            EffortLevel::None => 0,
            Minimal => 1,
            Low => 2,
            Medium => 3,
            High => 4,
            Xhigh => 5,
            Max => 6,
        }
    }
    assert_eq!(LEVEL_LADDER, LADDER);
    for (index, level) in LEVEL_LADDER.into_iter().enumerate() {
        assert_eq!(position(level), index);
    }
    assert!(LEVEL_LADDER.is_sorted());

    let shuffled = account(
        OPENAI_COPILOT,
        ModelApi::Responses,
        &[Max, Low, Minimal, High, Low, EffortLevel::None, Max],
    );
    assert_eq!(
        supported_reasoning_levels(Provider::OpenAI, Some(&shuffled), None),
        ReasoningLevelFacts::Stated {
            levels: vec![EffortLevel::None, Low, High, Max],
            source: ReasoningFactsSource::Account,
        }
    );
    let facts = supported_reasoning_levels(Provider::OpenAI, Some(&shuffled), None);
    assert_eq!(
        facts.supported(),
        Some(&[EffortLevel::None, Low, High, Max][..])
    );
    assert_eq!(facts.source(), Some(ReasoningFactsSource::Account));
    assert_eq!(ReasoningLevelFacts::Unknown.supported(), None);
    assert_eq!(ReasoningLevelFacts::Unknown.source(), None);
    assert_eq!(ReasoningLevelFacts::Unavailable.supported(), None);
    assert_eq!(ReasoningLevelFacts::Unavailable.source(), None);
}

// ---------------------------------------------------------------------
// R1.8
// ---------------------------------------------------------------------

/// A level resolved on an OpenAI-family route is written with the resolved
/// marker, in both modes; a refused resolve leaves the request copy
/// byte-identical, marker included.
#[test]
fn the_openai_translation_marks_the_level_resolved() {
    let catalog_row = row(Provider::OpenAI, &[Low, Medium, High]);
    let routes = [
        (
            observation(OPENAI_API, ModelApi::Responses),
            Some(&catalog_row),
        ),
        (
            observation(OPENAI_API, ModelApi::ChatCompletions),
            Some(&catalog_row),
        ),
        (
            account(OPENAI_COPILOT, ModelApi::Responses, &[Low, Medium, High]),
            None,
        ),
        (
            operator(
                ReasoningBackend::SelfHosted(SelfHostedBackendKind::OpenAiCompatible),
                ModelApi::ChatCompletions,
                &[Low, Medium, High],
            ),
            None,
        ),
    ];
    for (observation, catalog_row) in &routes {
        for mode in [
            ReasoningResolveMode::Strict,
            ReasoningResolveMode::Preference,
        ] {
            let baseline = Some(ProviderParamsOverride {
                temperature: Some(0.2),
                provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                    seed: Some(7),
                    ..Default::default()
                })),
                ..Default::default()
            });

            let mut params = baseline.clone();
            resolve(
                mode,
                High,
                Some(observation),
                family(observation.backend),
                *catalog_row,
                &mut params,
            )
            .expect("a supported level resolves");
            let tag = openai_tag_of(&params);
            assert_eq!(tag.reasoning_effort, Some(ReasoningEffort::High));
            assert!(tag.reasoning_effort_resolved, "the marker is set");
            assert_eq!(tag.seed, Some(7), "unrelated knobs stay");
            assert_eq!(
                params.as_ref().and_then(|params| params.temperature),
                Some(0.2)
            );

            let mut refused = baseline.clone();
            let refusal = resolve(
                mode,
                Max,
                Some(observation),
                family(observation.backend),
                *catalog_row,
                &mut refused,
            )
            .expect_err("max is not supported");
            assert_eq!(refusal.reason, Reason::UnsupportedLevel);
            assert_eq!(refused, baseline, "a refusal leaves the copy untouched");
            assert!(!openai_tag_of(&refused).reasoning_effort_resolved);

            let mut absent = None;
            resolve(
                mode,
                Max,
                Some(observation),
                family(observation.backend),
                *catalog_row,
                &mut absent,
            )
            .expect_err("max is not supported");
            assert_eq!(absent, None, "a refusal creates no params");
        }
    }
}

// ---------------------------------------------------------------------
// R1.10
// ---------------------------------------------------------------------

/// With no observation (a client that reports none) the levels are unknown
/// and the refusal names the family's direct route.
#[test]
fn a_refusal_without_an_observation_names_the_direct_route() {
    let direct = [
        (Provider::OpenAI, OPENAI_API, ModelApi::Responses),
        (Provider::Anthropic, ANTHROPIC_API, ModelApi::Messages),
        (Provider::Gemini, GOOGLE_GENAI, ModelApi::GenerateContent),
    ];
    for (provider, backend, api) in direct {
        assert_eq!(
            direct_route_observation(provider),
            Some(observation(backend, api)),
            "{provider:?}"
        );
        let catalog_row = row(provider, &LADDER);
        for catalog_row in [None, Some(&catalog_row)] {
            assert_eq!(
                supported_reasoning_levels(provider, None, catalog_row),
                ReasoningLevelFacts::Unknown
            );
            for mode in [
                ReasoningResolveMode::Strict,
                ReasoningResolveMode::Preference,
            ] {
                let mut params = None;
                let refusal = resolve(mode, High, None, provider, catalog_row, &mut params)
                    .expect_err("no observation, no source");
                assert_eq!(refusal.reason, Reason::SupportedLevelsUnknown);
                assert_eq!((refusal.supported, refusal.facts), (None, None));
                assert_eq!(refusal.route, Some(ReasoningRoute::new(backend, api)));
                assert_eq!(refusal.provider, provider);
                assert_eq!(params, None);
            }
        }
    }

    // The self-hosted family and a provider outside the families have no
    // direct observation; their levels are unknown all the same. The
    // self-hosted refusal names that family's own route. A provider outside
    // the families has no route to name, and none is guessed: `route: null`.
    let self_hosted = ReasoningRoute::new(
        ReasoningBackend::SelfHosted(SelfHostedBackendKind::SelfHosted),
        ModelApi::ChatCompletions,
    );
    for (provider, route) in [
        (Provider::SelfHosted, Some(self_hosted)),
        (Provider::Other, None),
    ] {
        assert_eq!(direct_route_observation(provider), None);
        assert_eq!(
            supported_reasoning_levels(provider, None, None),
            ReasoningLevelFacts::Unknown
        );
        for mode in [
            ReasoningResolveMode::Strict,
            ReasoningResolveMode::Preference,
        ] {
            let mut params = None;
            let refusal = resolve(mode, High, None, provider, None, &mut params)
                .expect_err("no observation, no source");
            assert_eq!(refusal.reason, Reason::SupportedLevelsUnknown);
            assert_eq!((refusal.supported.clone(), refusal.facts), (None, None));
            assert_eq!(refusal.route, route, "{provider:?}");
            assert_eq!(params, None);

            let value = serde_json::to_value(&refusal).expect("a refusal serializes");
            let text = refusal.to_string();
            match &route {
                Some(route) => {
                    assert_eq!(
                        value["route"],
                        serde_json::json!({"backend": "self_hosted", "api": "chat_completions"})
                    );
                    assert!(text.contains(&format!(" on {route}:")), "{text}");
                }
                None => {
                    assert!(value["route"].is_null(), "{value}");
                    assert!(!text.contains(" on "), "{text}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// R1.11
// ---------------------------------------------------------------------

/// On `copilot/messages` a level is sent as the pair `thinking: adaptive`
/// plus the effort, never as the effort alone, and a baseline that sets
/// another thinking type conflicts at every level. On a direct backend the
/// same inputs write the effort alone and leave `thinking` untouched.
#[test]
fn copilot_messages_writes_the_adaptive_pair_or_refuses() {
    const STATED: &[EffortLevel] = &[Low, Medium, High, Xhigh, Max];
    let copilot = account(ANTHROPIC_COPILOT, ModelApi::Messages, STATED);
    let direct = observation(ANTHROPIC_API, ModelApi::Messages);
    let direct_row = ModelCapabilities {
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        ..row(Provider::Anthropic, STATED)
    };

    // No baseline thinking type, or `adaptive`: the pair.
    for baseline in [
        None,
        Some(ProviderParamsOverride::default()),
        anthropic(AnthropicProviderTag {
            top_k: Some(20),
            ..Default::default()
        }),
        thinking(AnthropicThinkingConfig::Adaptive),
    ] {
        for level in STATED.iter().copied() {
            for mode in [
                ReasoningResolveMode::Strict,
                ReasoningResolveMode::Preference,
            ] {
                let mut params = baseline.clone();
                let resolution = resolve(
                    mode,
                    level,
                    Some(&copilot),
                    Provider::Anthropic,
                    None,
                    &mut params,
                )
                .expect("a stated level resolves");
                assert_eq!(
                    resolution.fields,
                    owned(&["thinking.type", "output_config.effort"])
                );
                assert_eq!(resolution.facts, ReasoningFactsSource::Account);
                assert_eq!(
                    resolution.route,
                    ReasoningRoute::new(ANTHROPIC_COPILOT, ModelApi::Messages)
                );
                let tag = anthropic_tag_of(&params);
                assert_eq!(tag.thinking, Some(AnthropicThinkingConfig::Adaptive));
                assert_eq!(tag.effort, Some(typed_anthropic(level)));
                let top_k = match baseline
                    .as_ref()
                    .and_then(|params| params.provider_tag.as_ref())
                {
                    Some(ProviderTag::Anthropic(tag)) => tag.top_k,
                    _ => None,
                };
                assert_eq!(tag.top_k, top_k, "unrelated knobs stay");

                // The same inputs on a direct backend: the effort alone.
                let mut params = baseline.clone();
                let resolution = resolve(
                    mode,
                    level,
                    Some(&direct),
                    Provider::Anthropic,
                    Some(&direct_row),
                    &mut params,
                )
                .expect("a catalogued level resolves");
                assert_eq!(resolution.fields, owned(&["output_config.effort"]));
                let tag = anthropic_tag_of(&params);
                assert_eq!(tag.effort, Some(typed_anthropic(level)));
                let baseline_thinking = match baseline
                    .as_ref()
                    .and_then(|params| params.provider_tag.as_ref())
                {
                    Some(ProviderTag::Anthropic(tag)) => tag.thinking.clone(),
                    _ => None,
                };
                assert_eq!(tag.thinking, baseline_thinking, "thinking is untouched");
            }
        }
    }

    // Another thinking type: a conflict at every level, the lowest
    // included. The preference reports the thinking-mode conflict.
    for config in [
        AnthropicThinkingConfig::Disabled,
        AnthropicThinkingConfig::BetweenTools,
    ] {
        let baseline = thinking(config.clone());
        for level in [Low, High, Max] {
            let mut params = baseline.clone();
            let refusal = strict(level, &copilot, None, &mut params)
                .expect_err("the level is sent there only with adaptive");
            assert_eq!(
                refusal,
                ReasoningEffortRefusal {
                    requested: level,
                    provider: Provider::Anthropic,
                    model: MODEL.to_owned(),
                    route: Some(ReasoningRoute::new(ANTHROPIC_COPILOT, ModelApi::Messages,)),
                    reason: Reason::ConflictingThinkingType,
                    supported: Some(STATED.to_vec()),
                    facts: Some(ReasoningFactsSource::Account),
                    conflicting_param: Some("provider_params.provider_tag.thinking".to_owned()),
                    conflicting_level: None,
                },
                "{config:?} at {level:?}"
            );
            assert_eq!(params, baseline, "a refusal writes nothing");

            let mut params = baseline.clone();
            let refusal = preference(level, &copilot, None, &mut params)
                .expect_err("the preference never sends the effort alone");
            assert_eq!(refusal.reason, Reason::ConflictingThinkingType);
            assert_eq!(params, baseline);
        }

        // On a direct backend the same baseline takes the effort at `low`
        // and keeps its thinking type.
        let mut params = baseline.clone();
        strict(Low, &direct, Some(&direct_row), &mut params).expect("low is accepted");
        let tag = anthropic_tag_of(&params);
        assert_eq!(tag.effort, Some(AnthropicEffort::Low));
        assert_eq!(tag.thinking, Some(config));
    }

    // A budget stays a budget conflict on the Copilot route.
    let budget = thinking(AnthropicThinkingConfig::Enabled {
        budget_tokens: 2048,
    });
    for mode in [
        ReasoningResolveMode::Strict,
        ReasoningResolveMode::Preference,
    ] {
        let mut params = budget.clone();
        let refusal = resolve(
            mode,
            High,
            Some(&copilot),
            Provider::Anthropic,
            None,
            &mut params,
        )
        .expect_err("a budget and a level are two controls");
        assert_eq!(refusal.reason, Reason::ConflictingThinkingBudget);
        assert_eq!(params, budget);
    }
}

// ---------------------------------------------------------------------
// R1.4
// ---------------------------------------------------------------------

fn conflict(
    reason: Reason,
    param: &str,
    level: Option<EffortLevel>,
) -> (Reason, Option<String>, Option<EffortLevel>) {
    (reason, Some(param.to_owned()), level)
}

fn conflict_of(refusal: &ReasoningEffortRefusal) -> (Reason, Option<String>, Option<EffortLevel>) {
    (
        refusal.reason,
        refusal.conflicting_param.clone(),
        refusal.conflicting_level,
    )
}

/// One case per row of the conflict table, both outcomes: what a strict
/// request accepts beside the level, and what it refuses with the typed
/// reason, the param and (for a level) its value. A conflict reports the
/// route's supported set, and writes nothing.
#[test]
fn strict_conflicts_match_section_8() {
    const LEVELS: &[EffortLevel] = &[EffortLevel::None, Low, Medium, High, Xhigh, Max];
    let openai_route = observation(OPENAI_API, ModelApi::Responses);
    let openai_row = row(Provider::OpenAI, LEVELS);
    let anthropic_route = observation(ANTHROPIC_API, ModelApi::Messages);
    let anthropic_row = row(Provider::Anthropic, LEVELS);
    let gemini_route = observation(GOOGLE_GENAI, ModelApi::GenerateContent);
    let gemini_row = row(Provider::Gemini, &LADDER);

    let refused = |requested: EffortLevel,
                   observation: &ReasoningRouteObservation,
                   catalog_row: &ModelCapabilities,
                   baseline: Option<ProviderParamsOverride>| {
        let mut params = baseline.clone();
        let refusal = strict(requested, observation, Some(catalog_row), &mut params)
            .expect_err("a conflict is refused");
        assert_eq!(params, baseline, "a conflict writes nothing");
        let stated = supported_reasoning_levels(
            family(observation.backend),
            Some(observation),
            Some(catalog_row),
        );
        assert_eq!(
            refusal.supported.as_deref(),
            stated.supported(),
            "a conflict reports the route's supported set"
        );
        assert_eq!(refusal.facts, Some(ReasoningFactsSource::Catalog));
        conflict_of(&refusal)
    };
    let accepted = |requested: EffortLevel,
                    observation: &ReasoningRouteObservation,
                    catalog_row: &ModelCapabilities,
                    baseline: Option<ProviderParamsOverride>| {
        let mut params = baseline;
        strict(requested, observation, Some(catalog_row), &mut params)
            .expect("accepted beside the level");
        params
    };

    // Row 1: a provider effort equal to the level is accepted.
    let params = accepted(
        High,
        &openai_route,
        &openai_row,
        openai(OpenAiProviderTag {
            reasoning_effort: Some(ReasoningEffort::High),
            ..Default::default()
        }),
    );
    assert_eq!(
        openai_tag_of(&params).reasoning_effort,
        Some(ReasoningEffort::High)
    );
    let params = accepted(
        High,
        &anthropic_route,
        &anthropic_row,
        anthropic(AnthropicProviderTag {
            effort: Some(AnthropicEffort::High),
            ..Default::default()
        }),
    );
    assert_eq!(
        anthropic_tag_of(&params).effort,
        Some(AnthropicEffort::High)
    );
    let params = accepted(
        High,
        &gemini_route,
        &gemini_row,
        gemini(GeminiProviderTag {
            thinking_level: Some(GeminiThinkingLevel::High),
            ..Default::default()
        }),
    );
    assert_eq!(
        gemini_tag_of(&params).thinking_level,
        Some(GeminiThinkingLevel::High)
    );
    let params = accepted(
        High,
        &gemini_route,
        &gemini_row,
        gemini(GeminiProviderTag {
            thinking: Some(GeminiThinkingConfig {
                include_thoughts: Some(true),
                thinking_level: Some(GeminiThinkingLevel::High),
                thinking_budget: None,
            }),
            ..Default::default()
        }),
    );
    let nested = gemini_tag_of(&params).thinking.clone().expect("nested");
    assert_eq!(nested.thinking_level, Some(GeminiThinkingLevel::High));
    assert_eq!(nested.include_thoughts, Some(true));

    // Row 2: a provider effort that differs names its param and its level.
    assert_eq!(
        refused(
            High,
            &openai_route,
            &openai_row,
            openai(OpenAiProviderTag {
                reasoning_effort: Some(ReasoningEffort::Low),
                ..Default::default()
            }),
        ),
        conflict(
            Reason::ConflictingProviderEffort,
            "provider_params.provider_tag.reasoning_effort",
            Some(Low)
        )
    );
    assert_eq!(
        refused(
            High,
            &anthropic_route,
            &anthropic_row,
            anthropic(AnthropicProviderTag {
                effort: Some(AnthropicEffort::XHigh),
                ..Default::default()
            }),
        ),
        conflict(
            Reason::ConflictingProviderEffort,
            "provider_params.provider_tag.effort",
            Some(Xhigh)
        )
    );
    assert_eq!(
        refused(
            High,
            &gemini_route,
            &gemini_row,
            gemini(GeminiProviderTag {
                thinking_level: Some(GeminiThinkingLevel::Minimal),
                ..Default::default()
            }),
        ),
        conflict(
            Reason::ConflictingProviderEffort,
            "provider_params.provider_tag.thinking_level",
            Some(Minimal)
        )
    );
    assert_eq!(
        refused(
            High,
            &gemini_route,
            &gemini_row,
            gemini(GeminiProviderTag {
                thinking: Some(GeminiThinkingConfig {
                    include_thoughts: None,
                    thinking_level: Some(GeminiThinkingLevel::Low),
                    thinking_budget: None,
                }),
                ..Default::default()
            }),
        ),
        conflict(
            Reason::ConflictingProviderEffort,
            "provider_params.provider_tag.thinking.thinking_level",
            Some(Low)
        )
    );

    // Row 3: Gemini nested and flat levels that disagree conflict, whichever
    // of them equals the level; the one that differs is named.
    let disagreeing = gemini(GeminiProviderTag {
        thinking: Some(GeminiThinkingConfig {
            include_thoughts: None,
            thinking_level: Some(GeminiThinkingLevel::High),
            thinking_budget: None,
        }),
        thinking_level: Some(GeminiThinkingLevel::Low),
        ..Default::default()
    });
    assert_eq!(
        refused(High, &gemini_route, &gemini_row, disagreeing.clone()),
        conflict(
            Reason::ConflictingProviderEffort,
            "provider_params.provider_tag.thinking_level",
            Some(Low)
        )
    );
    assert_eq!(
        refused(Low, &gemini_route, &gemini_row, disagreeing.clone()),
        conflict(
            Reason::ConflictingProviderEffort,
            "provider_params.provider_tag.thinking.thinking_level",
            Some(High)
        )
    );
    assert_eq!(
        refused(Medium, &gemini_route, &gemini_row, disagreeing),
        conflict(
            Reason::ConflictingProviderEffort,
            "provider_params.provider_tag.thinking.thinking_level",
            Some(High)
        )
    );

    // Row 4: any thinking budget.
    let generic_budget = Some(ProviderParamsOverride {
        thinking_budget_tokens: Some(4096),
        ..Default::default()
    });
    for (observation, catalog_row) in [
        (&openai_route, &openai_row),
        (&anthropic_route, &anthropic_row),
        (&gemini_route, &gemini_row),
    ] {
        assert_eq!(
            refused(High, observation, catalog_row, generic_budget.clone()),
            conflict(
                Reason::ConflictingThinkingBudget,
                "provider_params.thinking_budget_tokens",
                None
            )
        );
    }
    assert_eq!(
        refused(
            High,
            &anthropic_route,
            &anthropic_row,
            thinking(AnthropicThinkingConfig::Enabled {
                budget_tokens: 2048
            }),
        ),
        conflict(
            Reason::ConflictingThinkingBudget,
            "provider_params.provider_tag.thinking",
            None
        )
    );
    assert_eq!(
        refused(
            High,
            &anthropic_route,
            &anthropic_row,
            anthropic(AnthropicProviderTag {
                thinking_budget_tokens: Some(2048),
                ..Default::default()
            }),
        ),
        conflict(
            Reason::ConflictingThinkingBudget,
            "provider_params.provider_tag.thinking_budget_tokens",
            None
        )
    );
    assert_eq!(
        refused(
            High,
            &gemini_route,
            &gemini_row,
            gemini(GeminiProviderTag {
                thinking_budget: Some(4096),
                ..Default::default()
            }),
        ),
        conflict(
            Reason::ConflictingThinkingBudget,
            "provider_params.provider_tag.thinking_budget",
            None
        )
    );
    assert_eq!(
        refused(
            High,
            &gemini_route,
            &gemini_row,
            gemini(GeminiProviderTag {
                thinking: Some(GeminiThinkingConfig {
                    include_thoughts: None,
                    thinking_level: None,
                    thinking_budget: Some(4096),
                }),
                ..Default::default()
            }),
        ),
        conflict(
            Reason::ConflictingThinkingBudget,
            "provider_params.provider_tag.thinking.thinking_budget",
            None
        )
    );

    // Row 5: `between_tools`, or `disabled` on a row that caps it, with
    // `xhigh` or `max`. Rows 6: `adaptive`, and `disabled` on a row that
    // accepts it at any effort, are accepted at every level.
    let capped_row = ModelCapabilities {
        anthropic_disabled_thinking: AnthropicDisabledThinking::AtOrBelowHighEffort,
        ..anthropic_row
    };
    let any_effort_row = ModelCapabilities {
        anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
        ..anthropic_row
    };
    for level in [Xhigh, Max] {
        for (catalog_row, config) in [
            (&anthropic_row, AnthropicThinkingConfig::BetweenTools),
            (&capped_row, AnthropicThinkingConfig::BetweenTools),
            (&capped_row, AnthropicThinkingConfig::Disabled),
        ] {
            assert_eq!(
                refused(level, &anthropic_route, catalog_row, thinking(config)),
                conflict(
                    Reason::ConflictingThinkingType,
                    "provider_params.provider_tag.thinking",
                    None
                )
            );
        }
        for (catalog_row, config) in [
            (&anthropic_row, AnthropicThinkingConfig::Adaptive),
            (&any_effort_row, AnthropicThinkingConfig::Disabled),
        ] {
            let params = accepted(
                level,
                &anthropic_route,
                catalog_row,
                thinking(config.clone()),
            );
            let tag = anthropic_tag_of(&params);
            assert_eq!(tag.effort, Some(typed_anthropic(level)));
            assert_eq!(tag.thinking, Some(config), "the thinking type is untouched");
        }
    }
    for config in [
        AnthropicThinkingConfig::BetweenTools,
        AnthropicThinkingConfig::Disabled,
    ] {
        let params = accepted(
            High,
            &anthropic_route,
            &capped_row,
            thinking(config.clone()),
        );
        assert_eq!(anthropic_tag_of(&params).thinking, Some(config));
    }

    // Rows 7 and 8 (the Copilot Messages pair) are
    // `copilot_messages_writes_the_adaptive_pair_or_refuses`.

    // Row 9: an opaque reasoning body.
    assert_eq!(
        refused(
            High,
            &openai_route,
            &openai_row,
            openai(OpenAiProviderTag {
                reasoning: Some(OpaqueProviderBody::from_value(
                    &serde_json::json!({"effort": "low"})
                )),
                ..Default::default()
            }),
        ),
        conflict(
            Reason::ConflictingReasoningBody,
            "provider_params.provider_tag.reasoning",
            None
        )
    );

    // Row 10: the orthogonal OpenAI reasoning knobs are accepted and kept.
    let orthogonal = OpenAiProviderTag {
        reasoning_mode: Some(OpenAiReasoningMode::Pro),
        reasoning_context: Some(OpenAiReasoningContext::AllTurns),
        text_verbosity: Some(OpenAiTextVerbosity::Low),
        ..Default::default()
    };
    let params = accepted(High, &openai_route, &openai_row, openai(orthogonal.clone()));
    assert_eq!(
        openai_tag_of(&params),
        &OpenAiProviderTag {
            reasoning_effort: Some(ReasoningEffort::High),
            reasoning_effort_resolved: true,
            ..orthogonal
        }
    );

    // Row 11: generic reasoning off with a level other than `none`.
    let off = Some(ProviderParamsOverride {
        reasoning: Some(ReasoningMode::Off),
        ..Default::default()
    });
    for (observation, catalog_row) in [
        (&openai_route, &openai_row),
        (&anthropic_route, &anthropic_row),
        (&gemini_route, &gemini_row),
    ] {
        assert_eq!(
            refused(Low, observation, catalog_row, off.clone()),
            conflict(
                Reason::ConflictingReasoningMode,
                "provider_params.reasoning",
                None
            )
        );
    }

    // Row 12: off with `none` is accepted where the route accepts `none`,
    // and is an unsupported level where it does not.
    let params = accepted(EffortLevel::None, &openai_route, &openai_row, off.clone());
    assert_eq!(
        openai_tag_of(&params).reasoning_effort,
        Some(ReasoningEffort::None)
    );
    assert_eq!(
        params.as_ref().and_then(|params| params.reasoning),
        Some(ReasoningMode::Off)
    );
    let mut params = off.clone();
    let refusal = strict(
        EffortLevel::None,
        &anthropic_route,
        Some(&anthropic_row),
        &mut params,
    )
    .expect_err("messages cannot express none");
    assert_eq!(refusal.reason, Reason::UnsupportedLevel);
    assert_eq!(params, off);

    // Row 13: emit and silent are accepted.
    for mode in [ReasoningMode::Emit, ReasoningMode::Silent] {
        let params = accepted(
            High,
            &openai_route,
            &openai_row,
            Some(ProviderParamsOverride {
                reasoning: Some(mode),
                ..Default::default()
            }),
        );
        assert_eq!(
            params.as_ref().and_then(|params| params.reasoning),
            Some(mode)
        );
    }

    // Row 14: the internal reasoning override is not caller vocabulary: the
    // translation neither reads nor writes it.
    for value in [None, Some(false), Some(true)] {
        let params = accepted(
            High,
            &openai_route,
            &openai_row,
            openai(OpenAiProviderTag {
                supports_reasoning_override: value,
                ..Default::default()
            }),
        );
        assert_eq!(openai_tag_of(&params).supports_reasoning_override, value);
    }
}

/// A conflict comes before the facts: it is reported on a route whose
/// levels are unknown or unavailable too, with `supported: null`.
#[test]
fn a_strict_conflict_is_reported_before_the_facts() {
    let unavailable = ReasoningRouteObservation {
        account: AccountLevelStatement::ListUnavailable,
        ..observation(OPENAI_COPILOT, ModelApi::Responses)
    };
    let baseline = openai(OpenAiProviderTag {
        reasoning_effort: Some(ReasoningEffort::Low),
        ..Default::default()
    });
    let mut params = baseline.clone();
    let refusal = strict(High, &unavailable, None, &mut params).expect_err("a conflict");
    assert_eq!(
        conflict_of(&refusal),
        conflict(
            Reason::ConflictingProviderEffort,
            "provider_params.provider_tag.reasoning_effort",
            Some(Low)
        )
    );
    assert_eq!((refusal.supported, refusal.facts), (None, None));
    assert_eq!(params, baseline);

    // A conflict also comes before an unsupported level.
    let stated = account(OPENAI_COPILOT, ModelApi::Responses, &[Low, Medium]);
    let mut params = baseline.clone();
    let refusal = strict(High, &stated, None, &mut params).expect_err("a conflict");
    assert_eq!(refusal.reason, Reason::ConflictingProviderEffort);
    assert_eq!(refusal.supported, Some(vec![Low, Medium]));
    assert_eq!(refusal.facts, Some(ReasoningFactsSource::Account));
    assert_eq!(params, baseline);
}

/// The preference keeps the outcomes of the lowering it replaces where a
/// strict request conflicts: a provider effort is replaced, a generic
/// budget is not read on an OpenAI route, and reasoning off refuses every
/// level.
#[test]
fn the_preference_replaces_a_provider_effort() {
    const LEVELS: &[EffortLevel] = &[EffortLevel::None, Low, Medium, High];
    let openai_route = observation(OPENAI_API, ModelApi::Responses);
    let openai_row = row(Provider::OpenAI, LEVELS);

    let mut params = openai(OpenAiProviderTag {
        reasoning_effort: Some(ReasoningEffort::High),
        ..Default::default()
    });
    let resolution =
        preference(Low, &openai_route, Some(&openai_row), &mut params).expect("replaced");
    assert_eq!(resolution.decided_by, ReasoningEffortDecidedBy::Preference);
    assert_eq!(
        openai_tag_of(&params).reasoning_effort,
        Some(ReasoningEffort::Low)
    );

    let mut params = Some(ProviderParamsOverride {
        thinking_budget_tokens: Some(4096),
        ..Default::default()
    });
    preference(Low, &openai_route, Some(&openai_row), &mut params)
        .expect("the OpenAI lowering never read the generic budget");

    let off = Some(ProviderParamsOverride {
        reasoning: Some(ReasoningMode::Off),
        ..Default::default()
    });
    let mut params = off.clone();
    let refusal = preference(
        EffortLevel::None,
        &openai_route,
        Some(&openai_row),
        &mut params,
    )
    .expect_err("the baseline turns reasoning off");
    assert_eq!(refusal.reason, Reason::ConflictingReasoningMode);
    assert_eq!(params, off);
}

/// A request copy that carries another provider family's tag, or an
/// untyped tag bag, has no place for the route's typed field: the level is
/// refused and the copy is untouched.
#[test]
fn a_tag_of_another_family_is_refused() {
    let openai_route = observation(OPENAI_API, ModelApi::Responses);
    let openai_row = row(Provider::OpenAI, &LADDER);
    let anthropic_route = observation(ANTHROPIC_API, ModelApi::Messages);
    let anthropic_row = row(Provider::Anthropic, &LADDER);
    let gemini_route = observation(GOOGLE_GENAI, ModelApi::GenerateContent);
    let gemini_row = row(Provider::Gemini, &LADDER);
    let bag = tagged(ProviderTag::Unknown {
        bag: StructuredProviderExtension {
            namespace: "legacy".to_owned(),
            key: "thinking".to_owned(),
            body: "{}".to_owned(),
        },
    });
    let cases = [
        (&openai_route, &openai_row, anthropic(Default::default())),
        (&openai_route, &openai_row, gemini(Default::default())),
        (&openai_route, &openai_row, bag.clone()),
        (&anthropic_route, &anthropic_row, openai(Default::default())),
        (&anthropic_route, &anthropic_row, gemini(Default::default())),
        (&anthropic_route, &anthropic_row, bag.clone()),
        (&gemini_route, &gemini_row, openai(Default::default())),
        (&gemini_route, &gemini_row, anthropic(Default::default())),
        (&gemini_route, &gemini_row, bag),
    ];
    for (observation, catalog_row, baseline) in cases {
        for mode in [
            ReasoningResolveMode::Strict,
            ReasoningResolveMode::Preference,
        ] {
            let mut params = baseline.clone();
            let refusal = resolve(
                mode,
                High,
                Some(observation),
                family(observation.backend),
                Some(catalog_row),
                &mut params,
            )
            .expect_err("no place for the route's typed field");
            assert_eq!(
                conflict_of(&refusal),
                conflict(
                    Reason::ConflictingReasoningBody,
                    "provider_params.provider_tag",
                    None
                )
            );
            assert_eq!(params, baseline);
        }
    }
}

/// The OpenAI tag's opaque `thinking` and `chat_template_kwargs` bodies are
/// forwarded beside a level in both modes and kept as they are: only the
/// opaque `reasoning` body conflicts.
#[test]
fn opaque_thinking_and_chat_template_bodies_are_accepted_beside_a_level() {
    let body = || {
        Some(OpaqueProviderBody::from_value(
            &serde_json::json!({"type": "enabled"}),
        ))
    };
    let catalog_row = row(Provider::OpenAI, &[Low, Medium, High]);
    let routes = [
        (
            observation(OPENAI_API, ModelApi::Responses),
            Some(&catalog_row),
        ),
        (
            observation(OPENAI_API, ModelApi::ChatCompletions),
            Some(&catalog_row),
        ),
        (
            operator(
                ReasoningBackend::SelfHosted(SelfHostedBackendKind::OpenAiCompatible),
                ModelApi::ChatCompletions,
                &[Low, Medium, High],
            ),
            None,
        ),
    ];
    for (observation, catalog_row) in &routes {
        for mode in [
            ReasoningResolveMode::Strict,
            ReasoningResolveMode::Preference,
        ] {
            for tag in [
                OpenAiProviderTag {
                    thinking: body(),
                    ..Default::default()
                },
                OpenAiProviderTag {
                    chat_template_kwargs: body(),
                    ..Default::default()
                },
                OpenAiProviderTag {
                    thinking: body(),
                    chat_template_kwargs: body(),
                    ..Default::default()
                },
            ] {
                let mut params = openai(tag.clone());
                resolve(
                    mode,
                    High,
                    Some(observation),
                    family(observation.backend),
                    *catalog_row,
                    &mut params,
                )
                .expect("accepted beside the level");
                assert_eq!(
                    openai_tag_of(&params),
                    &OpenAiProviderTag {
                        reasoning_effort: Some(ReasoningEffort::High),
                        reasoning_effort_resolved: true,
                        ..tag
                    },
                    "the bodies are kept as they are"
                );
            }

            // The opaque `reasoning` body is the one that conflicts, with or
            // without the other two beside it.
            let baseline = openai(OpenAiProviderTag {
                reasoning: body(),
                thinking: body(),
                chat_template_kwargs: body(),
                ..Default::default()
            });
            let mut params = baseline.clone();
            let refusal = resolve(
                mode,
                High,
                Some(observation),
                family(observation.backend),
                *catalog_row,
                &mut params,
            )
            .expect_err("a raw reasoning body may carry its own effort");
            assert_eq!(
                conflict_of(&refusal),
                conflict(
                    Reason::ConflictingReasoningBody,
                    "provider_params.provider_tag.reasoning",
                    None
                )
            );
            assert_eq!(params, baseline);
        }
    }
}

/// The APIs a backend's family does not speak, written out by hand.
fn unspoken_apis(backend: ReasoningBackend) -> &'static [ModelApi] {
    match backend {
        ReasoningBackend::OpenAi(_) | ReasoningBackend::SelfHosted(_) => {
            &[ModelApi::Messages, ModelApi::GenerateContent]
        }
        ReasoningBackend::Anthropic(_) => &[
            ModelApi::Responses,
            ModelApi::ChatCompletions,
            ModelApi::GenerateContent,
        ],
        ReasoningBackend::Google(_) => &[ModelApi::Responses, ModelApi::Messages],
    }
}

/// An API the backend's family does not speak takes no effort, like the
/// realtime API: `supported: []` from the kind's own source, whatever the
/// sources state, and every level is refused as unsupported in both modes.
#[test]
fn an_api_the_family_does_not_speak_takes_no_effort() {
    let mut routes = 0;
    for backend in backends() {
        // The kind's source, written out by hand.
        let source = match backend {
            ReasoningBackend::SelfHosted(_) => ReasoningFactsSource::Operator,
            _ if backend.is_copilot() => ReasoningFactsSource::Account,
            _ => ReasoningFactsSource::Catalog,
        };
        for api in unspoken_apis(backend).iter().copied() {
            routes += 1;
            let context = format!("{}/{api}", backend.as_str());
            let (observation, catalog_row) = stating_everywhere(backend, api, &LADDER);
            assert_eq!(
                supported_reasoning_levels(
                    family(backend),
                    Some(&observation),
                    catalog_row.as_ref()
                ),
                ReasoningLevelFacts::Stated {
                    levels: Vec::new(),
                    source,
                },
                "{context}"
            );
            for level in LADDER {
                for mode in [
                    ReasoningResolveMode::Strict,
                    ReasoningResolveMode::Preference,
                ] {
                    let mut params = None;
                    let refusal = resolve(
                        mode,
                        level,
                        Some(&observation),
                        family(backend),
                        catalog_row.as_ref(),
                        &mut params,
                    )
                    .expect_err("the route takes no effort");
                    assert_eq!(refusal.reason, Reason::UnsupportedLevel, "{context}");
                    assert_eq!(refusal.supported, Some(Vec::new()), "{context}");
                    assert_eq!(refusal.facts, Some(source), "{context}");
                    assert_eq!(
                        refusal.route,
                        Some(ReasoningRoute::new(backend, api)),
                        "{context}"
                    );
                    assert!(
                        refusal
                            .to_string()
                            .ends_with("; this route accepts no reasoning effort"),
                        "{context}: {refusal}"
                    );
                    assert_eq!(params, None, "{context}");
                }
            }
        }
    }
    // OpenAI and self-hosted kinds: 6 kinds, 2 APIs. Anthropic: 5 kinds, 3
    // APIs. Gemini: 4 kinds, 2 APIs.
    assert_eq!(routes, 6 * 2 + 5 * 3 + 4 * 2);
}

// ---------------------------------------------------------------------
// The whole decision table
// ---------------------------------------------------------------------

/// Baseline request copies that exercise every conflict input of every
/// family, alone.
fn baselines() -> Vec<Option<ProviderParamsOverride>> {
    let generic = |reasoning: Option<ReasoningMode>, budget: Option<u32>| {
        Some(ProviderParamsOverride {
            reasoning,
            thinking_budget_tokens: budget,
            ..Default::default()
        })
    };
    let nested = |level: Option<GeminiThinkingLevel>, budget: Option<u32>| GeminiThinkingConfig {
        include_thoughts: Some(true),
        thinking_level: level,
        thinking_budget: budget,
    };
    vec![
        None,
        Some(ProviderParamsOverride::default()),
        Some(ProviderParamsOverride {
            temperature: Some(0.2),
            ..Default::default()
        }),
        generic(Some(ReasoningMode::Off), None),
        generic(Some(ReasoningMode::Emit), None),
        generic(Some(ReasoningMode::Silent), None),
        generic(None, Some(4096)),
        generic(Some(ReasoningMode::Off), Some(4096)),
        openai(Default::default()),
        openai(OpenAiProviderTag {
            reasoning_effort: Some(ReasoningEffort::None),
            ..Default::default()
        }),
        openai(OpenAiProviderTag {
            reasoning_effort: Some(ReasoningEffort::Low),
            ..Default::default()
        }),
        openai(OpenAiProviderTag {
            reasoning_effort: Some(ReasoningEffort::Max),
            reasoning_mode: Some(OpenAiReasoningMode::Pro),
            ..Default::default()
        }),
        openai(OpenAiProviderTag {
            reasoning: Some(OpaqueProviderBody::from_value(
                &serde_json::json!({"effort": "low"}),
            )),
            ..Default::default()
        }),
        openai(OpenAiProviderTag {
            reasoning_effort: Some(ReasoningEffort::High),
            reasoning: Some(OpaqueProviderBody::from_value(&serde_json::json!({}))),
            ..Default::default()
        }),
        openai(OpenAiProviderTag {
            thinking: Some(OpaqueProviderBody::from_value(&serde_json::json!({}))),
            supports_reasoning_override: Some(true),
            ..Default::default()
        }),
        anthropic(Default::default()),
        anthropic(AnthropicProviderTag {
            effort: Some(AnthropicEffort::Low),
            ..Default::default()
        }),
        anthropic(AnthropicProviderTag {
            effort: Some(AnthropicEffort::Max),
            thinking: Some(AnthropicThinkingConfig::Adaptive),
            ..Default::default()
        }),
        thinking(AnthropicThinkingConfig::Adaptive),
        thinking(AnthropicThinkingConfig::BetweenTools),
        thinking(AnthropicThinkingConfig::Disabled),
        thinking(AnthropicThinkingConfig::Enabled {
            budget_tokens: 2048,
        }),
        anthropic(AnthropicProviderTag {
            thinking_budget_tokens: Some(2048),
            ..Default::default()
        }),
        anthropic(AnthropicProviderTag {
            thinking: Some(AnthropicThinkingConfig::BetweenTools),
            effort: Some(AnthropicEffort::High),
            ..Default::default()
        }),
        gemini(Default::default()),
        gemini(GeminiProviderTag {
            thinking_level: Some(GeminiThinkingLevel::Low),
            ..Default::default()
        }),
        gemini(GeminiProviderTag {
            thinking_level: Some(GeminiThinkingLevel::Minimal),
            top_k: Some(20),
            ..Default::default()
        }),
        gemini(GeminiProviderTag {
            thinking: Some(nested(Some(GeminiThinkingLevel::High), None)),
            ..Default::default()
        }),
        gemini(GeminiProviderTag {
            thinking: Some(nested(None, None)),
            ..Default::default()
        }),
        gemini(GeminiProviderTag {
            thinking: Some(nested(Some(GeminiThinkingLevel::High), None)),
            thinking_level: Some(GeminiThinkingLevel::Low),
            ..Default::default()
        }),
        gemini(GeminiProviderTag {
            thinking: Some(nested(None, None)),
            thinking_level: Some(GeminiThinkingLevel::Medium),
            ..Default::default()
        }),
        gemini(GeminiProviderTag {
            thinking_budget: Some(4096),
            ..Default::default()
        }),
        gemini(GeminiProviderTag {
            thinking: Some(nested(None, Some(4096))),
            ..Default::default()
        }),
        tagged(ProviderTag::Unknown {
            bag: StructuredProviderExtension {
                namespace: "legacy".to_owned(),
                key: "thinking".to_owned(),
                body: "{}".to_owned(),
            },
        }),
    ]
}

/// The statements a route can meet: nothing stated anywhere, every level
/// stated everywhere, a partial set stated everywhere, an account list that
/// is not loaded, and an entry that states none.
fn statements(
    backend: ReasoningBackend,
    api: ModelApi,
) -> Vec<(ReasoningRouteObservation, Option<ModelCapabilities>)> {
    let capped = |levels: &'static [EffortLevel]| {
        family_row(backend, levels).map(|row| ModelCapabilities {
            anthropic_disabled_thinking: AnthropicDisabledThinking::AtOrBelowHighEffort,
            ..row
        })
    };
    vec![
        (observation(backend, api), None),
        stating_everywhere(backend, api, &LADDER),
        (stating_everywhere(backend, api, &LADDER).0, capped(&LADDER)),
        stating_everywhere(backend, api, &[Low, High]),
        stating_everywhere(backend, api, &[]),
        (
            ReasoningRouteObservation {
                account: AccountLevelStatement::ListUnavailable,
                ..observation(backend, api)
            },
            family_row(backend, &LADDER),
        ),
        (
            ReasoningRouteObservation {
                account: AccountLevelStatement::NotStated,
                ..observation(backend, api)
            },
            family_row(backend, &[]),
        ),
    ]
}

fn is_conflict(reason: Reason) -> bool {
    match reason {
        Reason::UnsupportedLevel
        | Reason::SupportedLevelsUnknown
        | Reason::SupportedLevelsUnavailable => false,
        Reason::ConflictingProviderEffort
        | Reason::ConflictingThinkingBudget
        | Reason::ConflictingThinkingType
        | Reason::ConflictingReasoningBody
        | Reason::ConflictingReasoningMode => true,
    }
}

/// One case of the decision table.
struct Case<'a> {
    mode: ReasoningResolveMode,
    requested: EffortLevel,
    provider: Provider,
    observation: Option<&'a ReasoningRouteObservation>,
    catalog_row: Option<&'a ModelCapabilities>,
    baseline: &'a Option<ProviderParamsOverride>,
}

/// The contract of every outcome, checked on one case of the table.
fn check_outcome(context: &str, case: &Case<'_>, seen: &mut std::collections::BTreeSet<String>) {
    let Case {
        mode,
        requested,
        provider,
        observation,
        catalog_row,
        baseline,
    } = *case;
    let mut params = baseline.clone();
    let outcome = resolve(
        mode,
        requested,
        observation,
        provider,
        catalog_row,
        &mut params,
    );
    let facts = supported_reasoning_levels(provider, observation, catalog_row);
    match outcome {
        Ok(resolution) => {
            seen.insert("resolved".to_owned());
            // The level sent is the level requested: resolution never picks
            // another one.
            assert_eq!(resolution.requested, requested, "{context}");
            assert_eq!(resolution.level, requested, "{context}");
            assert_eq!(resolution.provider, provider, "{context}");
            assert_eq!(resolution.model, MODEL, "{context}");
            let observation = observation.expect("a resolution needs an observed route");
            assert_eq!(resolution.route, observation.route(), "{context}");
            assert!(!resolution.fields.is_empty(), "{context}");
            // A resolution always has facts, and the level is among them.
            assert_eq!(Some(resolution.facts), facts.source(), "{context}");
            assert!(
                facts
                    .supported()
                    .is_some_and(|levels| levels.contains(&requested)),
                "{context}: resolved a level the facts do not state"
            );
            assert_eq!(
                resolution.facts,
                observation.backend.levels_source(),
                "{context}"
            );
            assert_eq!(
                explicit_provider_effort(params.as_ref()),
                Some(requested),
                "{context}: the request copy carries the level"
            );
        }
        Err(refusal) => {
            seen.insert(
                serde_json::to_value(refusal.reason)
                    .expect("a reason serializes")
                    .as_str()
                    .expect("a string")
                    .to_owned(),
            );
            assert_eq!(&params, baseline, "{context}: a refusal writes nothing");
            assert_eq!(refusal.requested, requested, "{context}");
            assert_eq!(refusal.provider, provider, "{context}");
            assert_eq!(refusal.model, MODEL, "{context}");
            match observation {
                Some(observation) => {
                    assert_eq!(refusal.route, Some(observation.route()), "{context}");
                }
                // Without an observation the refusal names the family's own
                // route; a provider outside the families has none to name.
                None => assert_eq!(
                    refusal.route.is_none(),
                    provider == Provider::Other,
                    "{context}"
                ),
            }
            // `facts` is absent exactly when `supported` is.
            assert_eq!(
                refusal.facts.is_none(),
                refusal.supported.is_none(),
                "{context}"
            );
            // The two combinations the refusal text has no documented form
            // for are never built: a conflict always names its param, and an
            // unsupported level always lists the supported set.
            assert_eq!(
                refusal.conflicting_param.is_some(),
                is_conflict(refusal.reason),
                "{context}: {refusal:?}"
            );
            let text = refusal.to_string();
            assert!(
                !text.contains("another reasoning setting"),
                "{context}: {text}"
            );
            match refusal.reason {
                Reason::UnsupportedLevel => {
                    let supported = refusal
                        .supported
                        .as_deref()
                        .unwrap_or_else(|| panic!("{context}: unsupported without a set"));
                    assert!(!supported.contains(&requested), "{context}");
                    assert_eq!(Some(supported), facts.supported(), "{context}");
                    assert!(
                        text.contains("; supported: ")
                            || text.contains("; this route accepts no reasoning effort"),
                        "{context}: {text}"
                    );
                }
                Reason::SupportedLevelsUnknown => {
                    assert_eq!(refusal.supported, None, "{context}");
                    assert_eq!(facts, ReasoningLevelFacts::Unknown, "{context}");
                }
                Reason::SupportedLevelsUnavailable => {
                    assert_eq!(refusal.supported, None, "{context}");
                    assert_eq!(facts, ReasoningLevelFacts::Unavailable, "{context}");
                }
                Reason::ConflictingProviderEffort
                | Reason::ConflictingThinkingBudget
                | Reason::ConflictingThinkingType
                | Reason::ConflictingReasoningBody
                | Reason::ConflictingReasoningMode => {
                    // A conflict reports the route's set when it is known.
                    assert_eq!(refusal.supported.as_deref(), facts.supported(), "{context}");
                    assert_eq!(refusal.facts, facts.source(), "{context}");
                }
            }
            // Only a conflicting provider effort is itself a level.
            assert_eq!(
                refusal.conflicting_level.is_some(),
                refusal.reason == Reason::ConflictingProviderEffort,
                "{context}"
            );
            if let Some(level) = refusal.conflicting_level {
                assert_ne!(
                    level, requested,
                    "{context}: an equal effort is no conflict"
                );
            }
            // The preference replaces a provider effort; it never refuses
            // on one.
            if mode == ReasoningResolveMode::Preference {
                assert_ne!(
                    refusal.reason,
                    Reason::ConflictingProviderEffort,
                    "{context}"
                );
            }
        }
    }
}

/// Every backend kind, API, level, statement, baseline and mode: each
/// outcome keeps the contract of `check_outcome`. In particular no refusal
/// is ever built in a form its message has no documented text for.
#[test]
fn every_outcome_of_the_decision_table_is_well_formed() {
    let baselines = baselines();
    let mut seen = std::collections::BTreeSet::new();
    let mut cases = 0_u32;
    for backend in backends() {
        for api in ALL_APIS {
            for (observation, catalog_row) in statements(backend, api) {
                for baseline in &baselines {
                    for requested in LADDER {
                        for mode in [
                            ReasoningResolveMode::Strict,
                            ReasoningResolveMode::Preference,
                        ] {
                            cases += 1;
                            let context = format!(
                                "{mode:?} {requested:?} on {}/{api} {:?} row {:?} with {baseline:?}",
                                backend.as_str(),
                                observation.account,
                                catalog_row.as_ref().map(|row| row.effort_levels),
                            );
                            check_outcome(
                                &context,
                                &Case {
                                    mode,
                                    requested,
                                    provider: family(backend),
                                    observation: Some(&observation),
                                    catalog_row: catalog_row.as_ref(),
                                    baseline,
                                },
                                &mut seen,
                            );
                        }
                    }
                }
            }
        }
    }
    // No observation, for every provider family.
    for provider in [
        Provider::OpenAI,
        Provider::Anthropic,
        Provider::Gemini,
        Provider::SelfHosted,
        Provider::Other,
    ] {
        let catalog_row = match provider {
            Provider::SelfHosted | Provider::Other => None,
            _ => Some(row(provider, &LADDER)),
        };
        for baseline in &baselines {
            for requested in LADDER {
                for mode in [
                    ReasoningResolveMode::Strict,
                    ReasoningResolveMode::Preference,
                ] {
                    cases += 1;
                    let context = format!(
                        "{mode:?} {requested:?} for {provider:?}, no observation, with {baseline:?}"
                    );
                    check_outcome(
                        &context,
                        &Case {
                            mode,
                            requested,
                            provider,
                            observation: None,
                            catalog_row: catalog_row.as_ref(),
                            baseline,
                        },
                        &mut seen,
                    );
                }
            }
        }
    }
    assert_eq!(baselines.len(), 34);
    assert_eq!(
        cases,
        (15 * 5 * 7 + 5) * 34 * 7 * 2,
        "the whole table ran: routes and statements, baselines, levels, modes"
    );
    // The table reaches a resolution and every one of the eight reasons.
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        [
            "conflicting_provider_effort",
            "conflicting_reasoning_body",
            "conflicting_reasoning_mode",
            "conflicting_thinking_budget",
            "conflicting_thinking_type",
            "resolved",
            "supported_levels_unavailable",
            "supported_levels_unknown",
            "unsupported_level",
        ]
    );
}

// ---------------------------------------------------------------------
// The preference wrapper against the lowering it replaces
// ---------------------------------------------------------------------

/// The per-provider preference lowering as it was before the resolver
/// existed, kept here verbatim as the oracle for the wrapper.
mod previous_lowering {
    use super::*;

    pub(super) fn lower(
        capabilities: Option<&ModelCapabilities>,
        provider: Provider,
        level: EffortLevel,
        params: &mut Option<ProviderParamsOverride>,
    ) -> (ReasoningLoweringBaseline, ReasoningLoweringOutcome) {
        let baseline = baseline_effort(params.as_ref());
        let outcome = match apply(capabilities, provider, level, params) {
            Ok(()) => ReasoningLoweringOutcome::Applied(level),
            Err(reason) => ReasoningLoweringOutcome::NotApplied(reason),
        };
        (baseline, outcome)
    }

    fn apply(
        capabilities: Option<&ModelCapabilities>,
        provider: Provider,
        level: EffortLevel,
        params: &mut Option<ProviderParamsOverride>,
    ) -> Result<(), ReasoningNotAppliedReason> {
        let Some(capabilities) = capabilities.filter(|caps| caps.provider == provider) else {
            return Err(ReasoningNotAppliedReason::NoCatalogFact);
        };
        if params
            .as_ref()
            .is_some_and(|params| params.reasoning == Some(ReasoningMode::Off))
        {
            return Err(ReasoningNotAppliedReason::ReasoningDisabledByBaseline);
        }
        match provider {
            Provider::OpenAI => {
                if !(capabilities.supports_reasoning && capabilities.effort_levels.contains(&level))
                {
                    return Err(ReasoningNotAppliedReason::UnsupportedLevel);
                }
                let effort =
                    openai_effort(level).ok_or(ReasoningNotAppliedReason::UnsupportedLevel)?;
                let tag = openai_tag(params).ok_or(ReasoningNotAppliedReason::NoCatalogFact)?;
                if tag.reasoning.is_some() {
                    return Err(ReasoningNotAppliedReason::OpaqueReasoningBody);
                }
                tag.reasoning_effort = Some(effort);
                Ok(())
            }
            Provider::Anthropic => {
                if !capabilities.effort_levels.contains(&level) {
                    return Err(ReasoningNotAppliedReason::UnsupportedLevel);
                }
                let effort =
                    anthropic_effort(level).ok_or(ReasoningNotAppliedReason::UnsupportedLevel)?;
                if params
                    .as_ref()
                    .is_some_and(|params| params.thinking_budget_tokens.is_some())
                {
                    return Err(ReasoningNotAppliedReason::BudgetConflict);
                }
                if let Some(ProviderTag::Anthropic(tag)) = params
                    .as_ref()
                    .and_then(|params| params.provider_tag.as_ref())
                {
                    if tag.thinking_budget_tokens.is_some()
                        || matches!(tag.thinking, Some(AnthropicThinkingConfig::Enabled { .. }))
                    {
                        return Err(ReasoningNotAppliedReason::BudgetConflict);
                    }
                    if let Some(thinking) = tag.thinking.as_ref()
                        && capabilities.anthropic_thinking_capped_at_high_effort(thinking)
                        && level > EffortLevel::High
                    {
                        return Err(ReasoningNotAppliedReason::ThinkingModeConflict);
                    }
                }
                let tag = anthropic_tag(params).ok_or(ReasoningNotAppliedReason::NoCatalogFact)?;
                tag.effort = Some(effort);
                Ok(())
            }
            Provider::Gemini => {
                match capabilities.thinking {
                    ThinkingSupport::GeminiThinkingLevel
                        if capabilities.effort_levels.is_empty() =>
                    {
                        return Err(ReasoningNotAppliedReason::UnknownSupportedLevels);
                    }
                    ThinkingSupport::GeminiThinkingLevel => {}
                    _ => return Err(ReasoningNotAppliedReason::UnsupportedLevel),
                }
                if !capabilities.effort_levels.contains(&level) {
                    return Err(ReasoningNotAppliedReason::UnsupportedLevel);
                }
                let thinking_level = gemini_thinking_level(level)
                    .ok_or(ReasoningNotAppliedReason::UnsupportedLevel)?;
                if params
                    .as_ref()
                    .is_some_and(|params| params.thinking_budget_tokens.is_some())
                {
                    return Err(ReasoningNotAppliedReason::BudgetConflict);
                }
                if let Some(ProviderTag::Gemini(tag)) = params
                    .as_ref()
                    .and_then(|params| params.provider_tag.as_ref())
                    && (tag.thinking_budget.is_some()
                        || tag
                            .thinking
                            .as_ref()
                            .is_some_and(|thinking| thinking.thinking_budget.is_some()))
                {
                    return Err(ReasoningNotAppliedReason::BudgetConflict);
                }
                let tag = gemini_tag(params).ok_or(ReasoningNotAppliedReason::NoCatalogFact)?;
                match tag.thinking.as_mut() {
                    Some(thinking) => thinking.thinking_level = Some(thinking_level),
                    None => tag.thinking_level = Some(thinking_level),
                }
                Ok(())
            }
            Provider::SelfHosted | Provider::Other => Err(ReasoningNotAppliedReason::NoCatalogFact),
        }
    }

    fn openai_tag(params: &mut Option<ProviderParamsOverride>) -> Option<&mut OpenAiProviderTag> {
        let params = params.get_or_insert_with(ProviderParamsOverride::default);
        let tag = params
            .provider_tag
            .get_or_insert_with(|| ProviderTag::OpenAi(Default::default()));
        match tag {
            ProviderTag::OpenAi(tag) => Some(tag),
            ProviderTag::Anthropic(_) | ProviderTag::Gemini(_) | ProviderTag::Unknown { .. } => {
                None
            }
        }
    }

    fn anthropic_tag(
        params: &mut Option<ProviderParamsOverride>,
    ) -> Option<&mut AnthropicProviderTag> {
        let params = params.get_or_insert_with(ProviderParamsOverride::default);
        let tag = params
            .provider_tag
            .get_or_insert_with(|| ProviderTag::Anthropic(Default::default()));
        match tag {
            ProviderTag::Anthropic(tag) => Some(tag),
            ProviderTag::OpenAi(_) | ProviderTag::Gemini(_) | ProviderTag::Unknown { .. } => None,
        }
    }

    fn gemini_tag(params: &mut Option<ProviderParamsOverride>) -> Option<&mut GeminiProviderTag> {
        let params = params.get_or_insert_with(ProviderParamsOverride::default);
        let tag = params
            .provider_tag
            .get_or_insert_with(|| ProviderTag::Gemini(Default::default()));
        match tag {
            ProviderTag::Gemini(tag) => Some(tag),
            ProviderTag::Anthropic(_) | ProviderTag::OpenAi(_) | ProviderTag::Unknown { .. } => {
                None
            }
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

    fn baseline_effort(params: Option<&ProviderParamsOverride>) -> ReasoningLoweringBaseline {
        let explicit = match params.and_then(|params| params.provider_tag.as_ref()) {
            Some(ProviderTag::OpenAi(tag)) => tag.reasoning_effort.map(|effort| match effort {
                ReasoningEffort::None => EffortLevel::None,
                ReasoningEffort::Low => EffortLevel::Low,
                ReasoningEffort::Medium => EffortLevel::Medium,
                ReasoningEffort::High => EffortLevel::High,
                ReasoningEffort::XHigh => EffortLevel::Xhigh,
                ReasoningEffort::Max => EffortLevel::Max,
            }),
            Some(ProviderTag::Anthropic(tag)) => tag.effort.map(|effort| match effort {
                AnthropicEffort::Low => EffortLevel::Low,
                AnthropicEffort::Medium => EffortLevel::Medium,
                AnthropicEffort::High => EffortLevel::High,
                AnthropicEffort::XHigh => EffortLevel::Xhigh,
                AnthropicEffort::Max => EffortLevel::Max,
            }),
            Some(ProviderTag::Gemini(tag)) => tag
                .thinking
                .as_ref()
                .and_then(|thinking| thinking.thinking_level)
                .or(tag.thinking_level)
                .map(GeminiThinkingLevel::as_effort_level),
            Some(ProviderTag::Unknown { .. }) | None => None,
        };
        explicit.map_or(
            ReasoningLoweringBaseline::ProviderDefault,
            ReasoningLoweringBaseline::Explicit,
        )
    }
}

/// The request copy without the resolved marker, which the previous
/// lowering did not write.
fn without_marker(mut params: Option<ProviderParamsOverride>) -> Option<ProviderParamsOverride> {
    if let Some(ProviderTag::OpenAi(tag)) = params
        .as_mut()
        .and_then(|params| params.provider_tag.as_mut())
    {
        tag.reasoning_effort_resolved = false;
    }
    params
}

/// The preference wrapper gives the outcome, the reported baseline and the
/// request copy of the lowering it replaces, for every provider, catalog
/// row shape, level and baseline. The one difference is the resolved marker
/// on an applied OpenAI level.
#[test]
fn the_preference_wrapper_matches_the_previous_lowering() {
    use crate::agent::reasoning_preference::lower_reasoning_preference;

    let mut rows: Vec<Option<ModelCapabilities>> = vec![None];
    rows.extend(TEST_CATALOG.capabilities.iter().copied().map(Some));
    for provider in [
        Provider::OpenAI,
        Provider::Anthropic,
        Provider::Gemini,
        Provider::SelfHosted,
        Provider::Other,
    ] {
        for levels in [
            ALL_LEVELS,
            &[EffortLevel::None, Low, Medium, High],
            &[Minimal, Low, High],
            &[Low, Medium, High, Xhigh, Max],
            &[],
        ] {
            let base = row(provider, levels);
            rows.push(Some(base));
            rows.push(Some(ModelCapabilities {
                supports_reasoning: !base.supports_reasoning,
                ..base
            }));
            rows.push(Some(ModelCapabilities {
                thinking: ThinkingSupport::None,
                ..base
            }));
            rows.push(Some(ModelCapabilities {
                anthropic_disabled_thinking: AnthropicDisabledThinking::AtOrBelowHighEffort,
                ..base
            }));
            rows.push(Some(ModelCapabilities {
                anthropic_disabled_thinking: AnthropicDisabledThinking::AnyEffort,
                thinking: ThinkingSupport::AnthropicAdaptiveOrBetweenTools,
                ..base
            }));
        }
    }

    let baselines = baselines();
    let mut cases = 0_u32;
    let mut applied = 0_u32;
    let mut reasons = std::collections::BTreeSet::new();
    for provider in [
        Provider::OpenAI,
        Provider::Anthropic,
        Provider::Gemini,
        Provider::SelfHosted,
        Provider::Other,
    ] {
        for catalog_row in &rows {
            for baseline in &baselines {
                for level in LADDER {
                    cases += 1;
                    let context = format!(
                        "{provider:?} {level:?} row {:?} with {baseline:?}",
                        catalog_row.as_ref().map(|row| (
                            row.provider,
                            row.effort_levels,
                            row.supports_reasoning
                        )),
                    );
                    let mut expected_params = baseline.clone();
                    let expected = previous_lowering::lower(
                        catalog_row.as_ref(),
                        provider,
                        level,
                        &mut expected_params,
                    );
                    let mut params = baseline.clone();
                    let actual = lower_reasoning_preference(
                        catalog_row.as_ref(),
                        provider,
                        level,
                        &mut params,
                    );
                    assert_eq!(actual, expected, "{context}");
                    match actual.1 {
                        ReasoningLoweringOutcome::Applied(_) => {
                            applied += 1;
                            if provider == Provider::OpenAI {
                                assert!(
                                    openai_tag_of(&params).reasoning_effort_resolved,
                                    "{context}: an applied OpenAI level is marked"
                                );
                            }
                            assert_eq!(without_marker(params), expected_params, "{context}");
                        }
                        ReasoningLoweringOutcome::NotApplied(reason) => {
                            reasons.insert(format!("{reason:?}"));
                            assert_eq!(params, expected_params, "{context}");
                            assert_eq!(&params, baseline, "{context}");
                        }
                    }
                }
            }
        }
    }
    assert_eq!(rows.len(), 1 + 3 + 5 * 5 * 5);
    assert_eq!(cases, 5 * (1 + 3 + 5 * 5 * 5) * 34 * 7);
    assert!(applied > 1000, "the grid applies levels ({applied})");
    // The grid reaches every reason the previous lowering could give.
    assert_eq!(
        reasons.into_iter().collect::<Vec<_>>(),
        [
            "BudgetConflict",
            "NoCatalogFact",
            "OpaqueReasoningBody",
            "ReasoningDisabledByBaseline",
            "ThinkingModeConflict",
            "UnknownSupportedLevels",
            "UnsupportedLevel",
        ]
    );
}
