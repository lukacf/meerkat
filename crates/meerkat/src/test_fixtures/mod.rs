//! Explicitly-featured test-support fixtures (never production
//! composition). Gated by `test-realtime-fixtures` (ADJ-P6B-4): the
//! deterministic realtime fakes are shared by the facade live-pipeline
//! battery, `crates/meerkat-mob/tests` (which cannot depend on the integration
//! crate — cycle), and the integration member-live rows.

#[cfg(feature = "test-realtime-fixtures")]
pub mod realtime;

/// OAuth-protected MCP server for MCP OAuth canaries
/// (`test-mcp-oauth-fixtures`).
#[cfg(feature = "test-mcp-oauth-fixtures")]
pub mod mcp_oauth;

/// One native auth error per public reason, for the auth surface tests
/// (`test-mcp-oauth-fixtures`).
#[cfg(feature = "test-mcp-oauth-fixtures")]
pub mod auth_errors;
