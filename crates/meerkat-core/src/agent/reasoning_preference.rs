//! Per-attempt lowering of a turn's reasoning-effort preference onto one
//! provider request's own copy of its params (see
//! [`crate::lifecycle::run_primitive::RequestReasoningPreference`]).
//!
//! Pure and bounded: it reads the selected model's catalog facts and the
//! baseline params, and either writes the provider's existing typed effort
//! field or leaves the baseline byte-identical. It never fails a request.

use crate::Provider;
use crate::lifecycle::run_primitive::{
    AnthropicEffort, AnthropicThinkingConfig, ProviderParamsOverride, ProviderTag, ReasoningEffort,
    ReasoningLoweringBaseline, ReasoningLoweringOutcome, ReasoningMode, ReasoningNotAppliedReason,
};
use crate::model_profile::capabilities::{EffortLevel, ModelCapabilities};

/// Lower `level` onto `params`, the request copy for `provider`, against the
/// selected model's catalog row (`None` for a model without one).
pub(crate) fn lower_reasoning_preference(
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
            if !(capabilities.supports_reasoning && capabilities.effort_levels.contains(&level)) {
                return Err(ReasoningNotAppliedReason::UnsupportedLevel);
            }
            let effort = openai_effort(level).ok_or(ReasoningNotAppliedReason::UnsupportedLevel)?;
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
            // Check against the baseline before writing anything.
            if let Some(ProviderTag::Anthropic(tag)) = params
                .as_ref()
                .and_then(|params| params.provider_tag.as_ref())
            {
                if tag.thinking_budget_tokens.is_some()
                    || matches!(tag.thinking, Some(AnthropicThinkingConfig::Enabled { .. }))
                {
                    return Err(ReasoningNotAppliedReason::BudgetConflict);
                }
                // Between-tools thinking is accepted at `high` effort or
                // below.
                if matches!(tag.thinking, Some(AnthropicThinkingConfig::BetweenTools))
                    && level > EffortLevel::High
                {
                    return Err(ReasoningNotAppliedReason::ThinkingModeConflict);
                }
            }
            let tag = anthropic_tag(params).ok_or(ReasoningNotAppliedReason::NoCatalogFact)?;
            tag.effort = Some(effort);
            Ok(())
        }
        // The catalog's Gemini thinking flag does not record which levels a
        // given model accepts, so no level is validated for it yet.
        Provider::Gemini => Err(ReasoningNotAppliedReason::UnknownSupportedLevels),
        Provider::SelfHosted | Provider::Other => Err(ReasoningNotAppliedReason::NoCatalogFact),
    }
}

/// The OpenAI tag of the request copy, created when absent. `None` when the
/// copy carries another provider's tag.
fn openai_tag(
    params: &mut Option<ProviderParamsOverride>,
) -> Option<&mut crate::lifecycle::run_primitive::OpenAiProviderTag> {
    let params = params.get_or_insert_with(ProviderParamsOverride::default);
    let tag = params
        .provider_tag
        .get_or_insert_with(|| ProviderTag::OpenAi(Default::default()));
    match tag {
        ProviderTag::OpenAi(tag) => Some(tag),
        ProviderTag::Anthropic(_) | ProviderTag::Gemini(_) | ProviderTag::Unknown { .. } => None,
    }
}

/// The Anthropic tag of the request copy, created when absent. `None` when
/// the copy carries another provider's tag.
fn anthropic_tag(
    params: &mut Option<ProviderParamsOverride>,
) -> Option<&mut crate::lifecycle::run_primitive::AnthropicProviderTag> {
    let params = params.get_or_insert_with(ProviderParamsOverride::default);
    let tag = params
        .provider_tag
        .get_or_insert_with(|| ProviderTag::Anthropic(Default::default()));
    match tag {
        ProviderTag::Anthropic(tag) => Some(tag),
        ProviderTag::OpenAi(_) | ProviderTag::Gemini(_) | ProviderTag::Unknown { .. } => None,
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

/// The effort the baseline request carries explicitly, if any.
pub(crate) fn baseline_effort(
    params: Option<&ProviderParamsOverride>,
) -> ReasoningLoweringBaseline {
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
            .map(|level| match level {
                crate::lifecycle::run_primitive::GeminiThinkingLevel::Minimal => {
                    EffortLevel::Minimal
                }
                crate::lifecycle::run_primitive::GeminiThinkingLevel::Low => EffortLevel::Low,
                crate::lifecycle::run_primitive::GeminiThinkingLevel::Medium => EffortLevel::Medium,
                crate::lifecycle::run_primitive::GeminiThinkingLevel::High => EffortLevel::High,
            }),
        Some(ProviderTag::Unknown { .. }) | None => None,
    };
    explicit.map_or(
        ReasoningLoweringBaseline::ProviderDefault,
        ReasoningLoweringBaseline::Explicit,
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::lifecycle::run_primitive::{
        AnthropicProviderTag, GeminiProviderTag, GeminiThinkingConfig, GeminiThinkingLevel,
        OpenAiProviderTag,
    };

    fn caps(
        provider: Provider,
        supports_reasoning: bool,
        levels: &'static [EffortLevel],
    ) -> ModelCapabilities {
        use crate::model_profile::test_catalog::{
            ANTHROPIC_MODEL, OPENAI_MODEL, TEST_CATALOG, VIDEO_MODEL,
        };
        let model = match provider {
            Provider::OpenAI => OPENAI_MODEL,
            Provider::Anthropic => ANTHROPIC_MODEL,
            _ => VIDEO_MODEL,
        };
        let row = TEST_CATALOG
            .capabilities_for(provider, model)
            .expect("test catalog row");
        ModelCapabilities {
            supports_reasoning,
            effort_levels: levels,
            ..*row
        }
    }

    const OPENAI_LEVELS: &[EffortLevel] = &[
        EffortLevel::None,
        EffortLevel::Low,
        EffortLevel::Medium,
        EffortLevel::High,
    ];
    const ANTHROPIC_LEVELS: &[EffortLevel] = &[
        EffortLevel::Low,
        EffortLevel::Medium,
        EffortLevel::High,
        EffortLevel::Max,
    ];

    #[test]
    fn openai_applies_a_catalog_level_and_reports_the_baseline() {
        let caps = caps(Provider::OpenAI, true, OPENAI_LEVELS);
        let mut params = Some(ProviderParamsOverride {
            temperature: Some(0.2),
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                reasoning_effort: Some(ReasoningEffort::High),
                ..Default::default()
            })),
            ..Default::default()
        });
        let (baseline, outcome) = lower_reasoning_preference(
            Some(&caps),
            Provider::OpenAI,
            EffortLevel::Low,
            &mut params,
        );
        assert_eq!(
            baseline,
            ReasoningLoweringBaseline::Explicit(EffortLevel::High)
        );
        assert_eq!(outcome, ReasoningLoweringOutcome::Applied(EffortLevel::Low));
        let params = params.expect("params");
        assert_eq!(params.temperature, Some(0.2), "unrelated knobs stay");
        let Some(ProviderTag::OpenAi(tag)) = params.provider_tag else {
            panic!("openai tag")
        };
        assert_eq!(tag.reasoning_effort, Some(ReasoningEffort::Low));

        let mut absent = None;
        let (baseline, outcome) = lower_reasoning_preference(
            Some(&caps),
            Provider::OpenAI,
            EffortLevel::Low,
            &mut absent,
        );
        assert_eq!(baseline, ReasoningLoweringBaseline::ProviderDefault);
        assert_eq!(outcome, ReasoningLoweringOutcome::Applied(EffortLevel::Low));
    }

    #[test]
    fn not_applied_leaves_the_baseline_byte_identical() {
        let openai_without_reasoning = caps(Provider::OpenAI, false, OPENAI_LEVELS);
        let anthropic = caps(Provider::Anthropic, false, ANTHROPIC_LEVELS);
        let gemini = caps(Provider::Gemini, false, &[]);
        let cases: Vec<(
            Option<&ModelCapabilities>,
            Provider,
            EffortLevel,
            Option<ProviderParamsOverride>,
            ReasoningNotAppliedReason,
        )> = vec![
            (
                None,
                Provider::OpenAI,
                EffortLevel::Low,
                None,
                ReasoningNotAppliedReason::NoCatalogFact,
            ),
            (
                Some(&openai_without_reasoning),
                Provider::OpenAI,
                EffortLevel::Low,
                None,
                ReasoningNotAppliedReason::UnsupportedLevel,
            ),
            (
                Some(&anthropic),
                Provider::Anthropic,
                EffortLevel::None,
                Some(ProviderParamsOverride {
                    provider_tag: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                        effort: Some(AnthropicEffort::Medium),
                        ..Default::default()
                    })),
                    ..Default::default()
                }),
                ReasoningNotAppliedReason::UnsupportedLevel,
            ),
            (
                Some(&anthropic),
                Provider::Anthropic,
                EffortLevel::Low,
                Some(ProviderParamsOverride {
                    provider_tag: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                        thinking: Some(AnthropicThinkingConfig::Enabled {
                            budget_tokens: 2048,
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                }),
                ReasoningNotAppliedReason::BudgetConflict,
            ),
            (
                Some(&gemini),
                Provider::Gemini,
                EffortLevel::Low,
                Some(ProviderParamsOverride {
                    provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag {
                        thinking: Some(GeminiThinkingConfig {
                            include_thoughts: Some(true),
                            thinking_level: Some(GeminiThinkingLevel::High),
                            thinking_budget: None,
                        }),
                        ..Default::default()
                    })),
                    ..Default::default()
                }),
                ReasoningNotAppliedReason::UnknownSupportedLevels,
            ),
            (
                Some(&anthropic),
                Provider::Anthropic,
                EffortLevel::Low,
                Some(ProviderParamsOverride {
                    reasoning: Some(ReasoningMode::Off),
                    ..Default::default()
                }),
                ReasoningNotAppliedReason::ReasoningDisabledByBaseline,
            ),
        ];
        for (caps, provider, level, params, reason) in cases {
            let mut copy = params.clone();
            let (_, outcome) = lower_reasoning_preference(caps, provider, level, &mut copy);
            assert_eq!(
                outcome,
                ReasoningLoweringOutcome::NotApplied(reason),
                "{provider:?} {level:?}"
            );
            assert_eq!(copy, params, "{reason:?}: the baseline is untouched");
        }
    }

    #[test]
    fn anthropic_applies_effort_and_between_tools_caps_it_at_high() {
        let caps = caps(Provider::Anthropic, false, ANTHROPIC_LEVELS);
        let between_tools = Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                thinking: Some(AnthropicThinkingConfig::BetweenTools),
                ..Default::default()
            })),
            ..Default::default()
        });
        let mut low = between_tools.clone();
        let (_, outcome) = lower_reasoning_preference(
            Some(&caps),
            Provider::Anthropic,
            EffortLevel::Low,
            &mut low,
        );
        assert_eq!(outcome, ReasoningLoweringOutcome::Applied(EffortLevel::Low));
        let Some(ProviderTag::Anthropic(tag)) = low.and_then(|params| params.provider_tag) else {
            panic!("anthropic tag")
        };
        assert_eq!(tag.effort, Some(AnthropicEffort::Low));
        assert_eq!(tag.thinking, Some(AnthropicThinkingConfig::BetweenTools));

        let mut max = between_tools.clone();
        let (_, outcome) = lower_reasoning_preference(
            Some(&caps),
            Provider::Anthropic,
            EffortLevel::Max,
            &mut max,
        );
        assert_eq!(
            outcome,
            ReasoningLoweringOutcome::NotApplied(ReasoningNotAppliedReason::ThinkingModeConflict)
        );
        assert_eq!(max, between_tools);
    }

    #[test]
    fn gemini_nested_level_is_reported_as_the_baseline() {
        let params = ProviderParamsOverride {
            provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag {
                thinking: Some(GeminiThinkingConfig {
                    include_thoughts: None,
                    thinking_level: Some(GeminiThinkingLevel::High),
                    thinking_budget: None,
                }),
                thinking_level: Some(GeminiThinkingLevel::Low),
                ..Default::default()
            })),
            ..Default::default()
        };
        assert_eq!(
            baseline_effort(Some(&params)),
            ReasoningLoweringBaseline::Explicit(EffortLevel::High),
            "nested wins over flat"
        );
    }
}
