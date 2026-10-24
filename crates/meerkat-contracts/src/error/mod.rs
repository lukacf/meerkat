//! Typed error envelope for all Meerkat protocol surfaces.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::capability::CapabilityId;

/// Stable error codes for wire protocol.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum::EnumString,
    strum::Display,
    strum::EnumIter,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    SessionNotFound,
    ScheduleNotFound,
    SessionBusy,
    SessionNotRunning,
    RequestCancelled,
    ProviderError,
    BudgetExhausted,
    HookDenied,
    AgentError,
    CapabilityUnavailable,
    SkillNotFound,
    SkillResolutionFailed,
    InvalidParams,
    InternalError,
    DuplicateInput,
    SupervisorRotationIncomplete,
    // Multi-host mobs (§17.4 allocation table; A15 corrected to match it in
    // the v4.1.1 errata) — exactly four console-facing codes.
    /// Plane-(b) control-scope denial; wire detail carries
    /// `{ required, presented }`.
    ScopeDenied,
    /// Member host unreachable/timeout; observation degrades typed, never
    /// quiet.
    HostUnavailable,
    /// Event cursor overran an ephemeral host's buffer; reply carries the
    /// current watermark.
    StaleCursor,
    /// Command carried a superseded `(generation, fence)` tuple.
    StaleFence,
    /// A mob member's runtime registration is durability-degraded and
    /// refuses ordinary work until an explicit registration-authorized cold
    /// reload replaces it (`MobError::MemberReloadRequired`). Deliberately
    /// its own code rather than [`Self::SessionBusy`]: the condition never
    /// clears on its own, so ordinary busy/backoff retry is never correct —
    /// only the named `reload_member_registration` repair resolves it.
    MemberReloadRequired,
    /// The native owner declined an unaccepted input. Details retain the
    /// canonical refusal kind without private policy or principal data.
    InputRefused,
    /// Current native input readiness is unavailable. This is an operational
    /// condition, not a permission verdict or a terminal result for prior work.
    InputNotReady,
    /// Runtime authority is unavailable before a session operation starts.
    SessionRuntimeUnavailable,
    /// The approval store committed nothing (unavailable or contended). An
    /// explicitly attempted retry through fresh checks is valid.
    ApprovalStoreUnavailable,
    /// Another owner of the approval store changed the approval first; it
    /// is reconciled before reuse.
    ApprovalConflict,
    /// The approval store cannot confirm whether the commit happened; the
    /// approval is reconciled from the store before reuse, never resent.
    ApprovalCommitUnknown,
    /// A scope-bound mob submit named a member session that is no longer the
    /// member's current session binding: the session rotated under a still
    /// current runtime incarnation and fence. Distinct from
    /// [`Self::StaleFence`] (a superseded incarnation): the recovery is to
    /// re-capture the delivery scope, not to re-resolve the member.
    StaleDeliveryScope,
    /// A durable fork was refused before commit because of the activation
    /// rows its child would inherit, under the host's fork instruction
    /// policy. No child exists; the typed cause and keys ride in the error
    /// data. A fork request may set `inherited_instructions` to
    /// `reactivate_inherited` to carry the source's effective activations.
    ForkInstructionRefused,
}

impl ErrorCode {
    /// Map to JSON-RPC error code.
    pub const fn jsonrpc_code(self) -> i32 {
        match self {
            Self::SessionNotFound => -32001,
            Self::ScheduleNotFound => -32023,
            Self::SessionBusy => -32002,
            Self::SessionNotRunning => -32003,
            Self::RequestCancelled => -32005,
            Self::ProviderError => -32010,
            Self::BudgetExhausted => -32011,
            Self::HookDenied => -32012,
            Self::AgentError => -32013,
            Self::CapabilityUnavailable => -32020,
            Self::SkillNotFound => -32021,
            Self::SkillResolutionFailed => -32022,
            Self::InvalidParams => -32602,
            Self::InternalError => -32603,
            Self::DuplicateInput => -32004,
            Self::SupervisorRotationIncomplete => -32024,
            Self::ScopeDenied => -32025,
            Self::HostUnavailable => -32026,
            Self::StaleCursor => -32027,
            Self::StaleFence => -32028,
            Self::MemberReloadRequired => -32029,
            Self::InputRefused => -32030,
            Self::InputNotReady => -32031,
            Self::SessionRuntimeUnavailable => -32032,
            Self::ApprovalStoreUnavailable => -32033,
            Self::ApprovalConflict => -32034,
            Self::ApprovalCommitUnknown => -32035,
            Self::StaleDeliveryScope => -32036,
            Self::ForkInstructionRefused => -32037,
        }
    }

    /// Convert a JSON-RPC error code back to the canonical wire code.
    pub const fn from_jsonrpc_code(code: i32) -> Option<Self> {
        match code {
            -32001 => Some(Self::SessionNotFound),
            -32023 => Some(Self::ScheduleNotFound),
            -32002 => Some(Self::SessionBusy),
            -32003 => Some(Self::SessionNotRunning),
            -32005 => Some(Self::RequestCancelled),
            -32010 => Some(Self::ProviderError),
            -32011 => Some(Self::BudgetExhausted),
            -32012 => Some(Self::HookDenied),
            -32013 => Some(Self::AgentError),
            -32020 => Some(Self::CapabilityUnavailable),
            -32021 => Some(Self::SkillNotFound),
            -32022 => Some(Self::SkillResolutionFailed),
            -32602 => Some(Self::InvalidParams),
            -32603 => Some(Self::InternalError),
            -32004 => Some(Self::DuplicateInput),
            -32024 => Some(Self::SupervisorRotationIncomplete),
            -32025 => Some(Self::ScopeDenied),
            -32026 => Some(Self::HostUnavailable),
            -32027 => Some(Self::StaleCursor),
            -32028 => Some(Self::StaleFence),
            -32029 => Some(Self::MemberReloadRequired),
            -32030 => Some(Self::InputRefused),
            -32031 => Some(Self::InputNotReady),
            -32032 => Some(Self::SessionRuntimeUnavailable),
            -32033 => Some(Self::ApprovalStoreUnavailable),
            -32034 => Some(Self::ApprovalConflict),
            -32035 => Some(Self::ApprovalCommitUnknown),
            -32036 => Some(Self::StaleDeliveryScope),
            -32037 => Some(Self::ForkInstructionRefused),
            _ => None,
        }
    }

    /// Map to HTTP status code.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::SessionNotFound | Self::ScheduleNotFound | Self::SkillNotFound => 404,
            Self::SessionBusy
            | Self::SessionNotRunning
            | Self::DuplicateInput
            | Self::SupervisorRotationIncomplete
            | Self::StaleFence
            | Self::MemberReloadRequired
            | Self::ApprovalConflict
            | Self::StaleDeliveryScope => 409,
            Self::RequestCancelled => 499,
            Self::ProviderError => 502,
            Self::BudgetExhausted => 429,
            Self::HookDenied | Self::ScopeDenied | Self::InputRefused => 403,
            Self::AgentError | Self::InternalError | Self::ApprovalCommitUnknown => 500,
            Self::CapabilityUnavailable => 501,
            Self::SkillResolutionFailed | Self::ForkInstructionRefused => 422,
            Self::InvalidParams => 400,
            Self::HostUnavailable
            | Self::InputNotReady
            | Self::SessionRuntimeUnavailable
            | Self::ApprovalStoreUnavailable => 503,
            Self::StaleCursor => 410,
        }
    }

    /// Map to CLI exit code.
    pub const fn cli_exit_code(self) -> i32 {
        match self {
            Self::SessionNotFound => 10,
            Self::ScheduleNotFound => 43,
            Self::SessionBusy => 11,
            Self::SessionNotRunning => 12,
            Self::RequestCancelled => 14,
            Self::ProviderError => 20,
            Self::BudgetExhausted => 21,
            Self::HookDenied => 22,
            Self::AgentError => 30,
            Self::CapabilityUnavailable => 40,
            Self::SkillNotFound => 41,
            Self::SkillResolutionFailed => 42,
            Self::InvalidParams => 2,
            Self::InternalError => 1,
            Self::DuplicateInput => 13,
            Self::SupervisorRotationIncomplete => 44,
            Self::ScopeDenied => 45,
            Self::HostUnavailable => 46,
            Self::StaleCursor => 47,
            Self::StaleFence => 48,
            Self::MemberReloadRequired => 49,
            Self::InputRefused => 50,
            Self::InputNotReady => 51,
            Self::SessionRuntimeUnavailable => 52,
            Self::ApprovalStoreUnavailable => 53,
            Self::ApprovalConflict => 54,
            Self::ApprovalCommitUnknown => 55,
            Self::StaleDeliveryScope => 56,
            Self::ForkInstructionRefused => 57,
        }
    }
}

/// Error category for grouping.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum::EnumString,
    strum::Display,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ErrorCategory {
    Session,
    Request,
    Provider,
    Budget,
    Hook,
    Agent,
    Capability,
    Skill,
    Validation,
    Internal,
}

impl ErrorCode {
    /// Get the category for this error code.
    pub fn category(self) -> ErrorCategory {
        match self {
            // StaleCursor/StaleFence are conflict-class like SessionBusy;
            // both resolve by re-reading current state and retrying.
            // MemberReloadRequired is session/conflict-class too, but
            // deliberately non-retryable — it shares the category, not the
            // retry semantics.
            Self::SessionNotFound
            | Self::ScheduleNotFound
            | Self::SessionBusy
            | Self::SessionNotRunning
            | Self::DuplicateInput
            | Self::SupervisorRotationIncomplete
            | Self::StaleCursor
            | Self::StaleFence
            | Self::MemberReloadRequired
            | Self::StaleDeliveryScope
            | Self::ForkInstructionRefused => ErrorCategory::Session,
            Self::RequestCancelled => ErrorCategory::Request,
            // Provider is the transient-upstream-failure class (502-family);
            // an unreachable member host is the same retryable class and
            // ErrorCategory has no host-specific grouping.
            Self::ProviderError | Self::HostUnavailable => ErrorCategory::Provider,
            Self::BudgetExhausted => ErrorCategory::Budget,
            // Hook is the existing 403 permission-denial class.
            Self::HookDenied | Self::ScopeDenied | Self::InputRefused => ErrorCategory::Hook,
            Self::AgentError => ErrorCategory::Agent,
            Self::CapabilityUnavailable | Self::InputNotReady | Self::SessionRuntimeUnavailable => {
                ErrorCategory::Capability
            }
            Self::SkillNotFound | Self::SkillResolutionFailed => ErrorCategory::Skill,
            Self::InvalidParams => ErrorCategory::Validation,
            // Approval store outcomes: an unavailable store is capability
            // unavailability; a conflict is conflict-class like a stale
            // fence; an unknown commit is internal until reconciled.
            Self::ApprovalStoreUnavailable => ErrorCategory::Capability,
            Self::ApprovalConflict => ErrorCategory::Session,
            Self::InternalError | Self::ApprovalCommitUnknown => ErrorCategory::Internal,
        }
    }
}

/// Hint about which capability is needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CapabilityHint {
    pub capability_id: CapabilityId,
    pub message: Cow<'static, str>,
}

/// Canonical wire error envelope.
///
/// Surfaces map this to their native format (RPC error, HTTP response, CLI exit).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireError {
    pub code: ErrorCode,
    pub category: ErrorCategory,
    pub message: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_hint: Option<CapabilityHint>,
}

impl WireError {
    /// Create a simple error with just a code and message.
    pub fn new(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            category: code.category(),
            code,
            message: message.into(),
            details: None,
            capability_hint: None,
        }
    }

    /// Add a capability hint to this error.
    pub fn with_capability_hint(mut self, hint: CapabilityHint) -> Self {
        self.capability_hint = Some(hint);
        self
    }

    /// Add details to this error.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// Audience-safe session details, including typed runtime readiness.
pub fn session_error_details(error: &meerkat_core::SessionError) -> Option<serde_json::Value> {
    match error {
        meerkat_core::SessionError::WithSettlementFailures { error: primary, .. } => {
            let mut details = match session_error_details(primary) {
                Some(serde_json::Value::Object(data)) => data,
                Some(value) => serde_json::Map::from_iter([("primary_data".into(), value)]),
                None => serde_json::Map::new(),
            };
            details.insert(
                "settlement_failures".into(),
                serde_json::json!(error.settlement_failures().collect::<Vec<_>>()),
            );
            Some(serde_json::Value::Object(details))
        }
        meerkat_core::SessionError::RuntimeUnavailable { reason } => Some(serde_json::json!({
            "code": error.code(),
            "reason": crate::wire::WireControllerReadinessFailure::from(*reason),
        })),
        _ => error.structured_data(),
    }
}

fn session_error_code(err: &meerkat_core::SessionError) -> ErrorCode {
    match err {
        meerkat_core::SessionError::WithSettlementFailures { error, .. } => {
            session_error_code(error)
        }
        meerkat_core::SessionError::RuntimeUnavailable { .. }
        | meerkat_core::SessionError::HostingUnavailable { .. } => {
            ErrorCode::SessionRuntimeUnavailable
        }
        meerkat_core::SessionError::NotFound { .. } => ErrorCode::SessionNotFound,
        meerkat_core::SessionError::Busy { .. }
        | meerkat_core::SessionError::ServedElsewhere { .. } => ErrorCode::SessionBusy,
        meerkat_core::SessionError::NotRunning { .. } => ErrorCode::SessionNotRunning,
        meerkat_core::SessionError::Agent(error) => match error.primary_error() {
            meerkat_core::AgentError::Cancelled => ErrorCode::RequestCancelled,
            meerkat_core::AgentError::SkillResolutionFailed { .. } => {
                ErrorCode::SkillResolutionFailed
            }
            _ => ErrorCode::AgentError,
        },
        meerkat_core::SessionError::CapabilityUnavailable(_)
        | meerkat_core::SessionError::PersistenceDisabled
        | meerkat_core::SessionError::CompactionDisabled
        | meerkat_core::SessionError::Unsupported(_) => ErrorCode::CapabilityUnavailable,
        meerkat_core::SessionError::DurableTailHeldForRecovery { .. }
        | meerkat_core::SessionError::DurableTailRecoveryRefused { .. }
        | meerkat_core::SessionError::DurableEvidenceQuarantined { .. }
        | meerkat_core::SessionError::WholeBlobAuditedEndpointDivergence { .. } => {
            ErrorCode::SessionNotRunning
        }
        meerkat_core::SessionError::ExternalWriteFenceConflict { .. } => ErrorCode::StaleFence,
        meerkat_core::SessionError::ExternalWriteFenceBackoff { .. } => ErrorCode::SessionBusy,
        meerkat_core::SessionError::FailedWithData { .. }
            if err.is_fork_source_build_state_missing() =>
        {
            ErrorCode::InvalidParams
        }
        // A session-scoped surface refused to restore a mob member's
        // session: the caller addressed the wrong surface.
        meerkat_core::SessionError::FailedWithData { .. }
            if err.is_mob_member_session_addressed_by_id() =>
        {
            ErrorCode::InvalidParams
        }
        meerkat_core::SessionError::ForkInstructionRefused(_) => ErrorCode::ForkInstructionRefused,
        meerkat_core::SessionError::Store(_)
        | meerkat_core::SessionError::FailedWithData { .. } => ErrorCode::InternalError,
    }
}

/// The [`ErrorCode`] a session-facing transport reports for a
/// [`meerkat_core::SessionError`]: the JSON-RPC error code of a session
/// method and the `rkat` exit status ([`ErrorCode::cli_exit_code`]).
///
/// This is the session transport mapping. It is not the [`WireError`]
/// envelope view built by `From<SessionError>`, which classifies some
/// variants differently.
pub fn session_transport_error_code(error: &meerkat_core::SessionError) -> ErrorCode {
    use meerkat_core::SessionError;
    match error.primary_error() {
        SessionError::RuntimeUnavailable { .. } | SessionError::HostingUnavailable { .. } => {
            ErrorCode::SessionRuntimeUnavailable
        }
        SessionError::NotFound { .. } => ErrorCode::SessionNotFound,
        // Another runtime owner hosts the session (#1813): the busy class.
        SessionError::Busy { .. } | SessionError::ServedElsewhere { .. } => ErrorCode::SessionBusy,
        SessionError::FailedWithData { .. } if error.is_runtime_teardown_in_progress() => {
            ErrorCode::SessionBusy
        }
        // A fork refused because its source cannot give a runnable child
        // refuses the caller's request; it is not a service fault.
        SessionError::FailedWithData { .. } if error.is_fork_source_build_state_missing() => {
            ErrorCode::InvalidParams
        }
        // A session-scoped surface refused to restore a mob member's session:
        // the caller addressed the wrong surface; the member's mob owns it.
        SessionError::FailedWithData { .. } if error.is_mob_member_session_addressed_by_id() => {
            ErrorCode::InvalidParams
        }
        // A durable resume hold is not a service fault: the session exists,
        // its content is retained intact, and it is not runnable until
        // reconciliation promotes or repairs it.
        SessionError::DurableTailHeldForRecovery { .. }
        | SessionError::DurableTailRecoveryRefused { .. }
        | SessionError::DurableEvidenceQuarantined { .. }
        | SessionError::WholeBlobAuditedEndpointDivergence { .. } => ErrorCode::SessionNotRunning,
        // A fork refused by the host's fork instruction policy: no child exists.
        SessionError::ForkInstructionRefused(_) => ErrorCode::ForkInstructionRefused,
        SessionError::Agent(error) => agent_transport_error_code(error),
        _ => ErrorCode::InternalError,
    }
}

/// The [`ErrorCode`] a session-facing transport reports for an
/// [`meerkat_core::AgentError`], by typed cause (see
/// [`session_transport_error_code`]).
pub fn agent_transport_error_code(error: &meerkat_core::AgentError) -> ErrorCode {
    use meerkat_core::AgentError;
    match error.primary_error() {
        AgentError::TokenBudgetExceeded { .. }
        | AgentError::TimeBudgetExceeded { .. }
        | AgentError::ToolCallBudgetExceeded { .. } => ErrorCode::BudgetExhausted,
        // A turn refused by its instruction activation history (a fork child
        // that never activated its inherited keys) keeps the code its former
        // configuration-string form had. A directed turn refused while a
        // staged create-time prompt is pending is refused the same way.
        AgentError::ConfigError(_)
        | AgentError::InstructionActivation(_)
        | AgentError::StagedPromptDirectedTurnRefused => ErrorCode::InvalidParams,
        AgentError::HookDenied { .. } => ErrorCode::HookDenied,
        AgentError::Llm { .. } | AgentError::BuildError(_) => ErrorCode::ProviderError,
        AgentError::SessionNotFound(_) => ErrorCode::SessionNotFound,
        AgentError::Cancelled => ErrorCode::RequestCancelled,
        AgentError::SkillResolutionFailed { .. } => ErrorCode::SkillResolutionFailed,
        _ => ErrorCode::InternalError,
    }
}

/// Convert from [`meerkat_core::SessionError`] to [`WireError`].
impl From<meerkat_core::SessionError> for WireError {
    fn from(err: meerkat_core::SessionError) -> Self {
        let code = session_error_code(&err);
        let details = session_error_details(&err);
        let wire = WireError::new(code, err.to_string());
        if let Some(details) = details {
            wire.with_details(details)
        } else {
            wire
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// A turn refused by instruction activation history is a typed agent
    /// error; on the wire it keeps the code its configuration-string form had.
    #[test]
    fn a_typed_instruction_activation_refusal_keeps_its_wire_code() {
        let error = meerkat_core::AgentError::InstructionActivation(
            meerkat_core::InstructionActivationError::InheritedActivationRequiresExplicitActivation {
                keys: Vec::new(),
            },
        );
        assert_eq!(agent_transport_error_code(&error), ErrorCode::InvalidParams);
    }

    #[test]
    fn settlement_companions_preserve_wire_class_and_readiness_details() {
        let marker = meerkat_core::ToolDispatchSettlementFailure {
            admission_source: meerkat_core::ToolDispatchAdmissionSource::ConfiguredGate,
            effect_kind: meerkat_core::LiveBridgeEffectKind::ToolDispatch,
            physical_outcome: meerkat_core::LiveBridgeEffectOutcome::Unknown,
            failure_kind: meerkat_core::ToolDispatchTerminalErrorKind::ExecutionFailed,
        };
        for (error, code) in [
            (
                meerkat_core::SessionError::NotFound {
                    id: meerkat_core::SessionId::new(),
                },
                ErrorCode::SessionNotFound,
            ),
            (
                meerkat_core::SessionError::Store(Box::new(std::io::Error::other("store failed"))),
                ErrorCode::InternalError,
            ),
            (
                meerkat_core::SessionError::RuntimeUnavailable {
                    reason:
                        meerkat_core::authorization::ControllerReadinessFailure::AuthorityChanged,
                },
                ErrorCode::SessionRuntimeUnavailable,
            ),
        ] {
            let wire = WireError::from(error.with_settlement_failures(vec![marker.clone()]));
            assert_eq!(wire.code, code);
            let data = wire.details.unwrap();
            assert_eq!(data["settlement_failures"], serde_json::json!([marker]));
            if code == ErrorCode::SessionRuntimeUnavailable {
                assert_eq!(data["reason"]["kind"], "authority_changed");
            }
        }
    }

    #[test]
    fn runtime_readiness_keeps_exact_safe_reason_and_distinct_code() {
        let wire = WireError::from(meerkat_core::SessionError::RuntimeUnavailable {
            reason: meerkat_core::authorization::ControllerReadinessFailure::AuthorityChanged,
        });
        assert_eq!(wire.code, ErrorCode::SessionRuntimeUnavailable);
        assert_eq!(wire.code.jsonrpc_code(), -32032);
        assert_eq!(ErrorCode::from_jsonrpc_code(-32032), Some(wire.code));
        assert_eq!(wire.code.http_status(), 503);
        assert_eq!(wire.code.cli_exit_code(), 52);
        assert_eq!(
            wire.details,
            Some(serde_json::json!({
                "code": "SESSION_RUNTIME_UNAVAILABLE",
                "reason": { "kind": "authority_changed" },
            }))
        );
    }

    /// #1813: a session another runtime owner hosts is refused under the
    /// session-busy code, and its typed kind travels in the details.
    #[test]
    fn served_elsewhere_is_session_busy_with_its_typed_kind() {
        let id = meerkat_core::SessionId::new();
        let wire = WireError::from(meerkat_core::SessionError::ServedElsewhere { id: id.clone() });
        assert_eq!(wire.code, ErrorCode::SessionBusy);
        assert_eq!(
            wire.details,
            Some(serde_json::json!({
                "kind": "session_served_elsewhere",
                "session_id": id.to_string(),
            }))
        );
    }

    /// #1813: a session whose cross-process hosting claim is unavailable is
    /// refused under the runtime-unavailable code with its typed kind, and
    /// no local cause (a path) crosses the wire.
    #[test]
    fn hosting_unavailable_is_runtime_unavailable_with_its_typed_kind() {
        let id = meerkat_core::SessionId::new();
        let wire =
            WireError::from(meerkat_core::SessionError::HostingUnavailable { id: id.clone() });
        assert_eq!(wire.code, ErrorCode::SessionRuntimeUnavailable);
        assert_eq!(wire.code.jsonrpc_code(), -32032);
        assert_eq!(wire.code.http_status(), 503);
        assert_eq!(
            wire.details,
            Some(serde_json::json!({
                "kind": "session_hosting_unavailable",
                "session_id": id.to_string(),
            }))
        );
    }

    #[test]
    fn test_error_code_roundtrip() {
        let codes = [
            ErrorCode::SessionNotFound,
            ErrorCode::ScheduleNotFound,
            ErrorCode::SessionBusy,
            ErrorCode::ProviderError,
            ErrorCode::InternalError,
            ErrorCode::SkillNotFound,
            ErrorCode::RequestCancelled,
            ErrorCode::SessionRuntimeUnavailable,
        ];
        for code in codes {
            let json = serde_json::to_string(&code).unwrap_or_default();
            let parsed: ErrorCode = serde_json::from_str(&json).unwrap_or(ErrorCode::InternalError);
            assert_eq!(code, parsed);
        }
    }

    #[test]
    fn test_wire_error_serialization() {
        let err = WireError::new(ErrorCode::SessionNotFound, "session not found");
        let json = serde_json::to_value(&err).unwrap_or_default();
        assert_eq!(json["code"], "SESSION_NOT_FOUND");
        assert_eq!(json["category"], "session");
    }

    #[test]
    fn session_cancelled_wire_error_uses_request_cancelled_code() {
        let err = WireError::from(meerkat_core::SessionError::Agent(
            meerkat_core::AgentError::Cancelled,
        ));

        assert_eq!(err.code, ErrorCode::RequestCancelled);
        assert_eq!(err.category, ErrorCategory::Request);
    }

    #[test]
    fn session_skill_resolution_wire_error_uses_skill_resolution_code()
    -> Result<(), Box<dyn std::error::Error>> {
        let skill_name = meerkat_core::skills::SkillName::parse("example")?;
        let key = meerkat_core::skills::SkillKey::builtin(skill_name);
        let err = WireError::from(meerkat_core::SessionError::Agent(
            meerkat_core::AgentError::SkillResolutionFailed {
                skill_key: Some(key.clone()),
                reason: Box::new(
                    meerkat_core::event::SkillResolutionFailureReason::NotFound { key },
                ),
            },
        ));

        assert_eq!(err.code, ErrorCode::SkillResolutionFailed);
        assert_eq!(err.category, ErrorCategory::Skill);
        Ok(())
    }

    #[test]
    fn session_failed_with_data_wire_error_preserves_details() {
        let details = serde_json::json!({
            "code": "mob_destroy_incomplete",
            "retryable": true,
            "destroy_report": {
                "errors": ["forced partial cleanup"]
            }
        });
        let err = WireError::from(meerkat_core::SessionError::FailedWithData {
            message: "mob cleanup incomplete".to_string(),
            data: details.clone(),
        });

        assert_eq!(err.code, ErrorCode::InternalError);
        assert_eq!(err.details, Some(details));
    }

    /// A fork refused because its source has no build state answers the
    /// invalid-params class on the session transports and in the wire
    /// envelope, with the refusal's data intact.
    #[test]
    fn fork_source_build_state_missing_answers_invalid_params_with_its_data() {
        let source = meerkat_core::SessionId::new();
        let refusal = meerkat_core::SessionError::fork_source_build_state_missing(&source);
        assert_eq!(
            session_transport_error_code(&refusal),
            ErrorCode::InvalidParams
        );
        assert_eq!(ErrorCode::InvalidParams.jsonrpc_code(), -32602);
        let wire = WireError::from(refusal);
        assert_eq!(wire.code, ErrorCode::InvalidParams);
        assert_eq!(
            wire.details,
            Some(serde_json::json!({
                "kind": "fork_source_build_state_missing",
                "source_session_id": source.to_string(),
            }))
        );
    }

    #[test]
    fn test_error_code_projections() {
        use strum::IntoEnumIterator;

        // Every code (exhaustively via EnumIter) has valid projections.
        for code in ErrorCode::iter() {
            let rpc = code.jsonrpc_code();
            let http = code.http_status();
            let cli = code.cli_exit_code();
            assert_eq!(ErrorCode::from_jsonrpc_code(rpc), Some(code));
            assert!(
                (400..600).contains(&http),
                "HTTP status should be 4xx or 5xx"
            );
            assert!(cli > 0, "CLI exit code should be positive");
        }
    }

    /// The session transport mapping (JSON-RPC session methods and the
    /// `rkat` exit status): one representative per `SessionError` class and
    /// per typed `AgentError` cause, through a settlement wrapper too.
    #[test]
    fn session_transport_error_code_classifies_each_typed_cause() {
        use meerkat_core::{AgentError, SessionError, SessionId};

        let id = SessionId::new();
        let cases = [
            (
                SessionError::NotFound { id: id.clone() },
                ErrorCode::SessionNotFound,
            ),
            (
                SessionError::Busy { id: id.clone() },
                ErrorCode::SessionBusy,
            ),
            (
                SessionError::ServedElsewhere { id: id.clone() },
                ErrorCode::SessionBusy,
            ),
            (
                SessionError::HostingUnavailable { id: id.clone() },
                ErrorCode::SessionRuntimeUnavailable,
            ),
            (
                SessionError::RuntimeUnavailable {
                    reason:
                        meerkat_core::authorization::ControllerReadinessFailure::AuthorityChanged,
                },
                ErrorCode::SessionRuntimeUnavailable,
            ),
            (
                SessionError::NotRunning { id: id.clone() },
                ErrorCode::InternalError,
            ),
            (
                SessionError::DurableTailHeldForRecovery { id: id.clone() },
                ErrorCode::SessionNotRunning,
            ),
            (
                SessionError::Store(Box::new(std::io::Error::other("store failed"))),
                ErrorCode::InternalError,
            ),
            (
                SessionError::Agent(AgentError::TokenBudgetExceeded { used: 2, limit: 1 }),
                ErrorCode::BudgetExhausted,
            ),
            (
                SessionError::Agent(AgentError::TimeBudgetExceeded {
                    elapsed_secs: 2,
                    limit_secs: 1,
                }),
                ErrorCode::BudgetExhausted,
            ),
            (
                SessionError::Agent(AgentError::ToolCallBudgetExceeded { count: 2, limit: 1 }),
                ErrorCode::BudgetExhausted,
            ),
            (
                SessionError::Agent(AgentError::MaxTurnsReached { turns: 3 }),
                ErrorCode::InternalError,
            ),
            (
                SessionError::Agent(AgentError::ConfigError("bad config".into())),
                ErrorCode::InvalidParams,
            ),
            (
                SessionError::Agent(AgentError::BuildError("no provider key".into())),
                ErrorCode::ProviderError,
            ),
            (
                SessionError::Agent(AgentError::SessionNotFound(id.clone())),
                ErrorCode::SessionNotFound,
            ),
            (
                SessionError::Agent(AgentError::Cancelled),
                ErrorCode::RequestCancelled,
            ),
            (
                SessionError::Agent(AgentError::InternalError("internal".into())),
                ErrorCode::InternalError,
            ),
        ];
        for (error, code) in cases {
            assert_eq!(session_transport_error_code(&error), code, "{error}");
            if let SessionError::Agent(agent) = &error {
                assert_eq!(agent_transport_error_code(agent), code, "{agent}");
            }
        }

        let marker = meerkat_core::ToolDispatchSettlementFailure {
            admission_source: meerkat_core::ToolDispatchAdmissionSource::ConfiguredGate,
            effect_kind: meerkat_core::LiveBridgeEffectKind::ToolDispatch,
            physical_outcome: meerkat_core::LiveBridgeEffectOutcome::Unknown,
            failure_kind: meerkat_core::ToolDispatchTerminalErrorKind::ExecutionFailed,
        };
        let wrapped = SessionError::NotFound { id }.with_settlement_failures(vec![marker]);
        assert_eq!(
            session_transport_error_code(&wrapped),
            ErrorCode::SessionNotFound
        );
    }

    /// #1105: `MemberReloadRequired` gets its own dedicated code — exact
    /// value pins plus a proof that it is neither `SessionBusy` nor one of
    /// the four multi-host codes, and that `SessionBusy`'s own projections
    /// are unchanged by the addition.
    #[test]
    fn member_reload_required_renders_exact_values_distinct_from_session_busy() {
        assert_eq!(ErrorCode::MemberReloadRequired.jsonrpc_code(), -32029);
        assert_eq!(ErrorCode::MemberReloadRequired.http_status(), 409);
        assert_eq!(ErrorCode::MemberReloadRequired.cli_exit_code(), 49);
        assert_eq!(
            ErrorCode::MemberReloadRequired.category(),
            ErrorCategory::Session
        );
        assert_eq!(
            ErrorCode::from_jsonrpc_code(-32029),
            Some(ErrorCode::MemberReloadRequired)
        );
        assert_ne!(ErrorCode::MemberReloadRequired, ErrorCode::SessionBusy);

        // SessionBusy's own projections are untouched by the new arm.
        assert_eq!(ErrorCode::SessionBusy.jsonrpc_code(), -32002);
        assert_eq!(ErrorCode::SessionBusy.http_status(), 409);
        assert_eq!(ErrorCode::SessionBusy.cli_exit_code(), 11);
        assert_eq!(ErrorCode::SessionBusy.category(), ErrorCategory::Session);
    }

    /// A15/§17.4 exact-value pins for the four multi-host codes — all four
    /// renderings each, plus JSON-RPC round-trip.
    #[test]
    fn multi_host_error_codes_render_exact_values() {
        let cases: &[(ErrorCode, i32, u16, i32, ErrorCategory)] = &[
            (ErrorCode::ScopeDenied, -32025, 403, 45, ErrorCategory::Hook),
            (
                ErrorCode::HostUnavailable,
                -32026,
                503,
                46,
                ErrorCategory::Provider,
            ),
            (
                ErrorCode::StaleCursor,
                -32027,
                410,
                47,
                ErrorCategory::Session,
            ),
            (
                ErrorCode::StaleFence,
                -32028,
                409,
                48,
                ErrorCategory::Session,
            ),
            (
                ErrorCode::StaleDeliveryScope,
                -32036,
                409,
                56,
                ErrorCategory::Session,
            ),
        ];
        for (code, rpc, http, cli, category) in cases {
            assert_eq!(code.jsonrpc_code(), *rpc, "{code:?} jsonrpc pin");
            assert_eq!(code.http_status(), *http, "{code:?} http pin");
            assert_eq!(code.cli_exit_code(), *cli, "{code:?} cli pin");
            assert_eq!(code.category(), *category, "{code:?} category pin");
            assert_eq!(
                ErrorCode::from_jsonrpc_code(*rpc),
                Some(*code),
                "{code:?} jsonrpc round-trip pin"
            );
        }
    }

    /// Exhaustive uniqueness sweep: no two codes may share a JSON-RPC code
    /// or a CLI exit code — a collision silently mis-routes SDK error
    /// classes.
    #[test]
    fn error_code_projections_are_collision_free() {
        use std::collections::HashMap;
        use strum::IntoEnumIterator;

        let mut jsonrpc: HashMap<i32, ErrorCode> = HashMap::new();
        let mut cli: HashMap<i32, ErrorCode> = HashMap::new();
        for code in ErrorCode::iter() {
            if let Some(previous) = jsonrpc.insert(code.jsonrpc_code(), code) {
                panic!(
                    "JSON-RPC code {} claimed by both {previous:?} and {code:?}",
                    code.jsonrpc_code()
                );
            }
            if let Some(previous) = cli.insert(code.cli_exit_code(), code) {
                panic!(
                    "CLI exit code {} claimed by both {previous:?} and {code:?}",
                    code.cli_exit_code()
                );
            }
        }
    }
}
