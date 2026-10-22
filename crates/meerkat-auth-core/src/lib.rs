//! meerkat-auth-core — shared auth primitives for Meerkat.
//!
//! Owns the concrete implementations of the auth traits declared in
//! `meerkat-core`: TokenStore backends (File/Keyring/Auto/Ephemeral),
//! RefreshCoordinator impls (InMemory/FileLock), OAuth2 helpers
//! (PKCE/callback/device-code/token-exchange), and generic cloud-IAM
//! authorizers (AWS SigV4, Google ADC, Azure AD).
//!
//! Filesystem stores, keyring access, OS lockfiles, interactive OAuth, and
//! cloud-IAM authorizers are native-only. The resolver and self-hosted runtime
//! modules also compile on wasm32 so browser hosts can resolve inline or
//! externally supplied credentials and register provider runtimes.
//!
//! Deferral §3 B2 split (2026-04-18): extracted from `meerkat-providers`.

// Without `workspace-tests` (the published package: `cargo package` strips
// the self dev-dependency that enables it) the unit tests that need the
// workspace compile out, and the test helpers only they use go unused. The
// workspace build turns the feature on and keeps both lints.
#![cfg_attr(
    all(test, not(feature = "workspace-tests")),
    allow(dead_code, unused_imports)
)]

#[cfg(not(target_arch = "wasm32"))]
pub mod auth_oauth;
#[cfg(not(target_arch = "wasm32"))]
pub mod auth_store;
#[cfg(not(target_arch = "wasm32"))]
pub mod authorizers;
#[cfg(not(target_arch = "wasm32"))]
mod browser_login;
#[cfg(not(target_arch = "wasm32"))]
#[cfg(feature = "oauth")]
pub mod connector_login;
#[cfg(not(target_arch = "wasm32"))]
pub mod connector_oauth;
#[cfg(not(target_arch = "wasm32"))]
#[cfg(feature = "oauth")]
pub mod connector_protocol;
pub mod github_copilot;
#[cfg(not(target_arch = "wasm32"))]
pub mod http_egress;
#[cfg(not(target_arch = "wasm32"))]
#[cfg(feature = "oauth")]
pub mod mcp_oauth;
#[cfg(not(target_arch = "wasm32"))]
pub mod oauth_flow;
// `resolver` contains per-source-spec arms that are individually cfg-split
// for filesystem/command/managed-store sources. The InlineSecret, Env
// (host env_lookup), ExternalResolver, and authorizer-stub arms compile
// on wasm32 — that path is what browser WASM callers need when their
// bootstrap populates `config.realm` with
// `CredentialSourceSpec::InlineSecret` via
// `populate_realm_from_api_keys`. Exposing the module on wasm32 is what
// lets `meerkat-anthropic` / `meerkat-openai` / `meerkat-gemini`
// register their runtimes on wasm32 so `build_agent` can resolve
// provider credentials in the browser.
pub mod resolver;
pub mod self_hosted;

#[cfg(all(not(target_arch = "wasm32"), feature = "keyring"))]
pub use auth_store::KeyringTokenStore;
#[cfg(all(not(target_arch = "wasm32"), feature = "file-lock"))]
pub use auth_store::refresh::FileLockCoordinator;
#[cfg(not(target_arch = "wasm32"))]
pub use auth_store::refresh::InMemoryCoordinator;
#[cfg(not(target_arch = "wasm32"))]
pub use auth_store::{
    AutoTokenStore, CommandCredentialRunner, CommandCredentialSpec, EphemeralTokenStore,
    FileTokenStore,
};
#[cfg(not(target_arch = "wasm32"))]
pub use browser_login::{
    BrowserOAuthFlowCommit, save_oauth_tokens_and_consume_browser_flow,
    save_oauth_tokens_and_consume_device_flow, save_tokens_and_publish_lifecycle,
};
#[cfg(not(target_arch = "wasm32"))]
#[cfg(feature = "oauth")]
pub use mcp_oauth::{
    CredentialHttpClientUnavailable, MCP_INTERACTIVE_LOGIN_TIMEOUT, MCP_OAUTH_CALLBACK_PATH,
    MCP_OAUTH_COMMIT_OBSERVATION_BOUND, MCP_OAUTH_COMPLETION_BOUND, MCP_OAUTH_STAGE_BOUND,
    MCP_OAUTH_START_BOUND, McpAccountSelection, McpAuthMode, McpOAuthAccountStrategy,
    McpOAuthAttemptCleanup, McpOAuthAuthority, McpOAuthBrowserLaunch, McpOAuthCallback,
    McpOAuthCancelHandle, McpOAuthCeremonyContext, McpOAuthError, McpOAuthListenerCleanup,
    McpOAuthLoginComplete, McpOAuthLoginDisposition, McpOAuthLoginStart, McpOAuthLoopbackBegin,
    McpOAuthNetworkBounds, McpOAuthNetworkStage, McpOAuthPendingAttempt, McpOAuthPendingClose,
    McpOAuthPendingLogin, McpOAuthRegisteredCallback, McpRegisteredRedirectBase, McpServerIdentity,
    OidcUserInfoAccountStrategy, UNVERIFIED_RESOURCE_STRATEGY_ID,
    stored_credential_matches_selection,
};
#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_core::auth::{
    RefreshCoordinator, RefreshError, RefreshFailureObservation, TokenStore,
};

pub use resolver::{resolve_external_authorizer, resolve_simple_secret};
pub use self_hosted::SelfHostedProviderRuntime;
