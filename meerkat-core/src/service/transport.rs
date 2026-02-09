//! Transport error mapping for `SessionError`.
//!
//! Each surface has a different wire format. These functions provide
//! standardized mapping from `SessionError` to surface-specific codes.

use super::SessionError;

/// JSON-RPC error code for a `SessionError`.
pub fn jsonrpc_code(err: &SessionError) -> i64 {
    match err {
        SessionError::NotFound { .. } => -32001,
        SessionError::Busy { .. } => -32002,
        SessionError::PersistenceDisabled => -32003,
        SessionError::CompactionDisabled => -32004,
        SessionError::NotRunning { .. } => -32005,
        SessionError::Agent(_) => -32000,
    }
}

/// HTTP status code for a `SessionError`.
pub fn http_status(err: &SessionError) -> u16 {
    match err {
        SessionError::NotFound { .. } => 404,
        SessionError::Busy { .. } => 409,
        SessionError::PersistenceDisabled => 501,
        SessionError::CompactionDisabled => 501,
        SessionError::NotRunning { .. } => 409,
        SessionError::Agent(_) => 500,
    }
}

/// CLI exit code for a `SessionError` (0 = success, 1 = error).
pub fn cli_exit_code(err: &SessionError) -> i32 {
    match err {
        SessionError::PersistenceDisabled | SessionError::CompactionDisabled => {
            // These are informational — stderr message, not hard failure
            0
        }
        _ => 1,
    }
}
