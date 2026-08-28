//! Native GitHub Copilot backend substrate.
//!
//! This crate intentionally does not depend on the Copilot SDK or CLI. It owns
//! the observed GitHub OAuth and CAPI contracts shared by the OpenAI,
//! Anthropic, and Gemini provider-family backends.

mod protocol;

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
mod runtime;

pub use protocol::{
    CopilotBackendConfig, CopilotEndpoint, CopilotModel, CopilotModelCapabilities,
    CopilotModelLimits, CopilotModelOffering, CopilotModelPolicy, CopilotModelPolicyState,
    CopilotModelSnapshot, CopilotModelSupports, CopilotModelsEnvelope, CopilotProviderRoute,
    CopilotTokenEnvelope, DEFAULT_COPILOT_API_BASE, GITHUB_COPILOT_AUTHORIZER_LABEL,
    GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID, GitHubCopilotEndpoints,
    PROVISIONAL_GITHUB_COPILOT_CLIENT_ID,
};

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub use runtime::{
    CopilotAuthorizer, CopilotChatCompletionsClientFactory, CopilotChatCompletionsClientSpec,
    CopilotModelAccess, CopilotResolvedAuth, CopilotRouteClientFactory, CopilotRouteResolution,
    CopilotRuntime, capability_gated_client, routed_client,
};
