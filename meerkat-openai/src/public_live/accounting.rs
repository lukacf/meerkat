//! Bounded advisory accounting from the original public response envelope.
//!
//! These observations never classify function readiness or executor success.
//! Missing model evidence is not replaced with the configured bridge model.

use meerkat_core::live_execution::request::{LiveDelegationAttribution, LiveResponseIdentity};
use oai_rt_rs::live::{Field, ServerEvent, ServerFrame};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_MODEL_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendAccountingObservation {
    pub response: LiveResponseIdentity,
    pub model: BackendModelEvidence,
    pub usage: BackendUsageEvidence,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackendModelEvidence {
    Unconfirmed {},
    Reported { model: String },
    Malformed {},
}

impl std::fmt::Debug for BackendModelEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfirmed {} => formatter.write_str("Unconfirmed"),
            Self::Reported { .. } => formatter.write_str("Reported([REDACTED])"),
            Self::Malformed {} => formatter.write_str("Malformed"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackendUsageEvidence {
    Absent {},
    Reported { counters: BackendTokenCounters },
    Malformed { field: BackendAccountingField },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendTokenCounters {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendAccountingField {
    Usage,
    InputTokens,
    OutputTokens,
    TotalTokens,
    InputTokenDetails,
    OutputTokenDetails,
    CachedTokens,
    ReasoningTokens,
}

/// `None` means this frame contains no response-snapshot accounting location.
/// A malformed field at that location returns explicit advisory evidence.
pub fn observe_backend_accounting(
    frame: &ServerFrame,
    response: LiveResponseIdentity,
) -> Result<Option<BackendAccountingObservation>, BackendAccountingIdentityError> {
    let ServerEvent::Response { delegation_id, .. } = &frame.event else {
        return Ok(None);
    };
    let Some(snapshot) = frame
        .raw
        .get("event")
        .and_then(|event| event.get("response"))
    else {
        return Ok(None);
    };
    if snapshot.get("id").and_then(Value::as_str) != Some(response.response.as_str()) {
        return Err(BackendAccountingIdentityError);
    }
    let attribution_matches = match (delegation_id, &response.attribution) {
        (Field::Absent, LiveDelegationAttribution::Absent {})
        | (Field::Null, LiveDelegationAttribution::ExplicitNull {}) => true,
        (Field::Value(actual), LiveDelegationAttribution::Known { delegation }) => {
            actual == delegation.as_str()
        }
        _ => false,
    };
    if !attribution_matches {
        return Err(BackendAccountingIdentityError);
    }
    let model = match snapshot.get("model") {
        None | Some(Value::Null) => BackendModelEvidence::Unconfirmed {},
        Some(Value::String(model))
            if !model.trim().is_empty() && model.len() <= MAX_MODEL_BYTES =>
        {
            BackendModelEvidence::Reported {
                model: model.clone(),
            }
        }
        Some(_) => BackendModelEvidence::Malformed {},
    };
    let usage = match snapshot.get("usage") {
        None | Some(Value::Null) => BackendUsageEvidence::Absent {},
        Some(value) => match token_counters(value) {
            Ok(counters) => BackendUsageEvidence::Reported { counters },
            Err(field) => BackendUsageEvidence::Malformed { field },
        },
    };
    Ok(Some(BackendAccountingObservation {
        response,
        model,
        usage,
    }))
}

fn token_counters(value: &Value) -> Result<BackendTokenCounters, BackendAccountingField> {
    if !value.is_object() {
        return Err(BackendAccountingField::Usage);
    }
    let input_tokens = value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .ok_or(BackendAccountingField::InputTokens)?;
    let output_tokens = value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .ok_or(BackendAccountingField::OutputTokens)?;
    let total_tokens = value
        .get("total_tokens")
        .and_then(Value::as_u64)
        .ok_or(BackendAccountingField::TotalTokens)?;
    if input_tokens.checked_add(output_tokens) != Some(total_tokens) {
        return Err(BackendAccountingField::TotalTokens);
    }
    let cached_input_tokens = detail_counter(
        value.get("input_tokens_details"),
        "cached_tokens",
        BackendAccountingField::InputTokenDetails,
        BackendAccountingField::CachedTokens,
    )?;
    let reasoning_output_tokens = detail_counter(
        value.get("output_tokens_details"),
        "reasoning_tokens",
        BackendAccountingField::OutputTokenDetails,
        BackendAccountingField::ReasoningTokens,
    )?;
    if cached_input_tokens.is_some_and(|cached| cached > input_tokens) {
        return Err(BackendAccountingField::CachedTokens);
    }
    if reasoning_output_tokens.is_some_and(|reasoning| reasoning > output_tokens) {
        return Err(BackendAccountingField::ReasoningTokens);
    }
    Ok(BackendTokenCounters {
        input_tokens,
        output_tokens,
        total_tokens,
        cached_input_tokens,
        reasoning_output_tokens,
    })
}

fn detail_counter(
    value: Option<&Value>,
    name: &str,
    object_error: BackendAccountingField,
    counter_error: BackendAccountingField,
) -> Result<Option<u64>, BackendAccountingField> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(details)) => match details.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(counter) => counter.as_u64().map(Some).ok_or(counter_error),
        },
        Some(_) => Err(object_error),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("live backend accounting response identity does not match its observed scope")]
pub struct BackendAccountingIdentityError;
