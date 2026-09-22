//! Batched semantic decision service for Meerkat.
//!
//! Ordinary code inserts narrow semantic judgments where it needs language
//! understanding: binary predicates, finite choices, and descriptive ordinal
//! grades over supplied state, batched into one bounded call. The judgments
//! are typed data; code composes the applicable answers and keeps exact
//! checks, thresholds, authorization, and effects for itself.
//!
//! Ownership:
//! - this crate owns the typed contracts, validation, the fixed answer
//!   interpretation, the `decide` tool, and its capability declaration;
//! - the session-LLM backend issues one tool-free structured request through
//!   the caller's already-admitted route;
//! - the optional Jev adapter owns that endpoint's transport and native
//!   signals and no application policy;
//! - the facade selects the route, resolves credentials, and injects the tool;
//! - consuming features (memory, work, rubrics) own thresholds and
//!   dispositions.
//!
//! Nothing here runs unless a realm sets `tools.decision_enabled = true`.

#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

pub mod backend;
pub mod contracts;
pub mod error;
#[cfg(all(feature = "jev", not(target_arch = "wasm32")))]
pub mod jev;
pub mod llm_backend;
pub mod service;
pub mod tool;
pub mod validate;

pub use backend::{
    AttemptUsage, BackendResponse, BackendUsage, Deadline, DecisionBackend, FailedEvaluation,
    RawAnswer, RawDistribution, RawGradeDistribution,
};
pub use contracts::{
    BackendKind, BinaryAnswer, BinaryCriteria, BinaryJudgment, BudgetParticipation, ChoiceJudgment,
    ChoiceOption, DECISION_CONTRACT_VERSION, DecisionAccounting, DecisionContractVersion,
    DecisionRequest, DecisionResult, DecisionState, GradeJudgment, GradeLevel, GradeLevelIndex,
    Instructions, InvalidIdentifier, InvalidStateShape, InvalidUnitInterval, Judgment,
    NativeSignal, OptionId, Question, QuestionId, QuestionJudgment, QuestionKind, RouteProvenance,
    UnitInterval,
};
pub use error::{
    AnswerValidationError, BackendFailure, DecisionError, DecisionErrorCode,
    DecisionUnavailableReason, RequestValidationError,
};
#[cfg(all(feature = "jev", not(target_arch = "wasm32")))]
pub use jev::{
    DEFAULT_JEV_ENDPOINT, DEFAULT_JEV_MODEL, JevBackend, JevBackendBuildError, JevBearerSecret,
    JevCredentialError, JevCredentialSource, JevDisclosurePermit, StaticJevCredential,
};
pub use llm_backend::{LLM_ROUTE_SYSTEM_PROMPT, LlmRouteBackend, RouteBinding};
pub use service::{BudgetAdmission, DecisionAdmission, DecisionService, RouteAdmission};
pub use tool::{
    DECIDE_TOOL_NAME, DECISION_TOOL_SOURCE_ID, DecisionToolSurface, decide_tool_input_schema,
    wire_decision_tool,
};
pub use validate::{RESERVED_ABSTAIN_OPTION, ValidatedRequest, validate_answers};

/// Reason reported when the capability is compiled in but switched off.
pub const DECISION_CAPABILITY_DISABLED_DESCRIPTION: &str = "config.tools.decision_enabled is false";

/// Realm policy switch for the decision capability.
pub fn decision_capability_enabled(config: &meerkat_core::Config) -> bool {
    config.tools.decision_enabled
}

pub const DECISION_CAPABILITY_POLICY: meerkat_capabilities::FeatureCapabilityPolicy =
    meerkat_capabilities::FeatureCapabilityPolicy::new(
        decision_capability_enabled,
        DECISION_CAPABILITY_DISABLED_DESCRIPTION,
    );

pub const fn decision_capability_policy() -> meerkat_capabilities::FeatureCapabilityPolicy {
    DECISION_CAPABILITY_POLICY
}

inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::Decision,
        description: "Batched semantic decision service (binary, choose-one, grade judgments)",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: Some(|config| {
            let policy = crate::decision_capability_policy();
            if policy.is_enabled(config) {
                meerkat_capabilities::CapabilityStatus::Available
            } else {
                meerkat_capabilities::CapabilityStatus::DisabledByPolicy {
                    description: policy.disabled_description().into(),
                }
            }
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use meerkat_capabilities::{CapabilityId, CapabilityStatus, resolve_capabilities};
    use meerkat_core::Config;

    #[test]
    fn capability_is_declared_and_disabled_by_default() {
        let config = Config::default();
        let (_, status) = resolve_capabilities(&config)
            .into_iter()
            .find(|(registration, _)| registration.id == CapabilityId::Decision)
            .expect("decision capability is registered");
        assert!(matches!(status, CapabilityStatus::DisabledByPolicy { .. }));

        let mut enabled = config;
        enabled.tools.decision_enabled = true;
        let (_, status) = resolve_capabilities(&enabled)
            .into_iter()
            .find(|(registration, _)| registration.id == CapabilityId::Decision)
            .unwrap();
        assert!(matches!(status, CapabilityStatus::Available));
    }
}
