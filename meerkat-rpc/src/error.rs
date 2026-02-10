//! JSON-RPC error codes.
//!
//! Standard JSON-RPC codes are defined here. Application-specific codes
//! delegate to `meerkat_contracts::ErrorCode::jsonrpc_code()` for consistency
//! across all protocol surfaces.

/// Standard JSON-RPC error: invalid JSON
pub const PARSE_ERROR: i32 = -32700;
/// Standard JSON-RPC error: not a valid request object
pub const INVALID_REQUEST: i32 = -32600;
/// Standard JSON-RPC error: method does not exist
pub const METHOD_NOT_FOUND: i32 = -32601;
/// Standard JSON-RPC error: invalid method parameters
pub const INVALID_PARAMS: i32 = -32602;
/// Standard JSON-RPC error: internal error
pub const INTERNAL_ERROR: i32 = -32603;

// Application error codes — delegated to contracts for consistency.

/// Session not found
pub const SESSION_NOT_FOUND: i32 = meerkat_contracts::ErrorCode::SessionNotFound.jsonrpc_code();
/// Session is busy (turn already in progress)
pub const SESSION_BUSY: i32 = meerkat_contracts::ErrorCode::SessionBusy.jsonrpc_code();
/// LLM provider error (missing API key, auth failure, etc.)
pub const PROVIDER_ERROR: i32 = meerkat_contracts::ErrorCode::ProviderError.jsonrpc_code();
/// Budget exhausted
pub const BUDGET_EXHAUSTED: i32 = meerkat_contracts::ErrorCode::BudgetExhausted.jsonrpc_code();
/// Hook denied the operation
pub const HOOK_DENIED: i32 = meerkat_contracts::ErrorCode::HookDenied.jsonrpc_code();
