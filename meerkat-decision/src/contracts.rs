//! Typed decision contracts.
//!
//! Every distinction that matters semantically is a type here: question and
//! answer kinds, identities, abstention, native backend evidence, route
//! provenance, and accounting degradation. Question ids are correlation keys,
//! never instructions. Backends decode evidence into these shapes; the service
//! validates them; consuming features own thresholds and dispositions.

use std::fmt;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Fixed, versioned interpretation contract for answer shapes.
///
/// A judgment's meaning is defined by this crate at this version. A future
/// interpretation change is a new version, never a silent re-reading of old
/// results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DecisionContractVersion {
    V1,
}

/// The contract version this crate implements.
pub const DECISION_CONTRACT_VERSION: DecisionContractVersion = DecisionContractVersion::V1;

const MAX_IDENTIFIER_BYTES: usize = 64;

/// Why an identifier was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InvalidIdentifier {
    #[error("identifier must not be empty")]
    Empty,
    #[error("identifier exceeds 64 UTF-8 bytes")]
    TooLong,
    #[error("identifier may only contain ASCII letters, digits, '_', '-', and '.'")]
    InvalidCharacter,
}

fn validate_identifier(raw: &str) -> Result<(), InvalidIdentifier> {
    if raw.is_empty() {
        return Err(InvalidIdentifier::Empty);
    }
    if raw.len() > MAX_IDENTIFIER_BYTES {
        return Err(InvalidIdentifier::TooLong);
    }
    if !raw
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(InvalidIdentifier::InvalidCharacter);
    }
    Ok(())
}

macro_rules! identifier_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Construct a validated identifier.
            pub fn new(raw: impl Into<String>) -> Result<Self, InvalidIdentifier> {
                let raw = raw.into();
                validate_identifier(&raw)?;
                Ok(Self(raw))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let raw = String::deserialize(deserializer)?;
                Self::new(raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

identifier_newtype!(
    /// Correlation key for one question. It is returned unchanged with the
    /// matching judgment and is never sent to a backend as meaning.
    QuestionId
);

identifier_newtype!(
    /// Identity of one supplied option in a choose-one question.
    OptionId
);

/// Zero-based index into a grade question's ordered levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct GradeLevelIndex(u32);

impl GradeLevelIndex {
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Instruction, criterion, option, or level text.
///
/// Either plain text or bounded structured JSON (an object or array) so a
/// question can carry the data it refers to. This is content the backend
/// evaluates, never semantic authority the service branches on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum Instructions {
    Text(String),
    Structured(Value),
}

impl Instructions {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// Serialized size used for bounding.
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Structured(value) => value.to_string().len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.trim().is_empty(),
            Self::Structured(value) => match value {
                Value::Object(map) => map.is_empty(),
                Value::Array(items) => items.is_empty(),
                Value::String(text) => text.trim().is_empty(),
                Value::Null => true,
                Value::Bool(_) | Value::Number(_) => false,
            },
        }
    }

    /// Whether the structured form has an admissible shape.
    pub fn has_admissible_shape(&self) -> bool {
        match self {
            Self::Text(_) => true,
            Self::Structured(value) => value.is_object() || value.is_array(),
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Text(text) => Value::String(text.clone()),
            Self::Structured(value) => value.clone(),
        }
    }
}

/// Optional descriptions of what a yes and a no mean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BinaryCriteria {
    pub yes: Instructions,
    pub no: Instructions,
}

/// One supplied alternative in a choose-one question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ChoiceOption {
    pub id: OptionId,
    pub description: Instructions,
}

/// One ordered level in a grade question. Each level must carry its complete
/// meaning; the index is a position, not a description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GradeLevel {
    pub description: Instructions,
}

/// Kind of a question or answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Binary,
    ChooseOne,
    Grade,
}

impl QuestionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::ChooseOne => "choose_one",
            Self::Grade => "grade",
        }
    }
}

/// A typed question. Each variant carries its complete meaning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Question {
    /// Whether a specific fact is stated or directly implied by the state.
    Binary {
        id: QuestionId,
        instructions: Instructions,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<BinaryCriteria>,
    },
    /// A relative choice among supplied alternatives. Relative choice is not
    /// absolute sufficiency; pair it with a binary predicate where
    /// sufficiency matters.
    ChooseOne {
        id: QuestionId,
        instructions: Instructions,
        options: Vec<ChoiceOption>,
    },
    /// A descriptive ordinal judgment over self-contained levels.
    Grade {
        id: QuestionId,
        instructions: Instructions,
        levels: Vec<GradeLevel>,
    },
}

impl Question {
    pub fn id(&self) -> &QuestionId {
        match self {
            Self::Binary { id, .. } | Self::ChooseOne { id, .. } | Self::Grade { id, .. } => id,
        }
    }

    pub fn instructions(&self) -> &Instructions {
        match self {
            Self::Binary { instructions, .. }
            | Self::ChooseOne { instructions, .. }
            | Self::Grade { instructions, .. } => instructions,
        }
    }

    pub const fn kind(&self) -> QuestionKind {
        match self {
            Self::Binary { .. } => QuestionKind::Binary,
            Self::ChooseOne { .. } => QuestionKind::ChooseOne,
            Self::Grade { .. } => QuestionKind::Grade,
        }
    }
}

/// Why a state value was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InvalidStateShape {
    #[error("decision state must be a string, object, or array")]
    NotStringObjectOrArray,
}

/// Bounded, host-authorized data the questions are evaluated against.
///
/// The service treats this as data only. Access to it does not authorize
/// disclosure to a new destination; the host grants that separately.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct DecisionState(Value);

impl DecisionState {
    pub fn new(value: Value) -> Result<Self, InvalidStateShape> {
        if value.is_string() || value.is_object() || value.is_array() {
            Ok(Self(value))
        } else {
            Err(InvalidStateShape::NotStringObjectOrArray)
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self(Value::String(text.into()))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Serialized size used for bounding.
    pub fn byte_len(&self) -> usize {
        match &self.0 {
            Value::String(text) => text.len(),
            other => other.to_string().len(),
        }
    }
}

impl<'de> Deserialize<'de> for DecisionState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// One batched evaluation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DecisionRequest {
    /// What the caller is trying to accomplish; context for every question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    pub state: DecisionState,
    pub questions: Vec<Question>,
}

/// Categorical answer to a binary question, including explicit abstention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BinaryAnswer {
    Yes,
    No,
    /// The state does not determine the answer. A valid judgment, not a fault.
    Abstain,
}

/// Why a real-valued signal was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InvalidUnitInterval {
    #[error("value is not finite")]
    NotFinite,
    #[error("value is outside [0, 1]")]
    OutOfRange,
}

/// A finite probability in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct UnitInterval(f64);

impl UnitInterval {
    pub fn new(value: f64) -> Result<Self, InvalidUnitInterval> {
        if !value.is_finite() {
            return Err(InvalidUnitInterval::NotFinite);
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(InvalidUnitInterval::OutOfRange);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for UnitInterval {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Judgment for a binary question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum BinaryJudgment {
    /// The backend supplied a categorical verdict.
    Categorical { answer: BinaryAnswer },
    /// The backend supplied only a native probability that the answer is yes.
    /// No threshold was applied here; the consuming feature owns that policy.
    NativeProbability { yes: UnitInterval },
}

/// Judgment for a choose-one question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum ChoiceJudgment {
    /// The backend selected one of the supplied options.
    Selected { option: OptionId },
    /// The backend declined to select. A valid judgment, not a fault.
    Abstain,
}

/// Judgment for a grade question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum GradeJudgment {
    /// The backend elected one of the supplied levels.
    Level { index: GradeLevelIndex },
    /// The backend declined to grade. A valid judgment, not a fault.
    Abstain,
    /// The backend supplied a probability-weighted ordinal position in
    /// `[0, levels - 1]` without electing a level. No level was elected here.
    NativeWeighted { position: f64 },
}

/// A typed judgment keyed by the question it answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Judgment {
    Binary(BinaryJudgment),
    Choice(ChoiceJudgment),
    Grade(GradeJudgment),
}

impl Judgment {
    pub const fn kind(&self) -> QuestionKind {
        match self {
            Self::Binary(_) => QuestionKind::Binary,
            Self::Choice(_) => QuestionKind::ChooseOne,
            Self::Grade(_) => QuestionKind::Grade,
        }
    }
}

/// Which backend produced a judgment or signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// One bounded structured request through the session's admitted LLM route.
    SessionLlm,
    /// The explicitly configured Jev evaluation endpoint.
    Jev,
}

/// Backend-qualified native evidence retained as data beside the judgment.
///
/// Never synthesized from an ordinary LLM's self-reported confidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum NativeSignal {
    /// Full probability distribution over the supplied options.
    ChoiceDistribution {
        backend: BackendKind,
        probabilities: IndexMap<OptionId, UnitInterval>,
        confidence: UnitInterval,
    },
    /// Probability of each level, by level index.
    GradeDistribution {
        backend: BackendKind,
        probabilities: Vec<UnitInterval>,
        confidence: UnitInterval,
    },
}

/// The judgment for one question plus any native evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct QuestionJudgment {
    pub judgment: Judgment,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_signals: Vec<NativeSignal>,
}

/// Exact route that served an evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum RouteProvenance {
    SessionLlm {
        provider: meerkat_core::Provider,
        model: String,
    },
    Jev {
        endpoint: String,
        requested_model: String,
        served_model: String,
    },
}

impl RouteProvenance {
    pub const fn backend(&self) -> BackendKind {
        match self {
            Self::SessionLlm { .. } => BackendKind::SessionLlm,
            Self::Jev { .. } => BackendKind::Jev,
        }
    }
}

/// Token accounting for one evaluation, or the typed fact that none was
/// measured. Absence is representable and never substituted with zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionAccounting {
    Measured {
        input_tokens: u64,
        output_tokens: u64,
    },
    Unmeasured,
}

/// How this evaluation participated in the caller's aggregate token budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BudgetParticipation {
    /// The owner's aggregate axis was charged exactly this many tokens.
    Charged { tokens: u64 },
    /// The owner issued accounting but nothing was measured; the estimate was
    /// released and the axis did not advance.
    Unmeasured,
    /// The dispatching context issued no accounting handle (standalone or
    /// host invocation). Nothing was charged and nothing was fabricated.
    NotIssued,
}

impl From<meerkat_core::NestedUsageSettlement> for BudgetParticipation {
    fn from(settlement: meerkat_core::NestedUsageSettlement) -> Self {
        match settlement {
            meerkat_core::NestedUsageSettlement::Charged { tokens } => Self::Charged { tokens },
            meerkat_core::NestedUsageSettlement::Unmeasured => Self::Unmeasured,
        }
    }
}

/// Validated result of one batched evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DecisionResult {
    pub contract: DecisionContractVersion,
    pub route: RouteProvenance,
    /// One judgment per question, in request order.
    pub judgments: IndexMap<QuestionId, QuestionJudgment>,
    pub accounting: DecisionAccounting,
    pub budget: BudgetParticipation,
    /// Backend attempts consumed, including bounded format repair.
    pub attempts: u32,
}

impl DecisionResult {
    pub fn judgment(&self, id: &QuestionId) -> Option<&QuestionJudgment> {
        self.judgments.get(id)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_validated_at_construction_and_deserialization() {
        assert!(QuestionId::new("is_urgent").is_ok());
        assert!(QuestionId::new("q.1-a").is_ok());
        assert_eq!(QuestionId::new("").unwrap_err(), InvalidIdentifier::Empty);
        assert_eq!(
            QuestionId::new("has space").unwrap_err(),
            InvalidIdentifier::InvalidCharacter
        );
        assert_eq!(
            QuestionId::new("x".repeat(65)).unwrap_err(),
            InvalidIdentifier::TooLong
        );
        assert!(serde_json::from_str::<OptionId>("\"bad id\"").is_err());
        let parsed: OptionId = serde_json::from_str("\"billing\"").unwrap();
        assert_eq!(parsed.as_str(), "billing");
    }

    #[test]
    fn state_rejects_scalars_other_than_strings() {
        assert!(DecisionState::new(Value::Bool(true)).is_err());
        assert!(DecisionState::new(serde_json::json!(3)).is_err());
        assert!(DecisionState::new(Value::Null).is_err());
        assert!(DecisionState::new(serde_json::json!({"a": 1})).is_ok());
        assert!(serde_json::from_str::<DecisionState>("42").is_err());
    }

    #[test]
    fn unit_interval_rejects_out_of_range_and_non_finite() {
        assert!(UnitInterval::new(0.0).is_ok());
        assert!(UnitInterval::new(1.0).is_ok());
        assert_eq!(
            UnitInterval::new(1.5).unwrap_err(),
            InvalidUnitInterval::OutOfRange
        );
        assert_eq!(
            UnitInterval::new(f64::NAN).unwrap_err(),
            InvalidUnitInterval::NotFinite
        );
        assert!(serde_json::from_str::<UnitInterval>("2.0").is_err());
    }

    #[test]
    fn question_wire_shape_is_kind_tagged() {
        let question: Question = serde_json::from_value(serde_json::json!({
            "kind": "choose_one",
            "id": "department",
            "instructions": "Which team should handle this?",
            "options": [
                {"id": "billing", "description": "Payments, invoicing, refunds"},
                {"id": "technical", "description": {"covers": ["bugs", "outages"]}}
            ]
        }))
        .unwrap();
        assert_eq!(question.kind(), QuestionKind::ChooseOne);
        assert_eq!(question.id().as_str(), "department");
    }

    #[test]
    fn judgments_and_results_round_trip() {
        let mut judgments = IndexMap::new();
        judgments.insert(
            QuestionId::new("is_urgent").unwrap(),
            QuestionJudgment {
                judgment: Judgment::Binary(BinaryJudgment::NativeProbability {
                    yes: UnitInterval::new(0.95).unwrap(),
                }),
                native_signals: Vec::new(),
            },
        );
        let result = DecisionResult {
            contract: DECISION_CONTRACT_VERSION,
            route: RouteProvenance::Jev {
                endpoint: "https://api.typesafe.ai/v1/systemone".into(),
                requested_model: "jev-latest".into(),
                served_model: "jev-1.13.0".into(),
            },
            judgments,
            accounting: DecisionAccounting::Measured {
                input_tokens: 10,
                output_tokens: 2,
            },
            budget: BudgetParticipation::NotIssued,
            attempts: 1,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["judgments"]["is_urgent"]["judgment"]["kind"], "binary");
        assert_eq!(
            json["judgments"]["is_urgent"]["judgment"]["form"],
            "native_probability"
        );
        let parsed: DecisionResult = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, result);
    }
}
