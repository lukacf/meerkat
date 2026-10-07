//! Observations at the actual operation boundary, never execution authority.

use serde::{Deserialize, Serialize};

use super::OperationRefusalKind;
use crate::ops::{AsyncOpRef, ToolDispatchOutcome, ToolDispatchTerminalErrorKind};

/// Stage whose observation could not be retained after the real operation.
/// A diagnostic never changes the operation's returned result or permits retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OperationObservationPhase {
    Outcome,
}

/// What the adapter actually observed. A returned dispatch may have started
/// deferred work; it is not proof that every physical effect completed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationObservedOutcome {
    ToolDispatchReturned {
        asynchronous_operations: Vec<AsyncOpRef>,
        result_is_error: bool,
        terminal_error: Option<ToolDispatchTerminalErrorKind>,
    },
    /// The call returned an error. Its physical effect remains unknown unless
    /// a separate actual operation owner supplies a stronger observation.
    ToolDispatchError {
        error: ToolDispatchTerminalErrorKind,
    },
    /// The transport returned headers, not a complete model response.
    HttpResponse { status: u16 },
    /// A send/receive failure does not prove that the remote request did not run.
    TransportError,
}

impl OperationObservedOutcome {
    #[must_use]
    pub fn from_tool_dispatch(result: &Result<ToolDispatchOutcome, crate::ToolError>) -> Self {
        match result {
            Ok(outcome) => Self::ToolDispatchReturned {
                asynchronous_operations: outcome.async_ops.clone(),
                result_is_error: outcome.result.is_error,
                terminal_error: outcome.terminal_cause().map(|cause| cause.kind()),
            },
            Err(error) => Self::ToolDispatchError {
                error: error.into(),
            },
        }
    }
}

impl std::fmt::Debug for OperationObservedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OperationObservedOutcome([REDACTED])")
    }
}

/// The exact operation owner supplies these observations. They do not advance
/// any run, grant, tool, or input lifecycle and cannot be replayed as commands.
#[derive(Clone)]
pub enum OperationObservation {
    Entry,
    Outcome(OperationObservedOutcome),
    Refused(OperationRefusalKind),
    AuthorizationUnavailable,
}

impl std::fmt::Debug for OperationObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OperationObservation([REDACTED])")
    }
}

/// Known failure to stage an authoritative observation. Before entry this
/// prevents entry and is reported as infrastructure failure. After entry it
/// accompanies the original result; callers must not relabel a returned result
/// or retry its effect because of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("operation observation unavailable")]
pub struct OperationObservationError;
