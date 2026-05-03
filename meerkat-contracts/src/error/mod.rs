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
            _ => None,
        }
    }

    /// Map to HTTP status code.
    pub const fn http_status(self) -> u16 {
        match self {
            Self::SessionNotFound | Self::ScheduleNotFound | Self::SkillNotFound => 404,
            Self::SessionBusy | Self::SessionNotRunning | Self::DuplicateInput => 409,
            Self::RequestCancelled => 499,
            Self::ProviderError => 502,
            Self::BudgetExhausted => 429,
            Self::HookDenied => 403,
            Self::AgentError | Self::InternalError => 500,
            Self::CapabilityUnavailable => 501,
            Self::SkillResolutionFailed => 422,
            Self::InvalidParams => 400,
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
            Self::SessionNotFound
            | Self::ScheduleNotFound
            | Self::SessionBusy
            | Self::SessionNotRunning
            | Self::DuplicateInput => ErrorCategory::Session,
            Self::RequestCancelled => ErrorCategory::Request,
            Self::ProviderError => ErrorCategory::Provider,
            Self::BudgetExhausted => ErrorCategory::Budget,
            Self::HookDenied => ErrorCategory::Hook,
            Self::AgentError => ErrorCategory::Agent,
            Self::CapabilityUnavailable => ErrorCategory::Capability,
            Self::SkillNotFound | Self::SkillResolutionFailed => ErrorCategory::Skill,
            Self::InvalidParams => ErrorCategory::Validation,
            Self::InternalError => ErrorCategory::Internal,
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

/// Convert from [`SessionError`] to [`WireError`].
impl From<meerkat_core::SessionError> for WireError {
    fn from(err: meerkat_core::SessionError) -> Self {
        let code = match &err {
            meerkat_core::SessionError::NotFound { .. } => ErrorCode::SessionNotFound,
            meerkat_core::SessionError::Busy { .. } => ErrorCode::SessionBusy,
            meerkat_core::SessionError::NotRunning { .. } => ErrorCode::SessionNotRunning,
            meerkat_core::SessionError::Agent(meerkat_core::AgentError::Cancelled) => {
                ErrorCode::RequestCancelled
            }
            meerkat_core::SessionError::Agent(_) => ErrorCode::AgentError,
            meerkat_core::SessionError::PersistenceDisabled
            | meerkat_core::SessionError::CompactionDisabled
            | meerkat_core::SessionError::Unsupported(_) => ErrorCode::CapabilityUnavailable,
            meerkat_core::SessionError::Store(_) => ErrorCode::InternalError,
        };
        WireError::new(code, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_error_code_projections() {
        // Every code should have valid projections
        for code in [
            ErrorCode::SessionNotFound,
            ErrorCode::ScheduleNotFound,
            ErrorCode::SessionBusy,
            ErrorCode::SessionNotRunning,
            ErrorCode::RequestCancelled,
            ErrorCode::ProviderError,
            ErrorCode::BudgetExhausted,
            ErrorCode::HookDenied,
            ErrorCode::AgentError,
            ErrorCode::CapabilityUnavailable,
            ErrorCode::SkillNotFound,
            ErrorCode::SkillResolutionFailed,
            ErrorCode::InvalidParams,
            ErrorCode::InternalError,
            ErrorCode::DuplicateInput,
        ] {
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
}
