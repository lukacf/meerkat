//! Pure pre-dispatch context-budget classification.
//!
//! The effective model registry remains the singular owner of the context
//! window and optional input ceiling. This module accepts a registry-minted
//! [`ModelProfileWitness`] and projects a loaded durable [`Session`] plus the
//! exact visible tool set and output reserve into one typed, side-effect-free
//! fact. Hosts can observe the
//! projection without maintaining a second model-limit table. Forecasts never
//! authorize runtime behavior; only exact provider-issued token evidence may
//! authorize pre-dispatch refusal.

use crate::{Message, ModelProfileWitness, ProviderRequestPressure, Session, ToolDef, ToolName};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Typed pre-dispatch state of one exact request budget projection.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextBudgetState {
    /// The request remains within the witnessed context and input limits.
    Within,
    /// Forecast evidence projects the request beyond the active context
    /// window or input ceiling, but does not authorize refusal.
    ForecastExceeded,
    /// Exact provider-issued token evidence exceeds the active model profile's
    /// context window or input ceiling.
    Exceeded,
}

/// Provenance of the effective input-token value used for classification.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextBudgetEstimateProvenance {
    /// Host-side forecast over canonical messages and visible tool definitions.
    #[default]
    CanonicalForecast,
    /// Exact input-token count issued by the provider for the fully lowered
    /// request.
    ExactProviderTokenCount,
}

impl ContextBudgetState {
    /// Whether provider dispatch must be refused for this fact.
    pub fn requires_dispatch_refusal(self) -> bool {
        matches!(self, Self::Exceeded)
    }
}

/// Deterministic budget evidence for a loaded durable session and one exact
/// prospective provider request.
///
/// This is a projection, not authority. The context window is copied only as
/// evidence from the supplied registry-minted [`ModelProfileWitness`]; callers
/// cannot provide an independent limit to this classifier.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ContextBudgetFact {
    /// Typed classification for host and runtime policy.
    pub state: ContextBudgetState,
    /// Active registry-owned model context window.
    pub context_window_tokens: u32,
    /// Active registry-owned separate input ceiling, when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u32>,
    /// Observational estimate for the durable ordered transcript.
    pub estimated_input_tokens: u64,
    /// Observational estimate for the exact visible tool definitions.
    pub estimated_tool_tokens: u64,
    /// Effective output tokens reserved by the prospective request.
    pub reserved_output_tokens: u32,
    /// Effective pre-output estimate plus the output reserve.
    ///
    /// An exact provider-issued count is used when available. Otherwise, this
    /// remains the canonical input/tool forecast.
    pub estimated_total_tokens: u64,
    /// Input headroom under both the input ceiling and context-minus-output
    /// budget, otherwise zero.
    pub remaining_tokens: u64,
    /// Greatest excess over either the context window or input ceiling,
    /// otherwise zero.
    pub overage_tokens: u64,
    /// Provenance of the effective input-token value used by `state`.
    #[serde(default)]
    pub estimate_provenance: ContextBudgetEstimateProvenance,
    /// Exact provider-lowered JSON body bytes, when runtime lowering can
    /// produce a witness before dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_lowered_encoded_bytes: Option<u64>,
    /// Exact provider-issued input-token count, when the provider exposes one
    /// synchronously before dispatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_issued_input_tokens: Option<u64>,
    /// Explicit identity of the fully lowered body whose pressure was
    /// observed. This remains observational unless accompanied by an exact
    /// provider-issued token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lowered_request_provenance: Option<crate::LoweredRequestProvenance>,
}

impl ContextBudgetFact {
    /// Whether provider dispatch must be refused for this fact.
    pub fn requires_dispatch_refusal(&self) -> bool {
        self.state.requires_dispatch_refusal()
            && self.estimate_provenance == ContextBudgetEstimateProvenance::ExactProviderTokenCount
    }

    /// Effective input-side token count for this request: the exact
    /// provider-issued count when one exists, otherwise the canonical
    /// transcript-plus-tools forecast.
    ///
    /// The output reserve is subtracted rather than the components re-summed,
    /// because summing `estimated_input_tokens + estimated_tool_tokens` would
    /// silently discard an exact provider-issued count. Excluding the reserve
    /// keeps this value comparable with an input-token threshold: what the
    /// request presents to the model, not what the response may add.
    #[must_use]
    pub fn effective_input_tokens(&self) -> u64 {
        self.estimated_total_tokens
            .saturating_sub(u64::from(self.reserved_output_tokens))
    }
}

/// Typed failure to construct a context-budget fact.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContextBudgetFactError {
    /// The active registry entry did not declare a context window, so a
    /// fail-closed classification cannot be made.
    #[error("active model profile has no context-window authority")]
    ContextWindowUnavailable,
    /// The durable transcript could not be measured.
    #[error("failed to estimate durable transcript tokens: {detail}")]
    InputEstimationFailed {
        /// Stable diagnostic from the estimator failure.
        detail: String,
    },
    /// One exact visible tool definition could not be measured.
    #[error("failed to estimate visible tool '{tool_name}' tokens: {detail}")]
    ToolEstimationFailed {
        /// Typed name of the tool definition that failed measurement.
        tool_name: ToolName,
        /// Stable diagnostic from the serialization failure.
        detail: String,
    },
}

/// Classify a loaded durable session against the exact active model profile
/// before provider dispatch.
///
/// `visible_tools` must be the final tool definitions exposed to this request,
/// after capability and policy filtering. `reserved_output_tokens` must be the
/// effective request-level output reserve. Neither value is inferred here,
/// because their owners are the request construction pipeline rather than the
/// model registry.
pub fn context_budget_fact_for_session(
    session: &Session,
    visible_tools: &[Arc<ToolDef>],
    reserved_output_tokens: u32,
    active_model_profile: &ModelProfileWitness,
) -> Result<ContextBudgetFact, ContextBudgetFactError> {
    context_budget_fact_for_messages(
        &session.messages_for_model_boundary(),
        visible_tools,
        reserved_output_tokens,
        active_model_profile,
    )
}

/// Classify the exact fully materialized request messages against the active
/// model profile before provider dispatch.
///
/// Runtime request construction uses this entry point after hydration and
/// injected-context lowering. It shares the same fact and estimator as the
/// durable-session convenience wrapper, preventing host and runtime policy
/// from drifting onto different token heuristics.
pub fn context_budget_fact_for_messages(
    messages: &[Message],
    visible_tools: &[Arc<ToolDef>],
    reserved_output_tokens: u32,
    active_model_profile: &ModelProfileWitness,
) -> Result<ContextBudgetFact, ContextBudgetFactError> {
    context_budget_fact_for_messages_with_pressure(
        messages,
        visible_tools,
        reserved_output_tokens,
        active_model_profile,
        None,
    )
}

/// Classify exact fully materialized request messages using the provider's
/// exact lowered-body pressure witness.
///
/// The canonical message/tool estimates remain visible as diagnostic
/// components and the lowered byte count remains observable evidence. An exact
/// provider-issued token count is authoritative when present. Bytes alone are
/// never converted into token truth or runtime authority.
pub fn context_budget_fact_for_provider_request(
    messages: &[Message],
    visible_tools: &[Arc<ToolDef>],
    reserved_output_tokens: u32,
    active_model_profile: &ModelProfileWitness,
    provider_request_pressure: ProviderRequestPressure,
) -> Result<ContextBudgetFact, ContextBudgetFactError> {
    context_budget_fact_for_messages_with_pressure(
        messages,
        visible_tools,
        reserved_output_tokens,
        active_model_profile,
        Some(provider_request_pressure),
    )
}

fn context_budget_fact_for_messages_with_pressure(
    messages: &[Message],
    visible_tools: &[Arc<ToolDef>],
    reserved_output_tokens: u32,
    active_model_profile: &ModelProfileWitness,
    provider_request_pressure: Option<ProviderRequestPressure>,
) -> Result<ContextBudgetFact, ContextBudgetFactError> {
    let context_window_tokens = active_model_profile
        .context_window()
        .ok_or(ContextBudgetFactError::ContextWindowUnavailable)?;
    let max_input_tokens = active_model_profile.max_input_tokens();

    let estimated_input_tokens = crate::agent::compact::estimate_tokens(messages)
        .map_err(|error| ContextBudgetFactError::InputEstimationFailed {
            detail: error.to_string(),
        })?
        // Account for per-message framing lost to integer division inside the
        // estimator. This keeps collections of short rows visible in the
        // observational forecast.
        .saturating_add(messages.len() as u64);

    let estimated_tool_tokens = visible_tools.iter().try_fold(
        0_u64,
        |total, tool| -> Result<u64, ContextBudgetFactError> {
            let serialized = serde_json::to_vec(tool.as_ref()).map_err(|error| {
                ContextBudgetFactError::ToolEstimationFailed {
                    tool_name: tool.tool_name(),
                    detail: error.to_string(),
                }
            })?;
            let tokens = (serialized.len() as u64).div_ceil(4);
            Ok(total.saturating_add(tokens))
        },
    )?;

    let canonical_input_and_tools = estimated_input_tokens.saturating_add(estimated_tool_tokens);
    let provider_issued_input_tokens =
        provider_request_pressure.and_then(|pressure| pressure.provider_issued_input_tokens);
    let effective_input_and_tools =
        provider_issued_input_tokens.unwrap_or(canonical_input_and_tools);
    let estimated_total_tokens =
        effective_input_and_tools.saturating_add(u64::from(reserved_output_tokens));
    let context_window = u64::from(context_window_tokens);
    let remaining_tokens = max_input_tokens.map_or(
        context_window.saturating_sub(estimated_total_tokens),
        |input_limit| {
            context_window
                .saturating_sub(estimated_total_tokens)
                .min(u64::from(input_limit).saturating_sub(effective_input_and_tools))
        },
    );
    let overage_tokens = estimated_total_tokens.saturating_sub(context_window).max(
        max_input_tokens.map_or(0, |input_limit| {
            effective_input_and_tools.saturating_sub(u64::from(input_limit))
        }),
    );
    let state = if overage_tokens > 0 {
        if provider_issued_input_tokens.is_some() {
            ContextBudgetState::Exceeded
        } else {
            ContextBudgetState::ForecastExceeded
        }
    } else {
        ContextBudgetState::Within
    };

    Ok(ContextBudgetFact {
        state,
        context_window_tokens,
        max_input_tokens,
        estimated_input_tokens,
        estimated_tool_tokens,
        reserved_output_tokens,
        estimated_total_tokens,
        remaining_tokens,
        overage_tokens,
        estimate_provenance: if provider_issued_input_tokens.is_some() {
            ContextBudgetEstimateProvenance::ExactProviderTokenCount
        } else {
            ContextBudgetEstimateProvenance::CanonicalForecast
        },
        provider_lowered_encoded_bytes: provider_request_pressure
            .map(|pressure| pressure.encoded_bytes),
        provider_issued_input_tokens,
        lowered_request_provenance: provider_request_pressure
            .and_then(|pressure| pressure.lowered_request_provenance),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CustomModelConfig;
    use crate::model_profile::test_catalog::TEST_CATALOG;
    use crate::{Config, ModelRegistry, Provider};

    const MODEL: &str = "context-budget-test-model";

    fn profile_witness(
        context_window: Option<u32>,
    ) -> Result<ModelProfileWitness, Box<dyn std::error::Error>> {
        profile_witness_with_input_limit(context_window, None)
    }

    fn profile_witness_with_input_limit(
        context_window: Option<u32>,
        max_input_tokens: Option<u32>,
    ) -> Result<ModelProfileWitness, Box<dyn std::error::Error>> {
        let mut config = Config::default();
        config.models.custom.insert(
            MODEL.to_string(),
            CustomModelConfig {
                provider: Provider::OpenAI,
                display_name: None,
                context_window,
                max_input_tokens,
                max_output_tokens: Some(8_192),
                vision: None,
                web_search: None,
                call_timeout_secs: None,
            },
        );
        let registry = ModelRegistry::from_config(&config, *TEST_CATALOG)?;
        registry
            .profile_witness_for_provider(Provider::OpenAI, MODEL)
            .ok_or_else(|| std::io::Error::other("missing exact test profile witness").into())
    }

    fn session_with_system_bytes(bytes: usize) -> Session {
        let mut session = Session::new();
        session.append_system_message("a".repeat(bytes));
        session
    }

    #[test]
    fn classification_is_deterministic_and_accounts_for_all_components()
    -> Result<(), Box<dyn std::error::Error>> {
        let session = session_with_system_bytes(1_900);
        let profile = profile_witness(Some(1_000))?;
        let tools = [Arc::new(ToolDef::new(
            "lookup",
            "look up an exact record",
            serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } }
            }),
        ))];

        let first = context_budget_fact_for_session(&session, &tools, 100, &profile)?;
        let second = context_budget_fact_for_session(&session, &tools, 100, &profile)?;

        assert_eq!(first, second);
        assert!(first.estimated_input_tokens > 0);
        assert!(first.estimated_tool_tokens > 0);
        assert_eq!(first.reserved_output_tokens, 100);
        assert_eq!(
            first.estimated_total_tokens,
            first
                .estimated_input_tokens
                .saturating_add(first.estimated_tool_tokens)
                .saturating_add(100)
        );
        Ok(())
    }

    #[test]
    fn provider_lowered_bytes_are_observable_but_not_token_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let session = session_with_system_bytes(20);
        let profile = profile_witness(Some(1_000))?;
        let pressure = ProviderRequestPressure::new(2_400, Some(10_000));

        let fact = context_budget_fact_for_provider_request(
            session.messages(),
            &[],
            100,
            &profile,
            pressure,
        )?;

        assert_eq!(
            fact.estimate_provenance,
            ContextBudgetEstimateProvenance::CanonicalForecast
        );
        assert_eq!(fact.provider_lowered_encoded_bytes, Some(2_400));
        assert!(fact.estimated_total_tokens < 1_000);
        assert_eq!(fact.state, ContextBudgetState::Within);
        assert!(!fact.requires_dispatch_refusal());
        Ok(())
    }

    #[test]
    fn provider_lowered_witness_never_shrinks_the_canonical_forecast()
    -> Result<(), Box<dyn std::error::Error>> {
        let session = session_with_system_bytes(2_400);
        let profile = profile_witness(Some(10_000))?;
        let forecast = context_budget_fact_for_session(&session, &[], 100, &profile)?;

        let witnessed = context_budget_fact_for_provider_request(
            session.messages(),
            &[],
            100,
            &profile,
            ProviderRequestPressure::new(12, None),
        )?;

        assert_eq!(
            witnessed.estimated_total_tokens,
            forecast.estimated_total_tokens
        );
        assert_eq!(
            witnessed.estimated_input_tokens,
            forecast.estimated_input_tokens
        );
        assert_eq!(
            witnessed.estimate_provenance,
            ContextBudgetEstimateProvenance::CanonicalForecast
        );
        assert_eq!(witnessed.provider_lowered_encoded_bytes, Some(12));
        Ok(())
    }

    #[test]
    fn exact_provider_token_count_dominates_all_forecasts() -> Result<(), Box<dyn std::error::Error>>
    {
        let session = session_with_system_bytes(20);
        let profile = profile_witness(Some(1_000))?;
        let pressure =
            ProviderRequestPressure::new(2_400, None).with_provider_issued_input_tokens(1_100);

        let fact = context_budget_fact_for_provider_request(
            session.messages(),
            &[],
            100,
            &profile,
            pressure,
        )?;

        assert_eq!(
            fact.estimate_provenance,
            ContextBudgetEstimateProvenance::ExactProviderTokenCount
        );
        assert_eq!(fact.provider_issued_input_tokens, Some(1_100));
        assert_eq!(fact.estimated_total_tokens, 1_200);
        assert_eq!(fact.state, ContextBudgetState::Exceeded);
        assert!(fact.requires_dispatch_refusal());
        Ok(())
    }

    #[test]
    fn effective_input_tokens_prefers_exact_provider_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let session = session_with_system_bytes(2_000);
        let profile = profile_witness(Some(1_000_000))?;
        let tools = [Arc::new(ToolDef::new(
            "lookup",
            "look up an exact record",
            serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } }
            }),
        ))];

        let forecast = context_budget_fact_for_session(&session, &tools, 8_192, &profile)?;
        assert_eq!(
            forecast.effective_input_tokens(),
            forecast
                .estimated_input_tokens
                .saturating_add(forecast.estimated_tool_tokens),
            "the forecast input side must include the visible tool definitions"
        );
        assert!(
            forecast.effective_input_tokens() > forecast.estimated_input_tokens,
            "tool definitions must be visible in the input-side total"
        );

        let counted = context_budget_fact_for_provider_request(
            session.messages(),
            &tools,
            8_192,
            &profile,
            ProviderRequestPressure::new(2_400, None).with_provider_issued_input_tokens(640_000),
        )?;
        assert_eq!(
            counted.effective_input_tokens(),
            640_000,
            "an exact provider-issued count must not be replaced by the component sum"
        );
        Ok(())
    }

    #[test]
    fn state_distinguishes_within_and_forecast_exceeded() -> Result<(), Box<dyn std::error::Error>>
    {
        let profile = profile_witness(Some(1_000))?;

        let within =
            context_budget_fact_for_session(&session_with_system_bytes(200), &[], 100, &profile)?;
        let exceeded =
            context_budget_fact_for_session(&session_with_system_bytes(4_000), &[], 100, &profile)?;

        assert_eq!(within.state, ContextBudgetState::Within);
        assert_eq!(exceeded.state, ContextBudgetState::ForecastExceeded);
        Ok(())
    }

    #[test]
    fn missing_registry_context_window_fails_typed() -> Result<(), Box<dyn std::error::Error>> {
        let profile = profile_witness(None)?;
        assert!(matches!(
            context_budget_fact_for_session(&session_with_system_bytes(20), &[], 0, &profile),
            Err(ContextBudgetFactError::ContextWindowUnavailable)
        ));
        Ok(())
    }

    #[test]
    fn separate_input_ceiling_and_shared_window_are_independent()
    -> Result<(), Box<dyn std::error::Error>> {
        let profile = profile_witness_with_input_limit(Some(1_000), Some(800))?;
        for (input, reserve, remaining, overage) in [
            (799, 100, 1, 0),
            (800, 200, 0, 0),
            (801, 1, 0, 1),
            (790, 220, 0, 10),
            (1_100, 300, 0, 400),
        ] {
            let fact = context_budget_fact_for_provider_request(
                &[],
                &[],
                reserve,
                &profile,
                ProviderRequestPressure::new(1, None).with_provider_issued_input_tokens(input),
            )?;
            assert_eq!(fact.max_input_tokens, Some(800));
            assert_eq!(fact.remaining_tokens, remaining);
            assert_eq!(fact.overage_tokens, overage);
            assert_eq!(fact.requires_dispatch_refusal(), overage > 0);
        }
        Ok(())
    }

    #[test]
    fn input_ceiling_forecast_never_authorizes_refusal() -> Result<(), Box<dyn std::error::Error>> {
        let profile = profile_witness_with_input_limit(Some(10_000), Some(100))?;
        let fact =
            context_budget_fact_for_session(&session_with_system_bytes(2_000), &[], 1, &profile)?;
        assert!(fact.estimated_total_tokens < u64::from(fact.context_window_tokens));
        assert!(fact.effective_input_tokens() > 100);
        assert_eq!(fact.state, ContextBudgetState::ForecastExceeded);
        assert_eq!(fact.remaining_tokens, 0);
        assert_eq!(fact.overage_tokens, fact.effective_input_tokens() - 100);
        assert!(!fact.requires_dispatch_refusal());
        assert_eq!(
            serde_json::from_value::<ContextBudgetFact>(serde_json::to_value(&fact)?)?,
            fact
        );
        Ok(())
    }

    #[test]
    fn absent_input_ceiling_preserves_shared_window_budget_and_wire_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let profile = profile_witness(Some(1_000))?;
        let fact = context_budget_fact_for_provider_request(
            &[],
            &[],
            100,
            &profile,
            ProviderRequestPressure::new(1, None).with_provider_issued_input_tokens(850),
        )?;
        assert_eq!(fact.state, ContextBudgetState::Within);
        assert_eq!(fact.remaining_tokens, 50);
        let wire = serde_json::to_value(&fact)?;
        assert!(wire.get("max_input_tokens").is_none());
        assert_eq!(serde_json::from_value::<ContextBudgetFact>(wire)?, fact);
        Ok(())
    }
}
