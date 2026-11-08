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

// Without `workspace-tests` (the published package, whose test command
// leaves it off) the unit tests that need the workspace compile out, and
// the test helpers only they use go unused. The workspace's test builds
// turn the feature on and keep both lints.
#![cfg_attr(
    all(test, not(feature = "workspace-tests")),
    allow(dead_code, unused_imports)
)]

// Test-feature canary. This crate's tests build with its test features,
// which no dev-dependency turns on: in this repository every test command
// passes them (`node scripts/core-workspace-args.mjs --tests -p meerkat-auth-core`
// prints them). A test build without them would compile the tests they
// gate out and pass over fewer tests, so it fails to compile here instead.
// Only code generation instantiates the generic function below (a test
// binary does), so `cargo check`, clippy and an editor's check run, which
// generate no code, are not stopped by it.
// `.cargo/config.toml` [env] sets MEERKAT_WORKSPACE_TEST_FEATURES for this
// repository's builds only; the published package builds without it.
#[cfg(all(test, not(feature = "workspace-tests")))]
mod test_features_canary {
    fn require<T>() {
        const {
            let _ = size_of::<T>();
            assert!(
                option_env!("MEERKAT_WORKSPACE_TEST_FEATURES").is_none(),
                "meerkat-auth-core: this repository's test builds need the crate's test features: \
                 `cargo test -p meerkat-auth-core --features workspace-tests` \
                 (core-workspace-args.mjs --tests prints the flags for any selection)"
            );
        }
    }

    #[used]
    static REQUIRE: fn() = require::<()>;
}

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
    McpOAuthCancelHandle, McpOAuthCeremonyContext, McpOAuthClientAuthMethod, McpOAuthClientProfile,
    McpOAuthClientProfileRefusal, McpOAuthClientSecret, McpOAuthError,
    McpOAuthInstalledClientProfile, McpOAuthListenerCleanup, McpOAuthLoginComplete,
    McpOAuthLoginDisposition, McpOAuthLoginStart, McpOAuthLoopbackBegin, McpOAuthNetworkBounds,
    McpOAuthNetworkStage, McpOAuthPendingAttempt, McpOAuthPendingClose, McpOAuthPendingLogin,
    McpOAuthRegisteredCallback, McpRegisteredRedirectBase, McpServerIdentity,
    McpStoredCredentialPhase, McpStoredCredentialStatus, OidcUserInfoAccountStrategy,
    UNVERIFIED_RESOURCE_STRATEGY_ID, stored_credential_matches_selection,
};
#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_core::auth::{
    RefreshCoordinator, RefreshError, RefreshFailureObservation, TokenStore,
};

pub use resolver::{resolve_external_authorizer, resolve_simple_secret};
pub use self_hosted::SelfHostedProviderRuntime;
