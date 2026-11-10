//! Per-attempt lowering of a turn's reasoning-effort preference onto one
//! provider request's own copy of its params (see
//! [`crate::lifecycle::run_primitive::RequestReasoningPreference`]).
//!
//! A thin wrapper over the resolver of the provider-neutral
//! `reasoning_effort` option ([`crate::reasoning_effort`]) in its preference
//! mode, against the provider's direct route and the selected model's
//! catalog row. Pure and bounded: it either writes the provider's existing
//! typed effort field or leaves the baseline byte-identical. It never fails
//! a request.

use crate::Provider;
use crate::lifecycle::run_primitive::{
    ProviderParamsOverride, ProviderTag, ReasoningLoweringBaseline, ReasoningLoweringOutcome,
    ReasoningNotAppliedReason,
};
use crate::model_profile::capabilities::{EffortLevel, ModelCapabilities};
use crate::reasoning_effort::{
    ReasoningEffortDecidedBy, ReasoningEffortRefusalReason, ReasoningResolveInput,
    ReasoningResolveMode, direct_route_observation, explicit_provider_effort,
    resolve_reasoning_effort,
};

// Named by the rows below through `super::*`.
#[cfg(test)]
use crate::lifecycle::run_primitive::{
    AnthropicEffort, AnthropicThinkingConfig, ReasoningEffort, ReasoningMode,
};

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
    // The preference reads catalog facts: a model without a row of its own
    // provider has none, whatever else the baseline says.
    let Some(row) = capabilities.filter(|caps| caps.provider == provider) else {
        return Err(ReasoningNotAppliedReason::NoCatalogFact);
    };
    let observation = direct_route_observation(provider);
    let input = ReasoningResolveInput {
        requested: level,
        provider,
        model: row.id,
        observation: observation.as_ref(),
        catalog_row: Some(row),
        decided_by: ReasoningEffortDecidedBy::Preference,
    };
    match resolve_reasoning_effort(input, params, ReasoningResolveMode::Preference) {
        Ok(_) => Ok(()),
        Err(refusal) => Err(not_applied_reason(
            refusal.reason,
            observation.is_some(),
            carries_another_providers_tag(provider, params.as_ref()),
        )),
    }
}

/// The preference's reason for a refused level. `has_direct_route` is
/// whether the provider has a direct route to judge the level on (the
/// self-hosted family and a provider outside the families have none);
/// `another_providers_tag` is whether the request copy carries a tag that
/// is not the provider's own.
fn not_applied_reason(
    reason: ReasoningEffortRefusalReason,
    has_direct_route: bool,
    another_providers_tag: bool,
) -> ReasoningNotAppliedReason {
    use ReasoningEffortRefusalReason as Refusal;
    match reason {
        Refusal::UnsupportedLevel => ReasoningNotAppliedReason::UnsupportedLevel,
        // No route to judge the level on: no catalog fact. A row that
        // records no levels, or a source that cannot be read now: unknown.
        Refusal::SupportedLevelsUnknown if !has_direct_route => {
            ReasoningNotAppliedReason::NoCatalogFact
        }
        Refusal::SupportedLevelsUnknown | Refusal::SupportedLevelsUnavailable => {
            ReasoningNotAppliedReason::UnknownSupportedLevels
        }
        Refusal::ConflictingThinkingBudget => ReasoningNotAppliedReason::BudgetConflict,
        Refusal::ConflictingThinkingType => ReasoningNotAppliedReason::ThinkingModeConflict,
        // A request copy that carries another provider's tag has no place
        // for the provider's typed effort field: no catalog fact.
        Refusal::ConflictingReasoningBody | Refusal::ConflictingProviderEffort
            if another_providers_tag =>
        {
            ReasoningNotAppliedReason::NoCatalogFact
        }
        // The preference mode replaces a provider effort and never refuses
        // on one; the reason is listed because the set is closed.
        Refusal::ConflictingReasoningBody | Refusal::ConflictingProviderEffort => {
            ReasoningNotAppliedReason::OpaqueReasoningBody
        }
        Refusal::ConflictingReasoningMode => ReasoningNotAppliedReason::ReasoningDisabledByBaseline,
    }
}

/// Whether the request copy carries a provider tag that is not `provider`'s
/// own typed tag.
fn carries_another_providers_tag(
    provider: Provider,
    params: Option<&ProviderParamsOverride>,
) -> bool {
    match (
        provider,
        params.and_then(|params| params.provider_tag.as_ref()),
    ) {
        (_, None)
        | (Provider::OpenAI, Some(ProviderTag::OpenAi(_)))
        | (Provider::Anthropic, Some(ProviderTag::Anthropic(_)))
        | (Provider::Gemini, Some(ProviderTag::Gemini(_))) => false,
        (_, Some(_)) => true,
    }
}

/// The effort the baseline request carries explicitly, if any.
pub(crate) fn baseline_effort(
    params: Option<&ProviderParamsOverride>,
) -> ReasoningLoweringBaseline {
    explicit_provider_effort(params).map_or(
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
    fn anthropic_disabled_thinking_caps_the_level_only_where_the_row_records_a_ceiling() {
        use crate::model_profile::capabilities::AnthropicDisabledThinking;
        let disabled = Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                thinking: Some(AnthropicThinkingConfig::Disabled),
                ..Default::default()
            })),
            ..Default::default()
        });
        let row = |ceiling| ModelCapabilities {
            anthropic_disabled_thinking: ceiling,
            ..caps(Provider::Anthropic, false, ANTHROPIC_LEVELS)
        };
        let lower = |caps: &ModelCapabilities, level| {
            let mut params = disabled.clone();
            let (_, outcome) =
                lower_reasoning_preference(Some(caps), Provider::Anthropic, level, &mut params);
            (outcome, params)
        };

        let capped = row(AnthropicDisabledThinking::AtOrBelowHighEffort);
        let (outcome, params) = lower(&capped, EffortLevel::High);
        assert_eq!(
            outcome,
            ReasoningLoweringOutcome::Applied(EffortLevel::High)
        );
        let Some(ProviderTag::Anthropic(tag)) = params.and_then(|params| params.provider_tag)
        else {
            panic!("anthropic tag")
        };
        assert_eq!(tag.effort, Some(AnthropicEffort::High));
        assert_eq!(tag.thinking, Some(AnthropicThinkingConfig::Disabled));

        let (outcome, params) = lower(&capped, EffortLevel::Max);
        assert_eq!(
            outcome,
            ReasoningLoweringOutcome::NotApplied(ReasoningNotAppliedReason::ThinkingModeConflict)
        );
        assert_eq!(params, disabled, "the baseline is untouched");

        let any = row(AnthropicDisabledThinking::AnyEffort);
        let (outcome, _) = lower(&any, EffortLevel::Max);
        assert_eq!(outcome, ReasoningLoweringOutcome::Applied(EffortLevel::Max));
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

    const GEMINI_LEVELS: &[EffortLevel] =
        &[EffortLevel::Low, EffortLevel::Medium, EffortLevel::High];

    fn gemini_tag(params: Option<ProviderParamsOverride>) -> GeminiProviderTag {
        match params.and_then(|params| params.provider_tag) {
            Some(ProviderTag::Gemini(tag)) => tag,
            other => panic!("expected a gemini tag, got {other:?}"),
        }
    }

    /// A Gemini thinking-level row lowers an accepted preference onto the
    /// level field the request actually sends: the nested `thinking` config
    /// when the baseline has one (it wins over the flat field on the wire),
    /// else the flat field, creating the tag when the baseline has none.
    #[test]
    fn gemini_applies_a_catalog_level_to_the_thinking_level_it_sends() {
        let caps = caps(Provider::Gemini, false, GEMINI_LEVELS);

        let mut flat = Some(ProviderParamsOverride {
            temperature: Some(0.2),
            provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag {
                thinking_level: Some(GeminiThinkingLevel::High),
                top_k: Some(20),
                ..Default::default()
            })),
            ..Default::default()
        });
        let (baseline, outcome) =
            lower_reasoning_preference(Some(&caps), Provider::Gemini, EffortLevel::Low, &mut flat);
        assert_eq!(
            baseline,
            ReasoningLoweringBaseline::Explicit(EffortLevel::High)
        );
        assert_eq!(outcome, ReasoningLoweringOutcome::Applied(EffortLevel::Low));
        assert_eq!(flat.as_ref().and_then(|p| p.temperature), Some(0.2));
        let tag = gemini_tag(flat);
        assert_eq!(tag.thinking_level, Some(GeminiThinkingLevel::Low));
        assert_eq!(tag.top_k, Some(20), "unrelated knobs stay");

        let mut nested = Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag {
                thinking: Some(GeminiThinkingConfig {
                    include_thoughts: Some(true),
                    thinking_level: Some(GeminiThinkingLevel::High),
                    thinking_budget: None,
                }),
                ..Default::default()
            })),
            ..Default::default()
        });
        let (_, outcome) = lower_reasoning_preference(
            Some(&caps),
            Provider::Gemini,
            EffortLevel::Medium,
            &mut nested,
        );
        assert_eq!(
            outcome,
            ReasoningLoweringOutcome::Applied(EffortLevel::Medium)
        );
        let thinking = gemini_tag(nested).thinking.expect("nested thinking config");
        assert_eq!(thinking.thinking_level, Some(GeminiThinkingLevel::Medium));
        assert_eq!(thinking.include_thoughts, Some(true));

        let mut absent = None;
        let (baseline, outcome) = lower_reasoning_preference(
            Some(&caps),
            Provider::Gemini,
            EffortLevel::High,
            &mut absent,
        );
        assert_eq!(baseline, ReasoningLoweringBaseline::ProviderDefault);
        assert_eq!(
            outcome,
            ReasoningLoweringOutcome::Applied(EffortLevel::High)
        );
        assert_eq!(
            gemini_tag(absent).thinking_level,
            Some(GeminiThinkingLevel::High)
        );
    }

    /// A level outside the row, a row that records no levels, and a baseline
    /// thinking budget (which a level would replace on the wire) leave the
    /// Gemini baseline byte-identical.
    #[test]
    fn gemini_not_applied_leaves_the_baseline_byte_identical() {
        let unrecorded = caps(Provider::Gemini, false, &[]);
        let levels = caps(Provider::Gemini, false, GEMINI_LEVELS);
        let budget_tag = |thinking_budget: Option<u32>, nested_budget: Option<u32>| {
            Some(ProviderParamsOverride {
                provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag {
                    thinking_budget,
                    thinking: nested_budget.map(|budget| GeminiThinkingConfig {
                        include_thoughts: None,
                        thinking_level: None,
                        thinking_budget: Some(budget),
                    }),
                    ..Default::default()
                })),
                ..Default::default()
            })
        };
        let cases: Vec<(
            &ModelCapabilities,
            EffortLevel,
            Option<ProviderParamsOverride>,
            ReasoningNotAppliedReason,
        )> = vec![
            (
                &levels,
                EffortLevel::Xhigh,
                None,
                ReasoningNotAppliedReason::UnsupportedLevel,
            ),
            (
                &levels,
                EffortLevel::None,
                None,
                ReasoningNotAppliedReason::UnsupportedLevel,
            ),
            (
                &unrecorded,
                EffortLevel::Low,
                None,
                ReasoningNotAppliedReason::UnknownSupportedLevels,
            ),
            (
                &levels,
                EffortLevel::Low,
                budget_tag(Some(4096), None),
                ReasoningNotAppliedReason::BudgetConflict,
            ),
            (
                &levels,
                EffortLevel::Low,
                budget_tag(None, Some(4096)),
                ReasoningNotAppliedReason::BudgetConflict,
            ),
            (
                &levels,
                EffortLevel::Low,
                Some(ProviderParamsOverride {
                    thinking_budget_tokens: Some(4096),
                    ..Default::default()
                }),
                ReasoningNotAppliedReason::BudgetConflict,
            ),
        ];
        for (caps, level, params, reason) in cases {
            let mut copy = params.clone();
            let (_, outcome) =
                lower_reasoning_preference(Some(caps), Provider::Gemini, level, &mut copy);
            assert_eq!(
                outcome,
                ReasoningLoweringOutcome::NotApplied(reason),
                "{level:?}"
            );
            assert_eq!(copy, params, "{reason:?}: the baseline is untouched");
        }
    }
}
