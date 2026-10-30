//! Agent - the core agent orchestrator
//!
//! The Agent struct ties together all components and runs the agent loop.

#[cfg(all(test, feature = "workspace-tests"))]
mod assistant_message_identity_tests;
#[cfg(all(test, feature = "workspace-tests"))]
mod brain_swap_promotion_tests;
#[cfg(all(test, feature = "workspace-tests"))]
mod budget_warning_tests;
mod builder;
pub mod comms_impl;
pub mod compact;
#[cfg(all(test, feature = "workspace-tests"))]
mod compaction_retention_tests;
#[cfg(test)]
mod durable_boundary_tests;
mod entered_operation;
#[cfg(test)]
mod entered_operation_tests;
mod extraction;
mod hook_impl;
#[cfg(all(test, feature = "workspace-tests"))]
mod hooks_behavior_tests;
#[cfg(test)]
mod observation_barrier_tests;
pub(crate) mod reasoning_preference;
#[cfg(all(test, feature = "workspace-tests"))]
mod round_commit_tests;
mod runner;
pub mod skills;
mod state;
#[cfg(all(test, feature = "workspace-tests"))]
mod structured_output_tests;
#[cfg(all(test, feature = "workspace-tests"))]
#[doc(hidden)]
pub(crate) mod test_turn_state_handle;
mod tool_application_execution;
#[cfg(test)]
mod tool_delegate_tests;
#[cfg(test)]
mod usage_accounting_tests;
#[cfg(all(test, feature = "workspace-tests"))]
mod usage_degradation_tests;
use crate::budget::Budget;
use crate::comms::{
    CommsCommand, CommsTrustMutation, CommsTrustMutationResult, EventStream, PeerDirectoryEntry,
    PeerId, SendAndStreamError, SendError, SendReceipt, StreamError, StreamScope,
    TrustedPeerDescriptor,
};
use crate::compact::SessionCompactionCadence;
use crate::completion_feed::CompletionSeq;
use crate::config::{AgentConfig, HookRunOverrides};
use crate::error::AgentError;
use crate::event::ExternalToolDelta;
use crate::hooks::HookEngine;
use crate::lifecycle::RunId;
use crate::lifecycle::run_primitive::ProviderParamsOverride;
use crate::ops::OperationId;
use crate::ops_lifecycle::{OperationKind, OperationStatus, OperationTerminalOutcome};
use crate::provider_evidence::DisputedTurnUsageAccountingIdentity;
use crate::retry::RetryPolicy;
use crate::schema::{CompiledSchema, SchemaError};
use crate::session::Session;
use crate::state::LoopState;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_catalog::{
    ToolCatalogCapabilities, ToolCatalogEntry, ToolCatalogMode, deferred_session_entry_count,
    select_catalog_mode_from_snapshot,
};
use crate::tool_scope::ToolScope;
use crate::turn_execution_authority::{
    ContentShape, TurnPhase, TurnPrimitiveKind, TurnTerminalCauseKind, TurnTerminalOutcome,
};
use crate::types::{
    AssistantBlock, BlockAssistantMessage, Message, OutputSchema, StopReason, ToolCallView,
    ToolDef, ToolName, ToolNameSet, TurnUsage, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub use builder::{AgentBuildPolicyError, AgentBuilder, DefaultSystemPromptPolicy};
pub use entered_operation::EnteredDispatchOperation;
pub use runner::{
    AgentControlStateError, AgentRunner, LiveBridgePreparedOperation, SnapshotProjectionError,
};
pub(crate) use runner::{
    injected_context_message_from_operator_renderable, user_message_from_operator_renderable,
};

/// Whether one turn's accounting agrees with the request identity it answered.
///
/// This is a classification, not a gate. See
/// [`classify_provider_turn_usage_identity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnUsageIdentityVerdict {
    /// The accounting names the provider and model that were requested.
    Agreed,
    /// The accounting names some other provider/model. The counters are still
    /// the ones the adapter minted and are carried unchanged.
    Disputed(DisputedTurnUsageAccountingIdentity),
}

/// Classify one turn's accounting identity against the active request
/// identity.
///
/// # Why this reports rather than refuses
///
/// The fact in question is attribution: WHOSE model these counters describe.
/// It is not a statement about the counters themselves, which arrive with
/// their [`crate::PresentedTokenConvention`] attached and therefore mean what
/// they say under either name. So a mismatch degrades - the number is
/// recorded, the dispute is published - instead of failing a turn the model
/// already answered.
///
/// # Why the disputed identity is never repaired
///
/// The obvious "fix" is to stamp the active identity over the reported one.
/// That publishes an agreement nobody observed and destroys the only evidence
/// that anything disagreed, which is strictly worse than saying so.
///
/// # What this cannot see
///
/// Every in-tree adapter mints its accounting from the model it lowered into
/// the request (`crates/meerkat-anthropic/src/client.rs`, `meerkat-gemini`,
/// `meerkat-openai` both surfaces), so both sides of this comparison echo the
/// same identity and no in-tree stream reaches `Disputed`. The check exists
/// for host-implemented `AgentLlmClient`s and for adapters that later account
/// under a provider-resolved model; it must not be read as coverage of
/// provider-side model substitution, which no adapter currently reports.
pub(crate) fn classify_provider_turn_usage_identity(
    turn_usage: &TurnUsage,
    active_provider: crate::Provider,
    active_model: &str,
) -> TurnUsageIdentityVerdict {
    let accounting = turn_usage.accounting();
    if accounting.provider == active_provider && accounting.model == active_model {
        return TurnUsageIdentityVerdict::Agreed;
    }

    TurnUsageIdentityVerdict::Disputed(DisputedTurnUsageAccountingIdentity {
        active_provider,
        active_model: active_model.to_string(),
        reported_provider: accounting.provider,
        reported_model: accounting.model.clone(),
    })
}

/// One request attempt whose provider projection, pressure evidence, cache
/// claims, and dispatch share the same route authority.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentLlmRequestAttempt: Send + Sync {
    fn request_pressure(&self) -> Result<Option<crate::ProviderRequestPressure>, AgentError>;

    /// Dispatch this attempt and stream the provider response.
    ///
    /// `assistant_message_id` is the core-minted occurrence id of the
    /// assistant message this provider turn will commit. Implementations that
    /// publish live events (`text_delta`, `reasoning_delta`,
    /// `reasoning_complete`, `server_tool_content`) stamp every one of them
    /// with it, and decorators that wrap an attempt must pass it through
    /// unchanged. It is event metadata only: it never enters the provider
    /// request. Retries of the same provider turn receive the same id.
    async fn stream_response(
        &self,
        assistant_message_id: crate::types::AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestAttemptAuthority {
    LegacySplit,
    Unified,
}

struct DirectAgentLlmRequestAttempt<C: AgentLlmClient + ?Sized> {
    client: Arc<C>,
    messages: Arc<Vec<Message>>,
    tools: Arc<[Arc<ToolDef>]>,
    max_tokens: u32,
    temperature: Option<f32>,
    provider_params: Option<ProviderParamsOverride>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<C> AgentLlmRequestAttempt for DirectAgentLlmRequestAttempt<C>
where
    C: AgentLlmClient + ?Sized + 'static,
{
    fn request_pressure(&self) -> Result<Option<crate::ProviderRequestPressure>, AgentError> {
        self.client.request_pressure(
            &self.messages,
            &self.tools,
            self.max_tokens,
            self.temperature,
            self.provider_params.as_ref(),
        )
    }

    /// A legacy split client streams through [`AgentLlmClient::stream_response`],
    /// which has no message identity, so any live events it publishes carry
    /// none. The loop still stamps the events it emits itself.
    async fn stream_response(
        &self,
        _assistant_message_id: crate::types::AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError> {
        self.client
            .stream_response(
                &self.messages,
                &self.tools,
                self.max_tokens,
                self.temperature,
                self.provider_params.as_ref(),
            )
            .await
    }
}

/// Trait for LLM clients that can be used with the agent
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentLlmClient: Send + Sync {
    /// Exact factory-selected target of this client, when available. A mutable
    /// selector must return only its currently selected target; callers must
    /// retain a stable client rather than treating this data as a runnable pin.
    fn controller_model_selection(&self) -> Option<crate::ControllerModelSelection> {
        None
    }

    /// Request-free route facts from the actual immutable selected client.
    /// Implementations must not perform I/O, resolve credentials or manufacture
    /// a model request. Unsupported wrappers must refuse rather than guess.
    fn controller_model_facts(
        &self,
    ) -> Result<crate::ControllerModelFacts, crate::ControllerFactsUnavailable> {
        Err(crate::ControllerFactsUnavailable)
    }

    /// Prepare an existing controller credential without sending a model request.
    /// Native admission authenticates the caller before invoking this cold path
    /// and rechecks current authority afterward. This is not an operation permit
    /// or an initial credential acquisition. Unsupported clients refuse.
    async fn prepare_controller_credential(&self) -> Result<(), crate::auth::AuthError> {
        Err(crate::auth::AuthError::HostOwnedUnavailable)
    }

    /// Retain an actual immutable selected client for controller feedback.
    /// Mutable selectors must delegate to the selected child once. Unsupported
    /// clients return none; a retained selection DTO alone cannot replace this
    /// runnable handle at governed admission.
    fn pin_controller(self: Arc<Self>) -> Option<crate::ControllerModelClient> {
        None
    }

    /// Prepare an exact request with its admitted work authorization context.
    ///
    /// A decorator must forward this companion with the request it lowers.
    /// Legacy clients keep their existing behavior for `None`; they explicitly
    /// refuse `Some` rather than silently discarding a governed association.
    fn prepare_request_attempt_authorized(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
        authorization: Option<crate::LlmRequestAuthorization>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError>
    where
        Self: 'static,
    {
        if authorization.is_some() {
            return Err(AgentError::OperationRefused {
                refusal: crate::authorization::OperationRefused::new(
                    crate::authorization::OperationRefusalKind::MalformedFacts,
                ),
            });
        }
        self.prepare_request_attempt(messages, tools, max_tokens, temperature, provider_params)
    }

    /// Prepare one owned request attempt. Adapters with dynamic routing return
    /// a handle that binds all request-derived facts to one route witness.
    ///
    /// Stable custom clients receive one direct attempt by default. Dynamic
    /// adapters override this and report [`RequestAttemptAuthority::Unified`].
    ///
    /// A decorator that wraps another client MUST forward this method to the
    /// inner client's `prepare_request_attempt`, passing the messages it
    /// transforms (as it would in [`AgentLlmClient::stream_response`]).
    /// Otherwise the loop uses the default direct attempt, which streams
    /// through `stream_response` and carries no assistant message identity:
    /// every live delta published below an unforwarded decorator has no
    /// `assistant_message_id`.
    fn prepare_request_attempt(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError>
    where
        Self: 'static,
    {
        Ok(Arc::new(DirectAgentLlmRequestAttempt {
            client: self,
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
        }))
    }

    fn request_attempt_authority(&self) -> RequestAttemptAuthority {
        RequestAttemptAuthority::LegacySplit
    }

    /// Stream a response from the LLM
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError>;

    /// Collect a request-local authorized response without publishing a new
    /// transcript assistant message, for example during compaction.
    /// Unsupported custom clients must refuse the supplied context explicitly.
    async fn stream_response_authorized(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
        authorization: Option<crate::LlmRequestAuthorization>,
    ) -> Result<LlmStreamResult, AgentError> {
        if authorization.is_some() {
            return Err(AgentError::OperationRefused {
                refusal: crate::authorization::OperationRefused::new(
                    crate::authorization::OperationRefusalKind::MalformedFacts,
                ),
            });
        }
        self.stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await
    }

    /// Measure the exact provider-lowered JSON request body for this invocation.
    ///
    /// The default is deliberately unavailable. Provider adapters that can
    /// reproduce their stream request lowering return a witness; custom
    /// clients retain existing behavior without pretending an estimate is
    /// exact.
    fn request_pressure(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Option<crate::ProviderRequestPressure>, AgentError> {
        Ok(None)
    }

    /// Freshly lower the target request while core lends an ephemeral issuer.
    ///
    /// Implementations must return capabilities only from the lowering they
    /// would use for this exact request. Persisted source cache evidence is not
    /// an input and cannot satisfy this method's target-proof contract.
    fn target_cache_lowering_capabilities(
        &self,
        _issuer: &crate::TargetCacheLoweringIssuer,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Vec<crate::TargetCacheLoweringCapability>, AgentError> {
        Ok(Vec::new())
    }

    /// Get the typed catalog provider identity for this client.
    ///
    /// Clients return the typed [`crate::provider::Provider`] directly so no
    /// boundary ever parses a caller-supplied string back into catalog
    /// identity. String projections are derived via
    /// [`crate::provider::Provider::as_str`].
    fn provider(&self) -> crate::provider::Provider;

    /// Get the current effective model identifier.
    ///
    /// Used by the agent loop for profile-default resolution (e.g., call timeout
    /// defaults that vary per model family). Must reflect the current model even
    /// after hot-swap.
    fn model(&self) -> &str;

    /// Fork this exact provider/model/auth client for one concurrent
    /// noncommitting live-bridge operation.
    ///
    /// The fork must share provider and credential ownership with this client
    /// while isolating all per-response stream counters and event sinks. The
    /// default fails closed so a decorator cannot accidentally cross-wire
    /// bridge deltas into the ordinary member stream.
    fn fork_noncommitting_live_bridge(&self) -> Result<Arc<dyn AgentLlmClient>, AgentError> {
        Err(AgentError::ConfigError(
            "LLM client does not support an event-isolated concurrent live bridge fork".to_string(),
        ))
    }

    /// Prepare the next prebuilt fallback model after the generated turn
    /// authority has classified the LLM failure as recoverable.
    ///
    /// This method does not classify failures and must not call the provider.
    /// It only selects an already-constructed candidate and returns the typed
    /// state the agent loop must apply before the retry attempt.
    fn prepare_model_fallback(
        &self,
        _failure: &AgentError,
        _request: &crate::model_fallback::ModelFallbackRequest<'_>,
    ) -> Result<AgentLlmFallbackSwitch, Vec<AgentLlmFallbackSkippedTarget>> {
        Err(Vec::new())
    }

    /// Move the client-local active candidate from `previous_identity` to the
    /// exact `target_identity` as one reversible transaction step.
    ///
    /// The core loop invokes this only after every target-dependent operation
    /// (including target-provider schema compilation) has been prevalidated,
    /// but before auth/session/machine state is committed. Implementations must
    /// either perform the exact switch or return an error. The default fails
    /// closed so a custom client cannot propose a fallback while silently
    /// continuing to issue requests through its old provider client.
    ///
    /// Core verifies [`AgentLlmClient::active_model_fallback_identity`] after
    /// the call and invokes this method in reverse if a later transaction step
    /// fails.
    fn commit_model_fallback(
        &self,
        _previous_identity: &crate::SessionLlmIdentity,
        target_identity: &crate::SessionLlmIdentity,
    ) -> Result<(), AgentError> {
        Err(AgentError::ConfigError(format!(
            "LLM client proposed fallback target '{}:{}' without an activation implementation",
            target_identity.provider.as_str(),
            target_identity.model
        )))
    }

    /// Exact identity of the client-local active fallback candidate.
    ///
    /// Fallback-capable clients must expose the full session identity,
    /// including auth binding and provider parameters. The default is absent,
    /// which makes fallback activation fail closed before canonical state is
    /// mutated.
    fn active_model_fallback_identity(&self) -> Option<crate::SessionLlmIdentity> {
        None
    }

    /// Compile an extraction schema against an inactive fallback target.
    ///
    /// This must delegate to the exact prebuilt target client without changing
    /// which client is active. Core calls it before auth, machine, visibility,
    /// session, or client activation state is mutated, then injects the
    /// compiled representation into the target provider request.
    fn compile_model_fallback_schema(
        &self,
        target_identity: &crate::SessionLlmIdentity,
        _output_schema: &OutputSchema,
    ) -> Result<CompiledSchema, AgentError> {
        Err(AgentError::ConfigError(format!(
            "LLM client cannot compile structured output for fallback target '{}:{}'",
            target_identity.provider.as_str(),
            target_identity.model
        )))
    }

    /// Reset per-call observation of user-visible streaming output.
    ///
    /// Adapters that emit display/reasoning deltas before returning the final
    /// stream result use this to let the retry loop distinguish a pre-stream
    /// failure from a post-partial-output failure. The default is no-op for
    /// clients that do not stream visible events outside the returned blocks.
    fn begin_stream_output_observation(&self) {}

    /// Whether the current LLM call has emitted user-visible streaming output.
    ///
    /// A `true` value suppresses model fallback for the failed call: retrying
    /// against a different model after users already saw partial output can
    /// produce duplicate assistant answers. Ordinary same-model retry policy is
    /// still governed by the generated turn recovery authority.
    fn stream_output_observed(&self) -> bool {
        false
    }

    /// Monotonic count of raw provider stream events observed by this client.
    ///
    /// This feeds the agent loop's stream-inactivity watchdog
    /// (`RetryPolicy::stream_inactivity_timeout`): the loop snapshots the
    /// count around each stream-event window and treats "no change" as a
    /// silent stream. Clients that consume a provider event stream should bump
    /// the count on every received event — including non-visible ones — so
    /// liveness is distinct from visible output
    /// ([`Self::stream_output_observed`]).
    ///
    /// `None` (the default) means this client does not report stream liveness
    /// and the watchdog is disabled for its calls; only the hard call/turn
    /// timeouts apply. This fails open on purpose: a non-streaming custom
    /// client would otherwise look permanently silent and be killed while
    /// healthy.
    fn stream_activity_count(&self) -> Option<u64> {
        None
    }

    /// Compile an output schema for this provider.
    ///
    /// Default implementation normalizes the schema without provider-specific lowering.
    /// Adapters override this to apply provider-specific transformations (e.g.,
    /// Anthropic adds `additionalProperties: false`, Gemini strips unsupported keywords).
    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        // Default passthrough: normalized clone, no provider-specific lowering
        Ok(CompiledSchema {
            schema: output_schema.schema.as_value().clone(),
            warnings: Vec::new(),
        })
    }
}

/// Ask the active adapter to freshly lower one target request under a
/// core-issued, non-persistable cache-proof authority.
pub fn target_cache_lowering_capabilities(
    client: &dyn AgentLlmClient,
    messages: &[Message],
    tools: &[Arc<ToolDef>],
    max_tokens: u32,
    temperature: Option<f32>,
    provider_params: Option<&ProviderParamsOverride>,
) -> Result<Vec<crate::TargetCacheLoweringCapability>, AgentError> {
    let issuer = crate::TargetCacheLoweringIssuer::new();
    client.target_cache_lowering_capabilities(
        &issuer,
        messages,
        tools,
        max_tokens,
        temperature,
        provider_params,
    )
}

/// Hook for wrapping the final agent-facing LLM client.
///
/// Factories and runtimes apply this after provider/raw-client adaptation so
/// embedders can compose cross-cutting behavior without provider-specific
/// registry hooks. A wrapper around a unified client must forward
/// [`AgentLlmClient::prepare_request_attempt`] and
/// [`AgentLlmClient::request_attempt_authority`]; factory composition rejects
/// decorators that erase that contract.
pub type AgentLlmClientDecorator =
    Arc<dyn Fn(Arc<dyn AgentLlmClient>) -> Arc<dyn AgentLlmClient> + Send + Sync + 'static>;

/// One fallback target skipped while selecting a viable backup model.
pub use crate::model_fallback::ModelFallbackSkippedTarget as AgentLlmFallbackSkippedTarget;

/// Typed state produced when an agent-facing LLM client activates a fallback.
///
/// The client owns only prebuilt candidate selection. The agent loop owns
/// applying request policy, durable identity metadata, and tool visibility
/// before issuing the machine-authorized retry.
#[derive(Debug, Clone)]
pub struct AgentLlmFallbackSwitch {
    pub policy: crate::config::ModelFallbackPolicy,
    pub previous_identity: crate::SessionLlmIdentity,
    pub new_identity: crate::SessionLlmIdentity,
    pub request_policy: crate::SessionLlmRequestPolicy,
    /// Proposed effective-registry witness for the exact target identity, as
    /// the target's own binding serves it
    /// (`ModelRegistry::profile_witness_for_identity`). Core rejects a
    /// witness its captured registry did not mint, or one judged for another
    /// provider, model or binding, and keeps the accepted witness as the
    /// target's capability and token-limit facts. The witness is required:
    /// unresolved fallback targets fail closed.
    pub target_profile: crate::ModelProfileWitness,
    pub skipped_targets: Vec<AgentLlmFallbackSkippedTarget>,
}

/// One-shot authorization for an exact sticky model-fallback activation.
///
/// There is deliberately no public constructor and the fields are private.
/// The constructor is owned by the `agent` module, so only the core agent loop
/// can mint this value after generated recovery acceptance and exact
/// effective-registry validation. A public
/// [`crate::handles::ModelRoutingHandle`] therefore cannot be driven directly
/// with a caller-minted or foreign-registry profile.
///
/// ```compile_fail
/// use meerkat_core::StickyModelFallbackActivationProof;
///
/// // Routing callers cannot fabricate an activation proof.
/// let _proof = StickyModelFallbackActivationProof::new();
/// ```
pub struct StickyModelFallbackActivationProof {
    previous_identity: crate::SessionLlmIdentity,
    target_identity: crate::SessionLlmIdentity,
    target_profile: crate::ModelProfileWitness,
    target_capability_base_filter: crate::ToolFilter,
    retry_attempt: u32,
}

impl StickyModelFallbackActivationProof {
    fn new(
        previous_identity: crate::SessionLlmIdentity,
        target_identity: crate::SessionLlmIdentity,
        target_profile: crate::ModelProfileWitness,
        retry_attempt: u32,
    ) -> Self {
        let target_capability_base_filter = crate::capability_base_filter_for_image_tool_results(
            target_profile.profile().image_tool_results,
        );
        Self {
            previous_identity,
            target_identity,
            target_profile,
            target_capability_base_filter,
            retry_attempt,
        }
    }

    /// Exact identity the generated recovery transition must still own.
    pub fn previous_identity(&self) -> &crate::SessionLlmIdentity {
        &self.previous_identity
    }

    /// Exact registry-resolved identity being activated.
    pub fn target_identity(&self) -> &crate::SessionLlmIdentity {
        &self.target_identity
    }

    /// Registry-owned target profile carried by this authorization.
    pub fn target_profile(&self) -> &crate::ModelProfileWitness {
        &self.target_profile
    }

    /// Registry-derived capability filter for the target model.
    pub fn target_capability_base_filter(&self) -> &crate::ToolFilter {
        &self.target_capability_base_filter
    }

    /// Machine-accepted retry attempt bound into this authorization.
    pub fn retry_attempt(&self) -> u32 {
        self.retry_attempt
    }
}

/// Result of streaming from the LLM
pub struct LlmStreamResult {
    blocks: Vec<AssistantBlock>,
    stop_reason: StopReason,
    usage: Usage,
    cache_breakpoint_claims: Vec<crate::ProviderCacheBreakpointClaim>,
    stop_detail: Option<Box<crate::types::ProviderStopDetail>>,
}

impl LlmStreamResult {
    pub fn new(blocks: Vec<AssistantBlock>, stop_reason: StopReason, usage: Usage) -> Self {
        Self {
            blocks,
            stop_reason,
            usage,
            cache_breakpoint_claims: Vec::new(),
            stop_detail: None,
        }
    }

    /// The provider's own account of why it stopped, carried to the caller
    /// as tagged provider data (see [`crate::types::ProviderStopDetail`]).
    pub fn with_stop_detail(
        mut self,
        stop_detail: Option<Box<crate::types::ProviderStopDetail>>,
    ) -> Self {
        self.stop_detail = stop_detail;
        self
    }

    pub fn stop_detail(&self) -> Option<&crate::types::ProviderStopDetail> {
        self.stop_detail.as_deref()
    }

    pub fn blocks(&self) -> &[AssistantBlock] {
        &self.blocks
    }
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason
    }
    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    pub fn with_cache_breakpoint_claims(
        mut self,
        cache_breakpoint_claims: Vec<crate::ProviderCacheBreakpointClaim>,
    ) -> Self {
        self.cache_breakpoint_claims = cache_breakpoint_claims;
        self
    }

    pub fn cache_breakpoint_claims(&self) -> &[crate::ProviderCacheBreakpointClaim] {
        &self.cache_breakpoint_claims
    }

    pub fn into_message(self) -> BlockAssistantMessage {
        BlockAssistantMessage::new(self.blocks, self.stop_reason)
    }

    pub fn into_parts(self) -> (Vec<AssistantBlock>, StopReason, Usage) {
        (self.blocks, self.stop_reason, self.usage)
    }

    pub fn into_parts_with_cache_claims(
        self,
    ) -> (
        Vec<AssistantBlock>,
        StopReason,
        Usage,
        Vec<crate::ProviderCacheBreakpointClaim>,
    ) {
        (
            self.blocks,
            self.stop_reason,
            self.usage,
            self.cache_breakpoint_claims,
        )
    }
}

/// Snapshot of the core agent's live execution state.
///
/// When a runtime-backed turn-state handle is attached, this snapshots the
/// runtime-owned turn machine; otherwise it falls back to the in-process
/// standalone turn state used by core-only execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExecutionSnapshot {
    pub loop_state: LoopState,
    pub turn_phase: TurnPhase,
    /// Machine-owned turn-terminality verdict.
    ///
    /// The `TurnTerminalityClassified.terminal` verdict emitted by the canonical
    /// MeerkatMachine `ClassifyTurnTerminality` input. Consumers mirror this bool
    /// and must not reclassify [`TurnPhase`] locally.
    pub turn_terminal: bool,
    pub active_run_id: Option<RunId>,
    pub terminal_run_id: Option<RunId>,
    pub primitive_kind: TurnPrimitiveKind,
    pub admitted_content_shape: Option<ContentShape>,
    pub vision_enabled: bool,
    pub image_tool_results_enabled: bool,
    pub tool_calls_pending: u32,
    pub pending_operation_ids: Option<Vec<OperationId>>,
    pub barrier_operation_ids: Vec<OperationId>,
    pub has_barrier_ops: bool,
    pub barrier_satisfied: bool,
    pub boundary_count: u32,
    pub cancel_after_boundary: bool,
    pub terminal_outcome: TurnTerminalOutcome,
    pub terminal_cause_kind: Option<TurnTerminalCauseKind>,
    pub extraction_attempts: u32,
    pub max_extraction_retries: u32,
    pub applied_cursor: CompletionSeq,
}

/// Result of polling for external tool updates.
///
/// Returned by [`AgentToolDispatcher::poll_external_updates`].
#[derive(Debug, Clone, Default)]
pub struct ExternalToolUpdate {
    /// Notices about completed background operations since last poll.
    pub notices: Vec<ExternalToolDelta>,
    /// Names of servers still connecting in the background.
    pub pending: Vec<String>,
}

/// Where a run stood when the host dropped its future to suspend it
/// ([`Agent::suspend_dropped_run`]), for a stop that keeps the session's
/// queued work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DroppedRunSuspension {
    /// The run's latest checkpoint is a round commit, cursor included, or the
    /// run wrote none: it resumes there at the session's next start, as after
    /// a crash. A save without the cursor that was in flight when the future
    /// was dropped counts here: the store may hold either tail, and the next
    /// start's recovery reads which.
    Resumable {
        /// The suspended run.
        run_id: crate::lifecycle::RunId,
    },
    /// The run's save without the cursor (its end-of-run save, or the closing
    /// write of its exit) had landed: its latest checkpoint holds the run's
    /// committed work and no cursor. Nothing of the run is left to resume,
    /// and its inputs must not run again. It is not suspended: nothing of the
    /// run was cleared, and the caller ends it with
    /// [`Agent::cancel_dropped_run`], which publishes the run's terminal
    /// where it is still owed.
    PastFinalSave {
        /// The run past its final save.
        run_id: crate::lifecycle::RunId,
    },
    /// The run takes no round commits (its agent has no session
    /// checkpointer, or it is a live bridge run, which commits nothing), so
    /// nothing durable names its progress and it cannot be suspended: a stop
    /// cancels it, as before. Nothing of the run was cleared; the caller ends
    /// it with [`Agent::cancel_dropped_run`].
    NotResumable {
        /// The run that cannot be suspended.
        run_id: crate::lifecycle::RunId,
    },
}

impl DroppedRunSuspension {
    /// The run this outcome names.
    #[must_use]
    pub fn run_id(&self) -> &crate::lifecycle::RunId {
        match self {
            Self::Resumable { run_id }
            | Self::PastFinalSave { run_id }
            | Self::NotResumable { run_id } => run_id,
        }
    }
}

/// Typed command requesting cancellation at the next turn boundary.
///
/// Carried over the cancel-after-boundary command channel from the surface
/// that authorized the request (e.g. `SessionService::cancel_after_boundary`)
/// to the agent loop, which observes it at the next boundary. The agent
/// resolves the request against its own live active run. The exact run witness
/// prevents a delayed request from an old executor attachment from cancelling
/// a successor run after same-session replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelAfterBoundaryCommand {
    expected_run_id: RunId,
}

impl CancelAfterBoundaryCommand {
    /// Bind a cooperative-cancel command to one exact run incarnation.
    pub fn for_run(expected_run_id: RunId) -> Self {
        Self { expected_run_id }
    }

    /// Exact run incarnation this command is authorized to affect.
    pub fn expected_run_id(&self) -> &RunId {
        &self.expected_run_id
    }
}

/// Producer end of the cancel-after-boundary command channel.
///
/// Cloned and handed to the requesting surface via
/// [`Agent::cancel_after_boundary_handle`]; mirrors the cloneable-handle shape
/// of the session-side `interrupt_notify` so a surface can request boundary
/// cancellation without holding a reference to the agent.
pub type CancelAfterBoundarySender = tokio::sync::mpsc::UnboundedSender<CancelAfterBoundaryCommand>;

/// Test observer of each durable round commit, given the cursor it wrote.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub type RoundCommitObserver = Arc<dyn Fn(&crate::SessionRunCursor) + Send + Sync>;

/// Typed context supplied by the agent loop when dispatching a tool call.
///
/// This is a dispatch-time projection of the already-admitted turn input. It
/// lets tool surfaces resolve typed turn-scoped references, such as a
/// `source=current_turn, index=0` image ref, without writing surface-local
/// metadata into canonical transcript history.
#[derive(Clone, Default)]
pub struct ToolDispatchContext {
    application_binding: Option<crate::tool_application::ToolApplicationBinding>,
    application_control: Option<Arc<crate::ToolApplicationControlRequest>>,
    current_turn: Option<CurrentTurnContent>,
    turn_metadata: BTreeMap<String, serde_json::Value>,
    /// Every peer delivery this turn can answer with `reply_to_peer`, typed:
    /// parsed once from the turn metadata when it is set, then extended by
    /// the deliveries written into the running turn at model boundaries.
    peer_replies: TurnPeerReplies,
    origin_session_id: Option<crate::types::SessionId>,
    interaction_lineage_id: Option<crate::interaction::InteractionId>,
    run_id: Option<crate::RunId>,
    /// Zero-based index of the run's round this dispatch belongs to. Bound
    /// with the run id by the agent loop for every tool round, so a call has
    /// one dedupe key whether it runs fresh or again after a resume, and
    /// cleared once the round's calls are collected.
    round: Option<u64>,
    streaming: Option<crate::ToolStreamingDispatchContext>,
    live_bridge_admission: Option<LiveBridgeToolDispatchAdmission>,
    work_authorization: Option<crate::WorkAuthorizationContext>,
    prepared_authorization: Option<crate::authorization::PreparedOperationCheck>,
    /// Trusted host review composition attached by the agent. Absence never
    /// skips a required tier: R2 then settles as local unavailable feedback.
    operation_review: Option<Arc<crate::approval::review::BoundOperationReview>>,
    /// Review entry of the one prepared operation, shared by clones so the
    /// check point is repeatable and the effect leaf spends exactly once.
    review_entry: Option<crate::approval::review::ReviewEntry>,
    /// Owner-change waiting state of the fenced dispatch attempt, shared by
    /// clones so every entry staging of the attempt waits within one bound.
    staging_wait: Option<Arc<EntryStagingWait>>,
    /// The host's operation refusal observer with the agent's session,
    /// attached by the agent. Host-only: nothing here reaches the model.
    refusal_observer: Option<crate::authorization::RefusalObserverBinding>,
    /// The work's observation barrier, awaited by every fenced entry made
    /// with this context before any dispatcher code runs. Only the native
    /// runtime installs one, for a governed service call.
    observation_barrier: Option<Arc<dyn crate::authorization::OperationObservationBarrier>>,
    // Monotonic restriction set by the core execution policy owner.
    read_only_execution_required: bool,
}

/// The reply capabilities of one turn: the turn-start list, parsed once when
/// the turn metadata is set, then the deliveries the agent wrote into the
/// running turn at model boundaries, in apply order. Each (sender, delivery
/// id) appears once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TurnPeerReplies {
    deliveries: crate::comms::PeerReplyDispatchContext,
    /// Set when the turn-start value does not decode: the turn's reply
    /// context then fails closed, whatever joins it later.
    malformed: Option<crate::comms::PeerReplyContextError>,
}

impl TurnPeerReplies {
    fn from_turn_metadata(metadata: &BTreeMap<String, serde_json::Value>) -> Self {
        let Some(value) = metadata.get(crate::comms::COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY) else {
            return Self::default();
        };
        match serde_json::from_value::<crate::comms::PeerReplyDispatchContext>(value.clone()) {
            Ok(turn_start) => {
                let mut deliveries = crate::comms::PeerReplyDispatchContext::default();
                for delivery in turn_start.deliveries {
                    deliveries.insert_delivery(delivery);
                }
                Self {
                    deliveries,
                    malformed: None,
                }
            }
            Err(error) => Self {
                deliveries: crate::comms::PeerReplyDispatchContext::default(),
                malformed: Some(crate::comms::PeerReplyContextError::Malformed(
                    error.to_string(),
                )),
            },
        }
    }
}

/// Owned, single-use native entry for a leaf whose effect runs on a worker.
/// See [`ToolDispatchContext::reviewed_entry_ticket`].
/// An ungoverned dispatch carries an empty ticket (no context clone).
#[derive(Debug)]
pub struct ReviewedEntryTicket(Option<GovernedEntryTicket>);

#[derive(Debug)]
struct GovernedEntryTicket {
    context: ToolDispatchContext,
    with_plan: bool,
}

impl ReviewedEntryTicket {
    /// Run the single consuming entry step now, immediately before the
    /// worker's first effect and after its waits and cancellation checks.
    /// Hold the returned custody for the whole effect body: a spent review's
    /// `Used` projection is delivered only after it is dropped (and the
    /// admitting dispatch ended), even if that dispatch was cancelled while
    /// this worker kept running.
    pub fn enter(self) -> Result<crate::approval::review::ReviewedEntryCustody, crate::ToolError> {
        let Some(GovernedEntryTicket { context, with_plan }) = self.0 else {
            return Ok(crate::approval::review::ReviewedEntryCustody::none());
        };
        match context
            .prepared_authorization()
            .and_then(|prepared| prepared.tool_dispatch_parts())
        {
            Some((call, plan)) => context
                .enter_reviewed_effect_held(call, with_plan.then_some(plan))
                .map(|(_, custody)| custody),
            None => Ok(crate::approval::review::ReviewedEntryCustody::none()),
        }
    }
}

/// The governed operation one tool dispatch prepared, owned by a trusted
/// native owner that must check it again later, after its own waits (for
/// example a WorkGraph admission fence that commits under its own
/// reservation). See [`ToolDispatchContext::governed_operation`].
///
/// It keeps this dispatch's work context, prepared check and binding-owned
/// call and plan inside core; it never exposes them for replay. Every
/// [`Self::run_if_current_within`] checks the exact binding-owned call and
/// its retained plan against current owners again. A reviewed call that
/// continues with further governed steps after its single consuming entry
/// uses the entry's typed witness instead ([`Self::into_entered`],
/// [`EnteredDispatchOperation`]).
///
/// Each step is synchronous and decided once. A step that another actor's
/// hold of the work owners' exclusion refused to rebuild may succeed once
/// that hold ends: the waiting forms
/// ([`Self::run_if_current_within_after_owner_change`],
/// [`Self::issue_child_if_current_within_after_owner_change`]) wait for it
/// within the call's remaining tool time and try again, with nothing held
/// across the wait.
#[derive(Debug, Clone)]
pub struct GovernedDispatchOperation {
    context: ToolDispatchContext,
    work: crate::WorkAuthorizationContext,
}

impl GovernedDispatchOperation {
    /// The current decision for exactly this prepared operation, refreshed
    /// once from current owners when stale.
    ///
    /// # Errors
    /// A refused, unavailable or no longer current operation (for example
    /// its run ended).
    pub(crate) fn current(
        &self,
    ) -> Result<crate::authorization::PreparedOperationCheck, crate::ToolError> {
        self.current_decision().map_err(crate::ToolError::from)
    }

    /// [`Self::current`], as the owners answered it.
    fn current_decision(
        &self,
    ) -> Result<crate::authorization::PreparedOperationCheck, crate::OperationAuthorizationError>
    {
        let malformed = || {
            crate::OperationAuthorizationError::Refused(
                crate::authorization::OperationRefused::new(
                    crate::authorization::OperationRefusalKind::MalformedFacts,
                ),
            )
        };
        let (Some(work), Some(prepared)) = (
            self.context.work_authorization.as_ref(),
            self.context.prepared_authorization.as_ref(),
        ) else {
            return Err(malformed());
        };
        let (call, plan) = prepared.tool_dispatch_parts().ok_or_else(malformed)?;
        prepared.current_tool(work, call, Some(plan), self.context.run_id())
    }

    /// Run `operation` only while this exact prepared operation is
    /// currently permitted, with `further` conjoined by the work's own
    /// retained owners (see `WorkAuthorization::run_if_current_within`),
    /// under those owners' non-waiting exclusion held through `operation`.
    /// An entry that requires review refuses: no review runs here. The
    /// prepared check, binding and call never leave core. After the call's
    /// single consuming entry, a reviewed operation runs its further steps
    /// through the entry's witness ([`EnteredDispatchOperation`]).
    ///
    /// # Errors
    /// A refused, unavailable or no longer current operation; `operation`
    /// did not run. A refusal to rebuild (`ReprepareRequired`) is no verdict:
    /// the native work owner also answers it while another actor holds its
    /// exclusion, and [`Self::run_if_current_within_after_owner_change`]
    /// waits for such a hold and tries again.
    pub fn run_if_current_within(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
    ) -> Result<(), crate::ToolError> {
        let check = self.current()?;
        check
            .require_unreviewed_entry()
            .map_err(crate::ToolError::from)?;
        self.work
            .authorization()
            .run_if_current_within(
                check.binding(),
                check.validity().as_ref(),
                further,
                operation,
            )
            .map_err(crate::ToolError::from)
    }

    /// [`Self::run_if_current_within`], waiting out what refused it to
    /// rebuild: an owner change in progress on the work's own publication,
    /// or another actor's hold of the work owners' exclusion (the native
    /// work owner refuses such contention as `ReprepareRequired` and names
    /// the hold through `WorkAuthorization::owner_change_in_progress`). After
    /// each such refusal it waits for the change to complete, within the
    /// bounds of [`crate::authorization::OwnerChangeChurn`] and never past
    /// this call's remaining tool time
    /// ([`ToolDispatchContext::remaining_tool_time`]), then tries again; with
    /// no change in progress at that read (the holder may have finished in
    /// between) it pauses one backoff step and tries again. Every retry is
    /// one of the step's free rebuilds, all within one deadline set at its
    /// first wait. Each wait is traced at site `tool_step` and reported as
    /// this call's to the host's operation refusal observer. A streaming
    /// tool's inactivity window is not part of the remaining tool time:
    /// inside a streaming tool, a wait longer than the window left ends as
    /// the tool's inactivity timeout, as the execution policy gate's staging
    /// wait does.
    ///
    /// Each attempt is the synchronous step itself, decided afresh, and it
    /// releases every guard, lock and custody read before it returns:
    /// nothing is held across a wait. `operation` runs at most once, inside
    /// the attempt that answers `Ok`, under the owners' exclusion, and must
    /// not await. Nothing is spent: as on the synchronous step, only an R1
    /// operation runs here. Shell synchronization only: no machine input.
    ///
    /// # Errors
    /// As [`Self::run_if_current_within`], once the bounds are spent: the
    /// last attempt's answer, `ReprepareRequired` included; `operation` did
    /// not run.
    pub async fn run_if_current_within_after_owner_change(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        operation: impl FnOnce(),
    ) -> Result<(), crate::ToolError> {
        let mut wait = StepOwnerChangeWait::new(&self.context, &self.work, "tool_step");
        let mut operation = Some(operation);
        loop {
            let answer = self.run_if_current_within(further, &mut || {
                if let Some(run) = operation.take() {
                    run();
                }
            });
            // An attempt whose operation ran is the step's answer, whatever
            // it says: the operation never runs twice.
            let rebuild = operation.is_some() && refuses_to_rebuild(&answer);
            if rebuild && wait.again().await {
                continue;
            }
            return answer;
        }
    }

    /// Issue (or recover) a child grant for `intent` as this dispatch's
    /// effect, only while this exact prepared operation is currently
    /// permitted with `further` conjoined, under the work owners' single
    /// exclusion (see `WorkAuthorization::issue_child_if_current_within`).
    /// The work owner first validates the typed `further` and `intent`
    /// (before any review is spent); then THIS dispatch's own review custody
    /// is entered (R1; the R2 review this dispatch admitted; or, in a fresh
    /// attempt of a consented candidate, R3 or an R2 decision a human took
    /// over, its authenticated human consent) and held through the issuance.
    /// An R3 decision never reaches a dispatcher outside such an attempt.
    /// This is the dispatch's single consuming entry: for R2 and R3 a second
    /// call finds the review spent (`AlreadyConsumed`), while an R1 entry
    /// spends nothing and can issue again. A reviewed call that issues more
    /// than one child, or admits other governed steps too, enters once with
    /// [`ToolDispatchContext::enter_governed_operation`] and issues through
    /// that witness ([`EnteredDispatchOperation::issue_child_if_current_within`])
    /// instead, never through this method. The returned box carries the grant
    /// owner's typed outcome, which callers must keep: an outer `Ok` is no
    /// receipt.
    ///
    /// # Errors
    /// A refused, unavailable or no longer current operation, or an entry
    /// this dispatch cannot make; nothing was issued. A refusal to rebuild
    /// (`ReprepareRequired`) is no verdict: the native work owner also
    /// answers it while another actor holds its exclusion, and
    /// [`Self::issue_child_if_current_within_after_owner_change`] waits for
    /// such a hold and tries again without spending again.
    pub fn issue_child_if_current_within(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<Box<dyn std::any::Any + Send>, crate::ToolError> {
        let prepared = self
            .context
            .prepared_authorization
            .as_ref()
            .ok_or_else(governed_malformed)?;
        let (call, plan) = prepared
            .tool_dispatch_parts()
            .ok_or_else(governed_malformed)?;
        // Validate the typed inputs before spending this dispatch's review: a
        // malformed call must leave the entry spendable.
        self.work
            .authorization()
            .validate_child_issuance(further, intent)
            .map_err(crate::ToolError::from)?;
        // The single consuming entry of this dispatch's effect: the final
        // current check and this dispatch's own review custody.
        let (entering, _custody) = self.context.enter_reviewed_effect_held(call, Some(plan))?;
        let entered = entering.as_ref().unwrap_or(&self.context);
        let check = entered
            .prepared_authorization
            .as_ref()
            .ok_or_else(governed_malformed)?;
        self.work
            .authorization()
            .issue_child_if_current_within(
                check.binding(),
                check.validity().as_ref(),
                further,
                intent,
                check.review_tier(),
            )
            .map_err(crate::ToolError::from)
    }

    /// [`Self::issue_child_if_current_within`], waiting out what refused it
    /// to rebuild, as [`Self::run_if_current_within_after_owner_change`]
    /// does (site `tool_child_issuance`). The work owner validates `further`
    /// and `intent` first. Then THIS dispatch's single consuming entry is
    /// made once, its staging waiting out an owner change in progress within
    /// the dispatch attempt's own bounds (site `tool_entry`). That staging
    /// shares the attempt's waiting state with the attempt's other stagings
    /// and its review's post-verdict check, so whatever they spent of its
    /// deadline and free rebuilds is spent here too. Unlike the dispatch's
    /// root staging, it never reviews again: at R2 an owner change it meets
    /// supersedes the reviewed decision, so once the change completes the
    /// call answers `ReviewUnsatisfied` (`ContextChanged`), as a later
    /// synchronous call would. R1 goes on, and so does an R3 consent, which
    /// its spend re-adjudicates.
    ///
    /// The child is then issued once under the entry just made, exactly as
    /// the synchronous form issues it: with the entering decision and no
    /// step admission, so another holder of the review owner (an approval
    /// read, another dispatch's admission) cannot refuse it after the spend.
    /// Only after a refusal to rebuild does it wait and issue again, through
    /// the entry's witness
    /// ([`EnteredDispatchOperation::issue_child_if_current_within_after_owner_change`]),
    /// whose attempts never spend and are each admitted by the review owner,
    /// since the dispatch may have ended meanwhile. An R2 review or an R3
    /// consent is spent exactly once, however often the issuance waits. As
    /// for the synchronous form, this is the dispatch's single consuming
    /// entry: for R2 and R3 a second call finds the review spent. Nothing is
    /// held across a wait. The returned box carries the grant owner's typed
    /// outcome, which callers must keep: an outer `Ok` is no receipt.
    ///
    /// # Errors
    /// As [`Self::issue_child_if_current_within`], once the bounds are
    /// spent; nothing was issued. A refusal after a wait is the witness
    /// step's, with no not-dispatched certificate.
    pub async fn issue_child_if_current_within_after_owner_change(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<Box<dyn std::any::Any + Send>, crate::ToolError> {
        let prepared = self
            .context
            .prepared_authorization
            .as_ref()
            .ok_or_else(governed_malformed)?;
        let (call, plan) = prepared
            .tool_dispatch_parts()
            .ok_or_else(governed_malformed)?;
        // Validate the typed inputs before spending this dispatch's review: a
        // malformed call must leave the entry spendable.
        self.work
            .authorization()
            .validate_child_issuance(further, intent)
            .map_err(crate::ToolError::from)?;
        // The single consuming entry, made once.
        let (entering, custody) = self
            .context
            .enter_reviewed_effect_held_after_owner_change(call, Some(plan))
            .await?;
        // The first issuance is the synchronous form's own, under the entry
        // just made and with its custody held: the entering decision, and no
        // step admission that a holder of the review owner could refuse.
        let first = {
            let entered = entering.as_ref().unwrap_or(&self.context);
            let check = entered
                .prepared_authorization
                .as_ref()
                .ok_or_else(governed_malformed)?;
            self.work
                .authorization()
                .issue_child_if_current_within(
                    check.binding(),
                    check.validity().as_ref(),
                    further,
                    intent,
                    check.review_tier(),
                )
                .map_err(crate::ToolError::from)
        };
        let mut wait = StepOwnerChangeWait::new(&self.context, &self.work, "tool_child_issuance");
        if !refuses_to_rebuild(&first) || !wait.again().await {
            return first;
        }
        // Only a refusal to rebuild goes on, through the entry's witness:
        // its attempts never spend, and each is admitted by the review owner,
        // since the dispatch may have ended during the wait.
        let operation = match entering {
            Some(entering) => entering.governed_operation(call)?,
            None => self.clone(),
        };
        operation
            .into_entered(custody)?
            .issue_child_waiting(&mut wait, further, intent)
            .await
    }

    /// The governed work context this operation was prepared in.
    pub fn work_authorization(&self) -> &crate::WorkAuthorizationContext {
        // Constructed only from a governed context, which carries one.
        &self.work
    }

    /// The typed witness of this operation's entered effect, for a leaf that
    /// already made the dispatch's single consuming entry itself
    /// ([`ToolDispatchContext::enter_reviewed_effect_held`], or
    /// [`ReviewedEntryTicket::enter`] on a worker). `custody` must be the one
    /// this dispatch's own entry returned; nothing is spent here. The
    /// operation may come from the dispatch's context or from the refreshed
    /// context the entry returned: every clone of a dispatch context shares
    /// the dispatch's one review entry, so both accept the same custody, and
    /// the entered tier and window are those of the decision the entry
    /// actually spent. An unreviewed (R1) dispatch passes the inert custody
    /// its entry returned, and its entered tier is R1. See
    /// [`EnteredDispatchOperation`].
    ///
    /// Accepted: this dispatch's own held custody while its entry is entered
    /// (R2 or R3), or an inert custody when this dispatch's entry is
    /// unreviewed or absent (R1).
    ///
    /// # Errors
    /// The effect may already have begun, so no error says that nothing
    /// entered: custody of another dispatch, an inert custody for a reviewed
    /// dispatch, or a review that was not spent is `AuthorizationRefused`
    /// (`MalformedFacts`); a dispatch that already ended, or whose spend
    /// entered nothing, is `AuthorizationRefused` (`Denied`). Neither carries
    /// a not-dispatched certificate.
    pub fn into_entered(
        self,
        custody: crate::approval::review::ReviewedEntryCustody,
    ) -> Result<EnteredDispatchOperation, crate::ToolError> {
        EnteredDispatchOperation::from_custody(self, custody)
    }
}

fn governed_malformed() -> crate::ToolError {
    crate::ToolError::AuthorizationRefused {
        refusal: crate::authorization::OperationRefused::new(
            crate::authorization::OperationRefusalKind::MalformedFacts,
        ),
    }
}

/// Process-local live bridge authority carried to the last actual tool
/// dispatch boundary. The opaque id participates in equality and diagnostics;
/// the authority object itself is deliberately never rendered.
#[derive(Clone)]
pub struct LiveBridgeToolDispatchAdmission {
    operation_id: Arc<str>,
    admission: Arc<dyn crate::ToolDispatchAdmission>,
}

/// One-use generated authority for entering the private noncommitting Agent
/// runner. Clones share the same consumption bit so request custody may move
/// across actors without creating a second model-computation authorization.
#[derive(Clone)]
pub struct LiveBridgeNoncommittingRunPermit {
    operation_id: Arc<str>,
    session_id: crate::SessionId,
    canonical_context_revision: crate::CanonicalContextRevision,
    consumed: Arc<std::sync::atomic::AtomicBool>,
}

impl LiveBridgeNoncommittingRunPermit {
    #[allow(dead_code)] // Called by the generated linker bridge in shipping compositions.
    fn new(
        operation_id: Arc<str>,
        session_id: crate::SessionId,
        canonical_context_revision: crate::CanonicalContextRevision,
    ) -> Self {
        Self {
            operation_id,
            session_id,
            canonical_context_revision,
            consumed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Validate exact request binding without consuming model authority.
    pub fn validate_binding(
        &self,
        operation_id: &str,
        session_id: &crate::SessionId,
        canonical_context_revision: &crate::CanonicalContextRevision,
    ) -> Result<(), crate::AgentError> {
        if self.operation_id.as_ref() != operation_id
            || &self.session_id != session_id
            || &self.canonical_context_revision != canonical_context_revision
        {
            return Err(crate::AgentError::ConfigError(
                "live bridge run permit does not match the exact operation, session, and snapshot revision"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn consume(
        &self,
        operation_id: &str,
        session_id: &crate::SessionId,
        canonical_context_revision: &crate::CanonicalContextRevision,
    ) -> Result<(), crate::AgentError> {
        self.validate_binding(operation_id, session_id, canonical_context_revision)?;
        self.consumed
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .map_err(|_| {
                crate::AgentError::ConfigError(
                    "live bridge model-computation authority was already consumed".to_string(),
                )
            })?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    pub fn __test_new(
        operation_id: impl Into<Arc<str>>,
        session_id: crate::SessionId,
        canonical_context_revision: crate::CanonicalContextRevision,
    ) -> Self {
        Self::new(operation_id.into(), session_id, canonical_context_revision)
    }
}

impl std::fmt::Debug for LiveBridgeNoncommittingRunPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveBridgeNoncommittingRunPermit")
            .field("operation_id", &"[REDACTED]")
            .field("session_id", &"[REDACTED]")
            .field("canonical_context_revision", &"[REDACTED]")
            .field(
                "consumed",
                &self.consumed.load(std::sync::atomic::Ordering::Acquire),
            )
            .finish()
    }
}

impl LiveBridgeToolDispatchAdmission {
    #[must_use]
    #[allow(dead_code)] // Called by the generated linker bridge in shipping compositions.
    pub(crate) fn new(
        operation_id: impl Into<Arc<str>>,
        admission: Arc<dyn crate::ToolDispatchAdmission>,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            admission,
        }
    }

    /// Test-only constructor for exercising the final Agent dispatch seam
    /// without linking a generated runtime machine.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    pub fn __test_new(
        operation_id: impl Into<Arc<str>>,
        admission: Arc<dyn crate::ToolDispatchAdmission>,
    ) -> Self {
        Self::new(operation_id, admission)
    }

    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub fn admission(&self) -> &Arc<dyn crate::ToolDispatchAdmission> {
        &self.admission
    }
}

#[cfg(all(
    meerkat_internal_generated_authority_bridge,
    feature = "__meerkat-live-authority-bridge",
    not(test)
))]
#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_bridge_tool_dispatch_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn runtime_live_bridge_tool_dispatch_generated_authority_bridge_token_is_valid(
        token: &(dyn std::any::Any + Send + Sync),
    ) -> bool;
}

#[cfg(all(
    meerkat_internal_generated_authority_bridge,
    feature = "__meerkat-live-authority-bridge",
    not(test)
))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_runtime_generated_live_bridge_tool_dispatch_admission_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_live_bridge_tool_dispatch_admission_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    operation_id: Arc<str>,
    admission: Arc<dyn crate::ToolDispatchAdmission>,
) -> Result<LiveBridgeToolDispatchAdmission, String> {
    #[allow(unsafe_code)]
    let valid = unsafe {
        runtime_live_bridge_tool_dispatch_generated_authority_bridge_token_is_valid(token)
    };
    if !valid {
        return Err(
            "live bridge tool dispatch admission requires generated runtime authority".to_string(),
        );
    }
    Ok(LiveBridgeToolDispatchAdmission::new(
        operation_id,
        admission,
    ))
}

#[cfg(all(
    meerkat_internal_generated_authority_bridge,
    feature = "__meerkat-live-authority-bridge",
    not(test)
))]
#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_bridge_noncommitting_run_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn runtime_live_bridge_noncommitting_run_generated_authority_bridge_token_is_valid(
        token: &(dyn std::any::Any + Send + Sync),
    ) -> bool;
}

#[cfg(all(
    meerkat_internal_generated_authority_bridge,
    feature = "__meerkat-live-authority-bridge",
    not(test)
))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_runtime_generated_live_bridge_noncommitting_run_permit_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn runtime_generated_live_bridge_noncommitting_run_permit_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    operation_id: Arc<str>,
    session_id: crate::SessionId,
    canonical_context_revision: crate::CanonicalContextRevision,
) -> Result<LiveBridgeNoncommittingRunPermit, String> {
    #[allow(unsafe_code)]
    let valid = unsafe {
        runtime_live_bridge_noncommitting_run_generated_authority_bridge_token_is_valid(token)
    };
    if !valid {
        return Err(
            "live bridge noncommitting run permit requires generated runtime authority".to_string(),
        );
    }
    Ok(LiveBridgeNoncommittingRunPermit::new(
        operation_id,
        session_id,
        canonical_context_revision,
    ))
}

impl std::fmt::Debug for LiveBridgeToolDispatchAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveBridgeToolDispatchAdmission")
            .field("operation_id", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl PartialEq for LiveBridgeToolDispatchAdmission {
    fn eq(&self, other: &Self) -> bool {
        self.operation_id == other.operation_id && Arc::ptr_eq(&self.admission, &other.admission)
    }
}

impl Eq for LiveBridgeToolDispatchAdmission {}

impl std::fmt::Debug for ToolDispatchContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolDispatchContext")
            .field("current_turn", &self.current_turn)
            .field("turn_metadata", &self.turn_metadata)
            .field("peer_replies", &self.peer_replies)
            .field("origin_session_id", &self.origin_session_id)
            .field("interaction_lineage_id", &self.interaction_lineage_id)
            .field("run_id", &self.run_id)
            .field("round", &self.round)
            .field("streaming", &self.streaming)
            .field("live_bridge_admission", &self.live_bridge_admission)
            .field("work_authorization", &self.work_authorization)
            .field("prepared_authorization", &self.prepared_authorization)
            .field("operation_review", &self.operation_review.is_some())
            .field("review_entry", &self.review_entry)
            .field("staging_wait", &self.staging_wait.is_some())
            .field("refusal_observer", &self.refusal_observer.is_some())
            .field(
                "read_only_execution_required",
                &self.read_only_execution_required,
            )
            .finish()
    }
}

impl PartialEq for ToolDispatchContext {
    fn eq(&self, other: &Self) -> bool {
        self.application_binding == other.application_binding
            && match (&self.application_control, &other.application_control) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
            && self.current_turn == other.current_turn
            && self.turn_metadata == other.turn_metadata
            && self.peer_replies == other.peer_replies
            && self.origin_session_id == other.origin_session_id
            && self.interaction_lineage_id == other.interaction_lineage_id
            && self.run_id == other.run_id
            && self.round == other.round
            && self.streaming == other.streaming
            && self.live_bridge_admission == other.live_bridge_admission
            && match (&self.work_authorization, &other.work_authorization) {
                (Some(left), Some(right)) => left.same_context(right),
                (None, None) => true,
                _ => false,
            }
            && match (&self.prepared_authorization, &other.prepared_authorization) {
                (Some(left), Some(right)) => left.same_check(right),
                (None, None) => true,
                _ => false,
            }
            && match (&self.operation_review, &other.operation_review) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
            && match (&self.review_entry, &other.review_entry) {
                (Some(left), Some(right)) => left.same_entry(right),
                (None, None) => true,
                _ => false,
            }
            && match (&self.staging_wait, &other.staging_wait) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
            && match (&self.refusal_observer, &other.refusal_observer) {
                (Some(left), Some(right)) => left.same_binding(right),
                (None, None) => true,
                _ => false,
            }
            && match (&self.observation_barrier, &other.observation_barrier) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
            && self.read_only_execution_required == other.read_only_execution_required
    }
}

impl Eq for ToolDispatchContext {}

/// Dispatch-context key carrying the current durable objective id.
pub const TOOL_DISPATCH_OBJECTIVE_ID_KEY: &str = "meerkat.objective_id";

impl ToolDispatchContext {
    pub fn tool_application_control(&self) -> Option<&Arc<crate::ToolApplicationControlRequest>> {
        self.application_control.as_ref()
    }

    pub fn with_tool_application_control(
        mut self,
        control: Arc<crate::ToolApplicationControlRequest>,
    ) -> Self {
        self.application_control = Some(control);
        self
    }
    pub fn application_binding(&self) -> Option<&crate::tool_application::ToolApplicationBinding> {
        self.application_binding.as_ref()
    }

    pub(crate) fn with_application_binding(
        mut self,
        binding: crate::tool_application::ToolApplicationBinding,
    ) -> Self {
        self.application_binding = Some(binding);
        self
    }
    /// Whether an enclosing execution policy requires a positive read-only
    /// declaration at the actual tool binding. This restriction is monotonic.
    pub const fn read_only_execution_required(&self) -> bool {
        self.read_only_execution_required
    }

    pub(crate) fn require_read_only_execution(&mut self) {
        self.read_only_execution_required = true;
    }

    pub fn from_current_turn_input(input: &crate::types::ContentInput) -> Self {
        let blocks = match input {
            crate::types::ContentInput::Text(_) => None,
            crate::types::ContentInput::Blocks(blocks) => Some(blocks.clone()),
        };
        Self {
            application_binding: None,
            application_control: None,
            current_turn: blocks.map(CurrentTurnContent::new),
            turn_metadata: BTreeMap::new(),
            peer_replies: TurnPeerReplies::default(),
            origin_session_id: None,
            interaction_lineage_id: None,
            run_id: None,
            round: None,
            streaming: None,
            live_bridge_admission: None,
            work_authorization: None,
            prepared_authorization: None,
            operation_review: None,
            review_entry: None,
            staging_wait: None,
            refusal_observer: None,
            observation_barrier: None,
            read_only_execution_required: false,
        }
    }

    /// Project the typed run input into a dispatch context. The
    /// pending-tool-results continuation carries no caller content, so it
    /// projects to an empty context rather than a fabricated empty prompt.
    pub fn from_run_input(input: &crate::types::RunInput) -> Self {
        match input {
            crate::types::RunInput::Content { content } => Self::from_current_turn_input(content),
            crate::types::RunInput::PendingToolResults => Self::default(),
        }
    }

    /// Attach the exact admitted work context without selecting a new scope.
    #[must_use]
    pub fn with_work_authorization(
        mut self,
        context: Option<crate::WorkAuthorizationContext>,
    ) -> Self {
        self.work_authorization = context;
        // A decision prepared for a prior work context cannot be retained,
        // nor can its dispatch attempt's waiting state.
        self.prepared_authorization = None;
        self.review_entry = None;
        self.staging_wait = None;
        self
    }

    /// Attach trusted host review composition. It only enables review; the
    /// required tier stays the prepared authorization's own truth.
    #[must_use]
    pub fn with_operation_review(
        mut self,
        review: Option<Arc<crate::approval::review::BoundOperationReview>>,
    ) -> Self {
        self.operation_review = review;
        self
    }

    pub(crate) fn operation_review(
        &self,
    ) -> Option<&Arc<crate::approval::review::BoundOperationReview>> {
        self.operation_review.as_ref()
    }

    #[must_use]
    pub(crate) fn with_review_entry(mut self, entry: crate::approval::review::ReviewEntry) -> Self {
        self.review_entry = Some(entry);
        self
    }

    /// Attach the fenced dispatch attempt's owner-change waiting state.
    #[must_use]
    fn with_staging_wait(mut self, wait: Arc<EntryStagingWait>) -> Self {
        self.staging_wait = Some(wait);
        self
    }

    /// Attach the work's observation barrier: every fenced entry made with
    /// this context awaits it before any dispatcher code runs, and its error
    /// refuses the call unsent (see
    /// [`crate::authorization::OperationObservationBarrier`]).
    ///
    /// Only the native runtime installs one, on the context of a governed
    /// service call it built itself; host code never does. It grants
    /// nothing, and every other context carries none.
    #[must_use]
    pub fn with_observation_barrier(
        mut self,
        barrier: Arc<dyn crate::authorization::OperationObservationBarrier>,
    ) -> Self {
        self.observation_barrier = Some(barrier);
        self
    }

    /// Attach the host's operation refusal observer, bound to the agent's
    /// session.
    #[must_use]
    pub(crate) fn with_refusal_observer(
        mut self,
        binding: Option<crate::authorization::RefusalObserverBinding>,
    ) -> Self {
        self.refusal_observer = binding;
        self
    }

    /// A fresh owner-change waiting state for a delegate operation of this
    /// dispatch's own call, such as a helper's model request, or for one of
    /// its waiting governed steps (see [`crate::authorization::OwnerChangeChurn`]
    /// and [`GovernedDispatchOperation::run_if_current_within_after_owner_change`]).
    /// Its waits are reported as
    /// this call's to the operation refusal observer of the host that
    /// composed the agent, when one is installed.
    #[must_use]
    pub fn owner_change_churn(&self) -> crate::authorization::OwnerChangeChurn {
        let report = self
            .prepared_authorization()
            .and_then(crate::authorization::PreparedOperationCheck::tool_dispatch_parts)
            .and_then(|(call, _)| self.call_wait_report(call.id));
        crate::authorization::OwnerChangeChurn::reporting(report)
    }

    /// Where the waits of an operation of the call `tool_use_id` are
    /// reported, when an operation refusal observer is attached.
    pub(crate) fn call_wait_report(
        &self,
        tool_use_id: &str,
    ) -> Option<crate::authorization::WaitReport> {
        self.refusal_observer.as_ref().map(|binding| {
            binding.wait_report(
                self.run_id().cloned(),
                crate::authorization::OperationRefusalSurface::ToolCall {
                    tool_use_id: tool_use_id.to_owned(),
                },
            )
        })
    }

    /// Report `error`, the result of the call `tool_use_id`, to the attached
    /// operation refusal observer when it is an operation refusal or owners
    /// that could not decide: it is returned to the model as that call's
    /// result. The outcome is read from the same refusal value its model
    /// payload renders.
    pub(crate) fn observe_tool_refusal(
        &self,
        tool_use_id: &str,
        operation_id: Option<crate::OperationId>,
        error: &crate::ToolError,
    ) {
        let (Some(binding), Some(outcome)) = (
            self.refusal_observer.as_ref(),
            crate::authorization::OperationRefusalOutcome::of_tool_error(error),
        ) else {
            return;
        };
        binding.observe(
            self.run_id().cloned(),
            operation_id,
            crate::authorization::OperationRefusalSurface::ToolCall {
                tool_use_id: tool_use_id.to_owned(),
            },
            outcome,
            crate::authorization::OperationRefusalDisposition::ToolResult,
        );
    }

    /// Non-consuming review check point for the decision entry staging
    /// returned. Ungoverned dispatch has no prepared decision and no tier.
    /// A retained model review that a completed owner publication change
    /// superseded comes back typed (see [`Self::stage_root_tool_entry`]).
    fn check_review(
        &self,
        entering: &Self,
    ) -> Result<(), crate::approval::review::ReviewAdmissionRefusal> {
        let Some(check) = entering.prepared_authorization() else {
            return Ok(());
        };
        match self.review_entry.as_ref() {
            Some(entry) => entry.check_superseding(check),
            // A governed decision that did not traverse the common prepared
            // entry has no admitted review and may enter only at R1.
            None => check
                .require_unreviewed_entry()
                .map_err(|refusal| crate::ToolError::from(refusal).into()),
        }
    }

    /// Whether this context carries actual bound work and a prepared
    /// authorization for exactly `call` and `plan`: the binding-owned call
    /// and plan a governed dispatch forwards to its dispatcher, matched by
    /// identity. An ungoverned or default context, whatever runtime identity
    /// it carries, and any other call or plan are `false`. For an owner that
    /// admits an effect only under governed work, such as a checked MCP
    /// route that bypasses its server's access ceiling. The prepared check
    /// never leaves core, and the effect's own reviewed entry decides it
    /// again.
    #[must_use]
    pub fn is_prepared_for(
        &self,
        call: ToolCallView<'_>,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> bool {
        let (Some(_), Some(prepared)) = (self.work_authorization(), self.prepared_authorization())
        else {
            return false;
        };
        prepared
            .tool_dispatch_parts()
            .is_some_and(|(bound_call, bound_plan)| {
                std::ptr::eq(call.id, bound_call.id)
                    && std::ptr::eq(call.name, bound_call.name)
                    && std::ptr::eq(call.args, bound_call.args)
                    && std::ptr::eq(plan, bound_plan)
            })
    }

    /// For governed work, require that this dispatch was authorized for
    /// exactly the typed operation `target` its owner is about to perform
    /// for `call`: the binding-owned call (matched by identity) under a plan
    /// whose single owner-produced target equals `target`. An owner whose
    /// effect is selected by a typed target calls this immediately before
    /// the effect, so work authorized for another target, or for none (a
    /// wrapper that resolved its own target-less plan, which a base lineage
    /// would cover), never runs. The context is read as
    /// [`Self::check_tool_authorization`] reads it: one with neither work
    /// nor a prepared authorization is ungoverned and unaffected (`target`
    /// is not computed); one with only one of them is malformed.
    ///
    /// The owner sees only the context it is given, so the guarantee holds
    /// when every wrapper between the root dispatcher and the owner forwards
    /// `resolve_execution_plan` and `dispatch_resolved_with_context` with
    /// its context. The fenced dispatch refuses a governed comms or memory
    /// plan that carries no target of its owner's kind, which a wrapper that
    /// does not forward the owner's plan resolution produces.
    ///
    /// # Errors
    /// Governed work authorized for another call, another target or none,
    /// and a context with work but no prepared authorization (or the
    /// reverse), refuse as malformed facts; an error from `target` is
    /// returned as is.
    pub fn require_authorized_operation_target(
        &self,
        call: ToolCallView<'_>,
        target: impl FnOnce() -> Result<crate::ToolOperationTarget, crate::ToolError>,
    ) -> Result<(), crate::ToolError> {
        let prepared = match (self.work_authorization(), self.prepared_authorization()) {
            (None, None) => return Ok(()),
            (Some(_), Some(prepared)) => prepared,
            _ => return Err(governed_malformed()),
        };
        let target = target()?;
        let (bound_call, bound_plan) = prepared
            .tool_dispatch_parts()
            .ok_or_else(governed_malformed)?;
        let mut bound_targets = bound_plan
            .owner_witnesses()
            .iter()
            .filter_map(crate::ToolExecutionOwnerWitness::operation_target);
        let authorized = std::ptr::eq(call.id, bound_call.id)
            && std::ptr::eq(call.name, bound_call.name)
            && std::ptr::eq(call.args, bound_call.args)
            && bound_targets.next() == Some(&target)
            && bound_targets.next().is_none();
        if authorized {
            Ok(())
        } else {
            Err(governed_malformed())
        }
    }

    /// The governed operation this dispatch prepared, for a trusted native
    /// owner that checks it again later (see [`GovernedDispatchOperation`]).
    /// `call` must be this dispatch's actual borrowed call: it is matched by
    /// identity against the binding-owned call, and the binding's retained
    /// plan is used internally (no caller-supplied plan).
    ///
    /// # Errors
    /// An ungoverned or default context, or any other call, refuses.
    pub fn governed_operation(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<GovernedDispatchOperation, crate::ToolError> {
        let (Some(work), Some(prepared)) =
            (self.work_authorization(), self.prepared_authorization())
        else {
            return Err(governed_malformed());
        };
        let (bound_call, _) = prepared
            .tool_dispatch_parts()
            .ok_or_else(governed_malformed)?;
        if !std::ptr::eq(call.id, bound_call.id)
            || !std::ptr::eq(call.name, bound_call.name)
            || !std::ptr::eq(call.args, bound_call.args)
        {
            return Err(governed_malformed());
        }
        Ok(GovernedDispatchOperation {
            context: self.clone(),
            work: work.clone(),
        })
    }

    /// The typed delegate attribution of this dispatch's own prepared
    /// builtin call, for its helper operation in `role` (see
    /// [`crate::authorization::ToolDelegateAttribution`]). `call` must be
    /// this dispatch's actual borrowed call, matched by identity against the
    /// binding-owned call, and the call's plan must carry exactly the
    /// builtin operation `role` requires, as its registration owner
    /// produced it. An ungoverned dispatch has none (`Ok(None)`). It grants
    /// nothing: the helper operation is decided by its own owners.
    ///
    /// # Errors
    /// A governed context without its prepared check (or a check without its
    /// work), any other call, or a plan with no target, another owner's
    /// target or more than one target is malformed facts.
    pub fn tool_delegate_attribution(
        &self,
        call: ToolCallView<'_>,
        role: crate::authorization::ToolDelegateRole,
    ) -> Result<Option<crate::authorization::ToolDelegateAttribution>, crate::ToolError> {
        let (work, prepared) = match (self.work_authorization(), self.prepared_authorization()) {
            (None, None) => return Ok(None),
            (Some(work), Some(prepared)) => (work, prepared),
            _ => return Err(governed_malformed()),
        };
        let (bound_call, _) = prepared
            .tool_dispatch_parts()
            .ok_or_else(governed_malformed)?;
        if !std::ptr::eq(call.id, bound_call.id)
            || !std::ptr::eq(call.name, bound_call.name)
            || !std::ptr::eq(call.args, bound_call.args)
        {
            return Err(governed_malformed());
        }
        crate::authorization::ToolDelegateAttribution::for_dispatch(prepared, work, role)
            .map(Some)
            .map_err(crate::ToolError::from)
    }

    /// An owned, single-use entry for a leaf whose effect runs on a worker
    /// (for example a queued blocking task). It re-derives the exact
    /// binding-owned call when entered, so it is `'static` and `Send`, and it
    /// is consumed by value, so it can enter at most once. A reviewed ticket
    /// entered after the admitting dispatch ended refuses; any ticket refuses
    /// once its run is no longer current.
    pub fn reviewed_entry_ticket(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
    ) -> Result<ReviewedEntryTicket, crate::ToolError> {
        // Ungoverned dispatch: nothing to stage or spend, so no clone.
        let Some(prepared) = self.prepared_authorization() else {
            return Ok(ReviewedEntryTicket(None));
        };
        let malformed = || crate::ToolError::AuthorizationRefused {
            refusal: crate::authorization::OperationRefused::new(
                crate::authorization::OperationRefusalKind::MalformedFacts,
            ),
        };
        let (bound_call, bound_plan) = prepared.tool_dispatch_parts().ok_or_else(malformed)?;
        if !std::ptr::eq(call.id, bound_call.id)
            || !std::ptr::eq(call.name, bound_call.name)
            || !std::ptr::eq(call.args, bound_call.args)
            || plan.is_some_and(|actual| !std::ptr::eq(actual, bound_plan))
        {
            return Err(malformed());
        }
        Ok(ReviewedEntryTicket(Some(GovernedEntryTicket {
            context: self.clone(),
            with_plan: plan.is_some(),
        })))
    }

    /// The single consuming step at the leaf that performs the effect.
    ///
    /// Call exactly once, after argument and preparation work and any waits,
    /// immediately before the body or handoff, with the context the leaf was
    /// handed. It runs [`Self::observe_tool_entry_at`] at the effect boundary
    /// ([`crate::authorization::OperationEntryBoundary::ToolEffect`]:
    /// staging, the final authorization check and the review check point),
    /// then spends a
    /// retained review allow once for the exact entering decision. Nothing
    /// external runs between that spend and the caller's effect; the review
    /// `Used` projection is delivered after dispatch returns. A second call
    /// for the same operation refuses. R1 and ungoverned calls behave exactly
    /// like `observe_tool_entry`. Use the returned context when present.
    pub fn enter_reviewed_effect(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
    ) -> Result<Option<Self>, crate::ToolError> {
        // An in-dispatch effect ends with its dispatch, whose guard defers
        // `Used`; the custody is not needed beyond this call.
        self.enter_reviewed_effect_held(call, plan)
            .map(|(entering, _custody)| entering)
    }

    /// [`Self::enter_reviewed_effect`], also returning custody of the
    /// entered effect for a leaf that may outlive its dispatch. Hand the
    /// custody to the owner that keeps the effect going (for example a send
    /// task that outlives the caller) and hold it until that effect settles.
    pub fn enter_reviewed_effect_held(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
    ) -> Result<(Option<Self>, crate::approval::review::ReviewedEntryCustody), crate::ToolError>
    {
        let entering = self.observe_tool_entry_at(
            call,
            plan,
            crate::authorization::OperationEntryBoundary::ToolEffect,
        )?;
        self.spend_review_entry(entering)
    }

    /// [`Self::enter_reviewed_effect_held`] whose staging waits out an owner
    /// change in progress on the work's own publication within the dispatch
    /// attempt's own bounds, as the root staging does
    /// ([`Self::stage_tool_entry_after_owner_change`]). Nothing is spent
    /// before or during a wait: the review is spent once, after the staging.
    async fn enter_reviewed_effect_held_after_owner_change(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
    ) -> Result<(Option<Self>, crate::approval::review::ReviewedEntryCustody), crate::ToolError>
    {
        let entering = self
            .stage_tool_entry_after_owner_change(
                call,
                plan,
                crate::authorization::OperationEntryBoundary::ToolEffect,
            )
            .await
            .map_err(crate::approval::review::ReviewAdmissionRefusal::into_settled)?;
        self.spend_review_entry(entering)
    }

    /// Spend this dispatch's retained review once for the decision staging
    /// returned (`entering`, or this context's own when staging refreshed
    /// nothing). R1 and ungoverned entries spend nothing.
    fn spend_review_entry(
        &self,
        entering: Option<Self>,
    ) -> Result<(Option<Self>, crate::approval::review::ReviewedEntryCustody), crate::ToolError>
    {
        let current = entering.as_ref().unwrap_or(self);
        let custody = match (self.review_entry.as_ref(), current.prepared_authorization()) {
            (Some(entry), Some(check)) => entry.consume_held(check)?,
            _ => crate::approval::review::ReviewedEntryCustody::none(),
        };
        Ok((entering, custody))
    }

    pub fn work_authorization(&self) -> Option<&crate::WorkAuthorizationContext> {
        self.work_authorization.as_ref()
    }

    /// The tool time this fenced dispatch has left now, measured from the
    /// dispatch's start against its plan's effective timeout. `None` without
    /// a tool timeout, and for a context outside a fenced dispatch. A tool
    /// that waits inside its body (for example out an owner change after a
    /// refused delegate request) bounds the wait by it, so the wait never
    /// turns the refusal into the dispatch's timeout.
    pub fn remaining_tool_time(&self) -> Option<std::time::Duration> {
        self.staging_wait
            .as_ref()
            .and_then(|wait| wait.deadline.remaining())
    }

    /// Identity of the staged run this dispatch belongs to, when its native
    /// owner recorded one. A producer that outlives the call (fork_off, a
    /// council) retains it so the completion can resume exactly this work.
    pub fn retained_work(&self) -> Option<&crate::retained_work::RetainedWorkIdentity> {
        self.work_authorization
            .as_ref()
            .and_then(crate::WorkAuthorizationContext::retained_work)
    }

    /// Retain one check with the exact immutable operation prepared upstream.
    #[must_use]
    pub(crate) fn with_prepared_authorization(
        mut self,
        check: crate::authorization::PreparedOperationCheck,
    ) -> Self {
        self.prepared_authorization = Some(check);
        self
    }

    /// Test-only: this context carrying `check` as its dispatch's prepared
    /// operation, as the fenced dispatch entry attaches it, so an owner
    /// outside core can exercise its governed dispatch path.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    pub fn __test_with_prepared_authorization(
        self,
        check: crate::authorization::PreparedOperationCheck,
    ) -> Self {
        self.with_prepared_authorization(check)
    }

    /// Prepared payload custody stays inside core. An adapter receives the
    /// exact borrowed call during dispatch, not a transferable payload handle.
    pub(crate) fn prepared_authorization(
        &self,
    ) -> Option<&crate::authorization::PreparedOperationCheck> {
        self.prepared_authorization.as_ref()
    }

    /// Check the actual borrowed tool call after an adapter's preparation waits.
    /// Use the returned context when present; it retains a refreshed decision
    /// for this same immutable call. `None` needs no replacement allocation.
    /// This does not expose the binding-owned payload for later replay.
    ///
    /// ```compile_fail
    /// use meerkat_core::ToolDispatchContext;
    /// let captured = ToolDispatchContext::default();
    /// let _ = captured.prepared_authorization();
    /// ```
    pub fn check_tool_authorization(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
    ) -> Result<Option<Self>, crate::ToolError> {
        if let Some(control) = self.tool_application_control() {
            control.revalidate().map_err(crate::ToolError::from)?;
        }
        match (self.work_authorization(), self.prepared_authorization()) {
            (None, None) => Ok(None),
            (Some(work), Some(prepared)) => prepared
                .current_tool(work, call, plan, self.run_id())
                .map(|current| {
                    if current.same_check(prepared) {
                        None
                    } else {
                        Some(self.clone().with_prepared_authorization(current))
                    }
                })
                .map_err(crate::ToolError::from),
            _ => Err(crate::ToolError::AuthorizationRefused {
                refusal: crate::authorization::OperationRefused::new(
                    crate::authorization::OperationRefusalKind::MalformedFacts,
                ),
            }),
        }
    }

    /// Record an entry attempt, then check actual currentness after that
    /// synchronous staging work. Adapters call this after their own awaits.
    /// Several adapter boundaries may observe the same operation ID; these
    /// observations do not count physical effects or authorize another call.
    /// The entry names the dispatch staging boundary
    /// ([`crate::authorization::OperationEntryBoundary::ToolDispatch`]); a
    /// staging point with an identity of its own uses
    /// [`Self::observe_tool_entry_at`].
    pub fn observe_tool_entry(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
    ) -> Result<Option<Self>, crate::ToolError> {
        self.observe_tool_entry_at(
            call,
            plan,
            crate::authorization::OperationEntryBoundary::ToolDispatch,
        )
    }

    /// [`Self::observe_tool_entry`] at the named `boundary`, so the entries
    /// that several boundaries record for one operation stay distinguishable.
    pub fn observe_tool_entry_at(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
        boundary: crate::authorization::OperationEntryBoundary,
    ) -> Result<Option<Self>, crate::ToolError> {
        self.stage_tool_entry(call, plan, boundary)
            .map_err(crate::approval::review::ReviewAdmissionRefusal::into_settled)
    }

    /// [`Self::observe_tool_entry`] at the fenced dispatch's own root
    /// staging, before any dispatcher code ran, waiting out an owner change
    /// in progress as [`Self::stage_tool_entry_after_owner_change`] does. A
    /// retained model review that a completed owner publication change
    /// superseded comes back typed, for the dispatch's one supersession
    /// decision, instead of settled; every other staging (adapters, the
    /// effect leaf) settles it. The entry is recorded before the check point
    /// sees the supersession, so an operation that is reviewed once more
    /// records this boundary twice.
    pub(crate) async fn stage_root_tool_entry(
        &self,
        call: ToolCallView<'_>,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<Option<Self>, crate::approval::review::ReviewAdmissionRefusal> {
        self.stage_tool_entry_after_owner_change(
            call,
            Some(plan),
            crate::authorization::OperationEntryBoundary::ToolDispatch,
        )
        .await
    }

    /// [`Self::observe_tool_entry_at`] for a staging point on the fenced
    /// dispatch chain that stages again after awaits of its own (the
    /// call-level execution policy gate, after its dispatch admission and
    /// consequence policy): an owner change in progress meanwhile is waited
    /// out within the dispatch attempt's own bounds, as at the root staging.
    pub(crate) async fn observe_tool_entry_after_owner_change_at(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
        boundary: crate::authorization::OperationEntryBoundary,
    ) -> Result<Option<Self>, crate::ToolError> {
        self.stage_tool_entry_after_owner_change(call, plan, boundary)
            .await
            .map_err(crate::approval::review::ReviewAdmissionRefusal::into_settled)
    }

    /// The one waiting entry staging: stage at `boundary`
    /// ([`Self::stage_tool_entry`]); while that refuses to rebuild because
    /// an owner change is still in progress on the work's own publication,
    /// wait for the change within the dispatch attempt's [`EntryStagingWait`]
    /// and stage again. The review check point runs on the decision staged
    /// after the wait, so no verdict is reused. A context outside a fenced
    /// dispatch carries no waiting state, and its refusal takes the ordinary
    /// path.
    async fn stage_tool_entry_after_owner_change(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
        boundary: crate::authorization::OperationEntryBoundary,
    ) -> Result<Option<Self>, crate::approval::review::ReviewAdmissionRefusal> {
        loop {
            let result = self.stage_tool_entry(call, plan, boundary);
            if let Err(crate::approval::review::ReviewAdmissionRefusal::Settled(
                crate::ToolError::AuthorizationRefused { refusal, .. },
            )) = &result
                && refusal.kind() == crate::authorization::OperationRefusalKind::ReprepareRequired
                && let Some(work) = self.work_authorization()
                && let Some(staging) = self.staging_wait.as_ref()
                && let Some(wait) = staging
                    .wait_for_settled_change(
                        work,
                        staging_wait_site(boundary),
                        self.prepared_authorization()
                            .map(|prepared| &prepared.binding().facts().operation_id),
                    )
                    .await
                && wait.settled()
            {
                continue;
            }
            return result;
        }
    }

    /// The one entry staging: record an entry attempt at `boundary`, check
    /// actual currentness after it, then run the review check point with the
    /// decision staging returned.
    fn stage_tool_entry(
        &self,
        call: ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
        boundary: crate::authorization::OperationEntryBoundary,
    ) -> Result<Option<Self>, crate::approval::review::ReviewAdmissionRefusal> {
        let first = self.check_tool_authorization(call, plan)?;
        let current = first.as_ref().unwrap_or(self);
        if let Some(prepared) = current.prepared_authorization() {
            prepared
                .observe_entry(boundary)
                .map_err(crate::ToolError::from)?;
        }
        let final_context = current.check_tool_authorization(call, plan)?;
        let entering = final_context.or(first);
        // Non-consuming review check point with the decision staging returned.
        // Repeatable: root and wrapper staging both run it; only the effect
        // leaf spends (see `enter_reviewed_effect`).
        self.check_review(entering.as_ref().unwrap_or(self))?;
        Ok(entering)
    }

    /// Keep an audit staging failure beside the original dispatch result.
    /// This observation makes no physical completion claim for an async job.
    pub fn observe_tool_outcome(
        &self,
        effect_kind: crate::LiveBridgeEffectKind,
        result: Result<crate::ops::ToolDispatchOutcome, crate::ToolError>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
        let Some(prepared) = self.prepared_authorization() else {
            return result;
        };
        if prepared
            .observe_outcome(
                crate::authorization::OperationObservedOutcome::from_tool_dispatch(&result),
            )
            .is_ok()
        {
            return result;
        }
        let failure = crate::ops::ToolDispatchSettlementFailure {
            admission_source: crate::ops::ToolDispatchAdmissionSource::AuthorizationAudit,
            effect_kind,
            physical_outcome: crate::LiveBridgeEffectOutcome::Unknown,
            failure_kind:
                crate::ops::ToolDispatchTerminalErrorKind::OperationObservationUnavailable,
        };
        match result {
            Ok(mut outcome) => {
                outcome.extend_settlement_failures(vec![failure]);
                Ok(outcome)
            }
            Err(error) => Err(error.with_settlement_failures(vec![failure])),
        }
    }

    #[must_use]
    /// Set the turn-scoped dispatch metadata. The peer reply context under
    /// [`crate::comms::COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY`] is parsed here,
    /// once; [`Self::peer_reply_deliveries`] reads the typed result.
    pub fn with_turn_metadata(mut self, metadata: BTreeMap<String, serde_json::Value>) -> Self {
        self.peer_replies = TurnPeerReplies::from_turn_metadata(&metadata);
        self.turn_metadata = metadata;
        self
    }

    pub fn turn_metadata(&self, key: &str) -> Option<&serde_json::Value> {
        self.turn_metadata.get(key)
    }

    /// Record the reply capabilities of peer deliveries written into the
    /// running turn at a model boundary. Typed and infallible: each one is
    /// added unless the same sender's delivery with the same id is already
    /// recorded ([`crate::comms::PeerReplyDispatchContext::insert_delivery`]).
    pub(crate) fn record_boundary_peer_replies(
        &mut self,
        deliveries: impl IntoIterator<Item = crate::comms::PeerReplyCapability>,
    ) {
        for delivery in deliveries {
            self.peer_replies.deliveries.insert_delivery(delivery);
        }
    }

    /// Every peer delivery this turn can answer with `reply_to_peer`: the
    /// turn-start deliveries the runtime minted under
    /// [`crate::comms::COMMS_PEER_REPLY_DISPATCH_CONTEXT_KEY`] (parsed once,
    /// by [`Self::with_turn_metadata`]), then the deliveries written into the
    /// running turn at model boundaries. Each (sender, delivery id) appears
    /// once.
    ///
    /// # Errors
    ///
    /// [`crate::comms::PeerReplyContextError::Malformed`] when the turn-start
    /// value does not decode. It fails closed instead of being coerced or
    /// ignored.
    pub fn peer_reply_deliveries(
        &self,
    ) -> Result<&crate::comms::PeerReplyDispatchContext, crate::comms::PeerReplyContextError> {
        match &self.peer_replies.malformed {
            Some(error) => Err(error.clone()),
            None => Ok(&self.peer_replies.deliveries),
        }
    }

    pub fn current_turn(&self) -> Option<&CurrentTurnContent> {
        self.current_turn.as_ref()
    }

    /// Bind the runtime-owned durable identity of the turn being dispatched.
    ///
    /// Standalone callers may leave this absent. Durable execution owners must
    /// fail closed rather than minting replacement identity at dispatch time.
    #[must_use]
    pub fn with_runtime_identity(
        mut self,
        origin_session_id: crate::types::SessionId,
        interaction_lineage_id: Option<crate::interaction::InteractionId>,
    ) -> Self {
        self.origin_session_id = Some(origin_session_id);
        self.interaction_lineage_id = interaction_lineage_id;
        self
    }

    pub fn origin_session_id(&self) -> Option<&crate::types::SessionId> {
        self.origin_session_id.as_ref()
    }

    pub const fn interaction_lineage_id(&self) -> Option<crate::interaction::InteractionId> {
        self.interaction_lineage_id
    }

    pub fn run_id(&self) -> Option<&crate::RunId> {
        self.run_id.as_ref()
    }

    pub(crate) fn bind_run_id(&mut self, run_id: crate::RunId) {
        self.run_id = Some(run_id);
        self.round = None;
    }

    /// Zero-based index of the run's round this dispatch belongs to, when the
    /// agent loop bound one.
    pub const fn round(&self) -> Option<u64> {
        self.round
    }

    pub(crate) fn bind_round(&mut self, round: u64) {
        self.round = Some(round);
    }

    /// The round's calls are dispatched and their outcomes collected: a call
    /// the host dispatches outside a round carries no dedupe key.
    pub(crate) fn clear_round(&mut self) {
        self.round = None;
    }

    /// This context for a call the host dispatches itself (an external
    /// call): it belongs to no round of the agent's loop, so it carries no
    /// dedupe key, whatever round the agent bound last.
    #[must_use]
    pub(crate) fn outside_round(mut self) -> Self {
        self.round = None;
        self
    }

    /// The deduplication key of one call of this dispatch's round.
    ///
    /// A durable run that resumes after a crash dispatches the calls of a
    /// round whose results were never committed again, under the same call
    /// ids, so the same call always carries the same key. Delivery is at
    /// least once: a tool with external effects that must not repeat consults
    /// this key and deduplicates on all of it. `None` outside an agent-loop
    /// tool round (no run or no round bound) and for arguments that are not
    /// JSON.
    ///
    /// An in-process tool reads it here. Core's MCP client sends it as the
    /// `tools/call` `_meta` entry [`crate::TOOL_CALL_DEDUPE_KEY_META_KEY`],
    /// and a callback tool receives it as the `dedupe_key` of its request or
    /// pending call.
    #[must_use]
    pub fn tool_call_dedupe_key(
        &self,
        call: crate::ToolCallView<'_>,
    ) -> Option<crate::ToolCallDedupeKey> {
        crate::ToolCallDedupeKey::new(
            self.run_id.clone()?,
            self.round?,
            call.id,
            call.name,
            call.args,
        )
        .ok()
    }

    /// Streaming-only liveness surface minted by the canonical supervisor.
    ///
    /// Fast and detached dispatch contexts carry no streaming surface. A tool
    /// that declared `Streaming` must fail closed if this is absent rather than
    /// fabricating a progress sink or cancellation authority.
    pub const fn streaming(&self) -> Option<&crate::ToolStreamingDispatchContext> {
        self.streaming.as_ref()
    }

    pub(crate) fn with_streaming(mut self, streaming: crate::ToolStreamingDispatchContext) -> Self {
        self.streaming = Some(streaming);
        self
    }

    /// Bind one exact live bridge operation's generated dispatch authority.
    #[must_use]
    pub fn with_live_bridge_admission(
        mut self,
        admission: LiveBridgeToolDispatchAdmission,
    ) -> Self {
        self.live_bridge_admission = Some(admission);
        self
    }

    #[must_use]
    pub fn live_bridge_admission(&self) -> Option<&LiveBridgeToolDispatchAdmission> {
        self.live_bridge_admission.as_ref()
    }

    pub fn current_turn_image(
        &self,
        image_ref: CurrentTurnImageRef,
    ) -> Option<&crate::types::ContentBlock> {
        self.current_turn
            .as_ref()
            .and_then(|current_turn| current_turn.image(image_ref))
    }
}

/// Typed reference to an image in the current admitted turn.
///
/// The wrapped index addresses the turn's *filtered image stream*, not the
/// raw block list: ref `N` designates the `(N + 1)`-th image block of the
/// current turn, skipping non-image blocks (so ref `0` is the first image
/// even when text blocks precede it).
///
/// The field is private. In-process code mints refs only via
/// [`CurrentTurnContent::image_ref`], which returns a ref only when the
/// referenced image exists. Wire ingress (e.g. the comms `image_ref` tool
/// input) deserializes a bare JSON integer directly into this type via
/// `#[serde(transparent)]` — that is the sanctioned parse-at-ingress path,
/// and resolution through [`CurrentTurnContent::image`] still validates
/// existence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CurrentTurnImageRef(usize);

impl std::fmt::Display for CurrentTurnImageRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// Multimodal content from the currently admitted turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentTurnContent {
    blocks: Vec<crate::types::ContentBlock>,
}

impl CurrentTurnContent {
    pub fn new(blocks: Vec<crate::types::ContentBlock>) -> Self {
        Self { blocks }
    }

    pub fn blocks(&self) -> &[crate::types::ContentBlock] {
        &self.blocks
    }

    /// Mint a typed reference to the `n`-th image of this turn's filtered
    /// image stream. Returns `Some` only when that image exists, so every
    /// in-process [`CurrentTurnImageRef`] is resolvable at mint time.
    pub fn image_ref(&self, n: usize) -> Option<CurrentTurnImageRef> {
        self.images().nth(n).map(|_| CurrentTurnImageRef(n))
    }

    pub fn image(&self, image_ref: CurrentTurnImageRef) -> Option<&crate::types::ContentBlock> {
        self.images().nth(image_ref.0)
    }

    fn images(&self) -> impl Iterator<Item = &crate::types::ContentBlock> {
        self.blocks
            .iter()
            .filter(|block| matches!(block, crate::types::ContentBlock::Image { .. }))
    }
}

/// Completion notice for a detached background operation, projected from
/// canonical ops-lifecycle terminal state plus dispatcher-owned display metadata.
///
/// This is a rebuildable projection (INV-003), not authoritative state.
/// Terminal class and timing come from `OperationLifecycleSnapshot` (INV-001).
/// Shell-projected detail is supplementary display only (INV-002).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetachedOpCompletion {
    /// App-facing job identifier (the control noun for surfaces).
    pub job_id: String,
    /// Operation kind from canonical ops-lifecycle.
    pub kind: OperationKind,
    /// Terminal status from canonical ops-lifecycle.
    pub status: OperationStatus,
    /// Terminal outcome from canonical ops-lifecycle.
    pub terminal_outcome: Option<OperationTerminalOutcome>,
    /// Canonical display label from ops-lifecycle snapshot.
    pub display_name: String,
    /// Dispatcher-projected summary (exit code, output tail). Display only.
    pub detail: String,
    /// Monotonic elapsed millis from ops-lifecycle snapshot.
    pub elapsed_ms: Option<u64>,
}

/// Dispatcher binding capabilities — what optional bindings this dispatcher supports.
///
/// Returned by [`AgentToolDispatcher::capabilities`]. Replaces individual
/// `supports_*` boolean methods with a single structured query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DispatcherCapabilities {
    /// Whether `bind_ops_lifecycle` is implemented.
    pub ops_lifecycle: bool,
}

/// Result of a dispatcher binding operation.
///
/// Distinguishes "binding was applied" from "binding was skipped" so callers
/// can decide whether to wire downstream side effects (e.g. bridge tasks).
///
/// **Semantics (decision 11 — supported/best-effort/rejected):**
/// - `Ok(Bound(d))` = **supported** — binding succeeded, side effects should be wired
/// - `Ok(Skipped(d))` = **best-effort** - no inner dispatcher needed the binding, dispatcher unchanged
/// - `Err(SharedOwnership)` from a wrapper = **rejected** - an inner dispatcher that needs the
///   binding is shared, so it cannot be rebound; it is never left silently unbound
/// - `Err(SharedOwnership)` = **rejected** — outer wrapper is shared, caught by factory pre-check
/// - `Err(Unsupported)` = **rejected** — type doesn't support this binding, caught by `capabilities()`
pub enum BindOutcome {
    /// Binding was applied. The dispatcher was rebound.
    Bound(Arc<dyn AgentToolDispatcher>),
    /// Binding was skipped: no inner dispatcher needed it. The returned
    /// dispatcher is unchanged but safe to use.
    Skipped(Arc<dyn AgentToolDispatcher>),
}

impl BindOutcome {
    /// Extract the dispatcher, regardless of bind status.
    pub fn into_dispatcher(self) -> Arc<dyn AgentToolDispatcher> {
        match self {
            Self::Bound(d) | Self::Skipped(d) => d,
        }
    }

    /// Whether the binding was actually applied.
    pub fn was_bound(&self) -> bool {
        matches!(self, Self::Bound(_))
    }
}

/// A resource an agent's construction handed to the agent, released by the
/// agent's owner when it ends the agent.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentOwnedRelease: Send + Sync {
    /// Release the resource and await its release. Idempotent.
    async fn release(&self);
}

/// Trait for tool dispatchers
///
/// An operation refusal certified
/// [`crate::authorization::RefusalDispatch::NotDispatched`] speaks only for
/// the tool code below the core component that certified it (the fenced
/// dispatch, or the execution policy gate for its inner dispatcher). A
/// dispatcher layer that runs tool code of its own before delegating to such
/// a component forwards the refusal as `OperationRefused::new(refusal.kind())`,
/// which makes no claim.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;

    /// Resolve a host UI request against the leaf that owns the original tool.
    /// `invocation` is read from the committed result by the session owner.
    /// A leaf may serve protocol data or select a tool call; it cannot bypass
    /// core's normal app scope and execution path for that call.
    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        _request: &crate::ToolApplicationRequest,
        _invocation: &serde_json::Value,
        _context: &ToolDispatchContext,
    ) -> Result<crate::tool_application::ToolApplicationResolution, crate::ToolError> {
        Err(crate::ToolError::access_denied(source_tool))
    }

    /// Query exact catalog support for this dispatcher.
    ///
    /// Dispatchers report `exact_catalog=true` only when `tool_catalog()`
    /// returns the exact precedence-resolved winner registry for the plane
    /// they own. Wrappers that cannot prove exactness must leave this false.
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities::default()
    }

    /// Return the precedence-resolved tool catalog for this dispatcher.
    ///
    /// The default implementation mirrors `tools()` as a visible-only inline
    /// catalog. Callers must gate any deferred-catalog behavior on
    /// `tool_catalog_capabilities().exact_catalog`.
    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.tools()
            .iter()
            .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
            .collect::<Vec<_>>()
            .into()
    }

    /// Declared world-mutation semantics for one tool name this dispatcher
    /// owns.
    ///
    /// The default is [`crate::ToolMutationClass::Unknown`]: a dispatcher that
    /// has not made a declaration is never treated as read-only. Leaf
    /// dispatchers declare their own tools; wrappers and composites forward to
    /// the child that owns the name so
    /// [`crate::tool_execution_policy::ExecutionPolicyGatedDispatcher`] reads
    /// the owner's declaration and not the wrapper's default.
    fn tool_mutation_class(&self, _tool_name: &str) -> crate::ToolMutationClass {
        crate::ToolMutationClass::Unknown
    }

    /// Provider-neutral effect class for one tool owned by this dispatcher.
    /// Unknown or undeclared tools fail closed as external IO.
    fn live_bridge_effect_kind(&self, _tool_name: &str) -> crate::LiveBridgeEffectKind {
        crate::LiveBridgeEffectKind::ExternalIo
    }

    /// Whether this dispatcher carries review to the tool's physical entry
    /// (its leaf calls `ToolDispatchContext::enter_reviewed_effect` exactly
    /// once before the body or handoff). Wrappers forward the owning leaf's
    /// declaration. The default is fail-closed: a required review for an
    /// undeclared tool settles as local unavailable feedback.
    fn review_entry_support(
        &self,
        _tool_name: &str,
    ) -> crate::approval::review::ReviewEntrySupport {
        crate::approval::review::ReviewEntrySupport::Unsupported
    }

    /// Live generation for one logical tool binding.
    ///
    /// Static dispatchers keep the default zero epoch. Mutable authorities
    /// must override this and advance it for every replacement, including an
    /// identical-metadata A→B replacement.
    fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
        0
    }

    /// Snapshot the current live logical binding advertised for `tool_name`.
    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<crate::EphemeralToolBindingFingerprint, crate::ToolExecutionResolutionError> {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        Ok(crate::ephemeral_tool_catalog_binding_fingerprint(entry)
            .with_live_authority(0, self.execution_binding_epoch(tool_name)))
    }

    /// Resolve the exact execution class and deadline chain before dispatch.
    ///
    /// The default uses this dispatcher's effective catalog, so wrappers that
    /// filter or select winners apply the same decision to declaration and
    /// resolution. Hybrid tools may override this method to inspect typed
    /// arguments while preserving the catalog contract as the upper bound.
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &ToolDispatchContext,
        resolution_context: &crate::ToolExecutionResolutionContext,
    ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?;
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(crate::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason,
            });
        }
        entry
            .execution
            .resolve_default(resolution_context.deadlines().clone())
            .map_err(crate::ToolExecutionResolutionError::from)
    }

    /// Validate a resolved plan against both the caller-owned deadline prefix
    /// and this dispatcher's live advertised catalog contract.
    ///
    /// This is the mandatory root seam after argument-sensitive resolution:
    /// hybrid tools may select any advertised mode, while an override cannot
    /// return a mode or mode-derived facet absent from the effective catalog.
    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution_context: &crate::ToolExecutionResolutionContext,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<(), crate::ToolExecutionResolutionError> {
        resolution_context.validate_resolved_plan(plan)?;
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?;
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(crate::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason,
            });
        }
        entry
            .execution
            .validate_resolved_plan(plan)
            .map_err(crate::ToolExecutionResolutionError::from)
    }

    /// Return non-draining pending source names for exact-catalog discovery.
    ///
    /// Pending sources are catalog-level discovery metadata rather than
    /// provider-visible tools. The default implementation reports none.
    fn pending_catalog_sources(&self) -> Arc<[String]> {
        Arc::from([])
    }

    /// Execute a tool call, returning the transcript result and any async operations.
    ///
    /// The `ToolDispatchOutcome` separates transcript data (`result`) from
    /// execution metadata (`async_ops`). Most tools return no async ops;
    /// use `ToolDispatchOutcome::from(result)` for synchronous tools.
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError>;

    /// Execute a tool call with the current turn's typed dispatch context.
    ///
    /// Most tools do not need turn-local context and inherit the plain
    /// `dispatch` behavior. Context-sensitive surfaces override this method
    /// rather than reaching into session history or prompt text.
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        _context: &ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        self.dispatch(call).await
    }

    /// Execute a previously resolved plan without re-selecting its mode.
    ///
    /// The default is deliberately a one-way lowering for Fast calls only.
    /// Streaming and Detached require an explicit mode owner; silently sending
    /// either through ordinary dispatch would erase their liveness, output,
    /// restart, and idempotency contracts.
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        match plan.mode() {
            crate::ToolExecutionMode::Fast => self.dispatch_with_context(call, context).await,
            crate::ToolExecutionMode::Streaming | crate::ToolExecutionMode::Detached => {
                Err(crate::error::ToolError::unavailable(
                    call.name,
                    crate::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
                ))
            }
        }
    }

    /// Poll for external tool updates from background operations (e.g. async MCP loading).
    ///
    /// The default implementation returns an empty update. Implementations that
    /// support background tool loading (like `McpRouterAdapter`) override this
    /// to drain completed results and report pending servers.
    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        ExternalToolUpdate::default()
    }

    /// Snapshot the live external tool-surface machine state, if supported.
    ///
    /// This is a hidden diagnostic surface for MeerkatMachine mapping work.
    /// Dispatchers that do not own dynamic external tool mutation should
    /// return `None`.
    fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        None
    }

    /// Query which optional bindings this dispatcher supports.
    fn capabilities(&self) -> DispatcherCapabilities {
        DispatcherCapabilities::default()
    }

    /// Bind a session-canonical ops registry into this dispatcher.
    ///
    /// Dispatchers that emit session-visible `AsyncOpRef`s must route those
    /// operation IDs into the bound registry. Under the identity-first Mob
    /// regime the owner binding passed here is the canonical bridge session
    /// binding, even though many compatibility surfaces still spell it
    /// `session_id`. Default returns Unsupported.
    fn bind_ops_lifecycle(
        self: Arc<Self>,
        _registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        _owner_bridge_session_id: crate::types::SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        Err(OpsLifecycleBindError::Unsupported)
    }

    /// Return the completion enrichment provider, if available.
    ///
    /// Dispatchers with shell job management return a provider that maps
    /// operation IDs to display details (job ID, status detail string).
    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>> {
        None
    }

    /// Bind a session-scoped MCP server lifecycle handle (Phase 5G / T5g).
    ///
    /// Dispatchers that manage per-server MCP handshake lifecycle (like
    /// `McpRouterAdapter`) use the handle to mirror connection state into
    /// the session's MeerkatMachine DSL. The default implementation is a
    /// no-op for dispatchers that have no MCP handshake to route.
    fn bind_mcp_server_lifecycle_handle(
        &self,
        _handle: Arc<dyn crate::handles::McpServerLifecycleHandle>,
    ) {
    }

    /// Bind the session-canonical external tool-surface handle.
    ///
    /// MCP dispatchers use this to route add/remove/reload/call lifecycle
    /// semantics through the session's MeerkatMachine DSL instead of their
    /// standalone compatibility projection. The default implementation is a
    /// no-op for dispatchers that do not own dynamic external tool surfaces.
    fn bind_external_tool_surface_handle(
        &self,
        _handle: Arc<dyn crate::handles::ExternalToolSurfaceHandle>,
    ) {
    }

    /// Bind the session's mob operator authority.
    ///
    /// The agent factory calls this on a session's external tools once the
    /// session's final mob operator authority is settled, before it composes
    /// them. `Some` is the session's effective-authority handle, which the
    /// agent updates through `apply_session_effects`; `None` means the
    /// session holds no mob operator authority. A dispatcher that admits mob
    /// operator calls for the session (a mob runtime's own tools for a
    /// member) admits against it, and a composite forwards it to every
    /// child. A host that wraps a session's external tools in its own
    /// dispatcher forwards it to the wrapped one. The default implementation
    /// is a no-op for dispatchers that admit no mob operator calls.
    fn bind_session_mob_tool_authority(
        &self,
        _authority: Option<Arc<std::sync::RwLock<crate::service::MobToolAuthorityContext>>>,
    ) {
    }
}

/// Resolve a plan against the exact live root dispatcher allocation.
///
/// The returned plan retains an ephemeral `Arc` lease to that allocation.
/// This makes reconstruction and allocator address reuse unforgeable without
/// serializing process-local authority or conflating it with durable job
/// fencing.
pub fn resolve_tool_execution_plan_fenced<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    call: ToolCallView<'_>,
    dispatch_context: &ToolDispatchContext,
    resolution_context: &crate::ToolExecutionResolutionContext,
) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
    let before = dispatcher.execution_binding_fingerprint(call.name)?;
    let plan = dispatcher.resolve_execution_plan(call, dispatch_context, resolution_context)?;
    if dispatcher.execution_binding_fingerprint(call.name)? != before {
        return Err(crate::ToolExecutionResolutionError::Unavailable {
            tool_name: call.name.to_string(),
            reason: crate::ToolUnavailableReason::ExecutionOwnerChanged,
        });
    }
    let witness = crate::ToolExecutionOwnerWitness::new(
        crate::tool_execution::ROOT_DISPATCHER_WITNESS,
        call.name,
        before,
    )
    .map_err(crate::ToolExecutionResolutionError::from)?;
    let plan = plan.with_owner_witness(witness)?;
    if let Some(binding) = dispatch_context.application_binding() {
        binding.validate_execution_plan(call.name, &plan)?;
    }
    plan.bind_root_dispatch(Arc::clone(dispatcher), call, resolution_context)
}

/// The plan still names this exact root allocation, the exact canonical call
/// and the tool's current execution owner.
fn validate_fenced_root_dispatch<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
    plan: &crate::ResolvedToolExecutionPlan,
) -> Result<(), crate::error::ToolError> {
    plan.validate_root_dispatch(dispatcher, call)?;
    if let Some(binding) = context.application_binding() {
        binding.validate_execution_plan(call.name, plan)?;
    }
    let witness = plan
        .owner_witness(crate::tool_execution::ROOT_DISPATCHER_WITNESS)
        .ok_or_else(|| {
            crate::error::ToolError::unavailable(
                call.name,
                crate::ToolUnavailableReason::ExecutionOwnerChanged,
            )
        })?;
    if witness.binding_fingerprint() != &dispatcher.execution_binding_fingerprint(call.name)? {
        return Err(crate::error::ToolError::unavailable(
            call.name,
            crate::ToolUnavailableReason::ExecutionOwnerChanged,
        ));
    }
    Ok(())
}

/// Under governed work, a comms or memory tool's plan must carry targets of
/// its owner's kind only: a peer delivery or the peer directory for comms,
/// a memory operation for memory. Those owners attach one to every plan
/// they resolve and require it again at their leaf, which they can do only
/// when every wrapper forwards the dispatch context to them. A plan without
/// one was resolved by a wrapper that did not forward the owner's plan
/// resolution (another owner's target, such as the one a registered bundle
/// attaches to a plan that names none, does not stand in for it), and that
/// wrapper may drop the context too, so the plan is refused here, before
/// preparation and before any wrapper runs. The source kind is the root
/// catalog entry's own provenance for the call.
fn require_owner_target_for_targeted_sources<T: AgentToolDispatcher + ?Sized>(
    dispatcher: &T,
    call: ToolCallView<'_>,
    plan: &crate::ResolvedToolExecutionPlan,
) -> Result<(), crate::error::ToolError> {
    let catalog = dispatcher.tool_catalog();
    let Some(provenance) = catalog
        .iter()
        .find(|entry| entry.tool.name == call.name)
        .and_then(|entry| entry.tool.provenance.as_ref())
    else {
        return Ok(());
    };
    let owners_kind: fn(&crate::ToolOperationTarget) -> bool = match &provenance.kind {
        crate::types::ToolSourceKind::Comms => |target| {
            matches!(
                target,
                crate::ToolOperationTarget::PeerSend { .. }
                    | crate::ToolOperationTarget::PeerDirectoryRead
            )
        },
        crate::types::ToolSourceKind::Memory => {
            |target| matches!(target, crate::ToolOperationTarget::MemoryOperation { .. })
        }
        _ => return Ok(()),
    };
    let mut targets = plan
        .owner_witnesses()
        .iter()
        .filter_map(crate::ToolExecutionOwnerWitness::operation_target)
        .peekable();
    let carries_its_owners_target = targets.peek().is_some() && targets.all(owners_kind);
    if carries_its_owners_target {
        Ok(())
    } else {
        Err(governed_malformed())
    }
}

/// When one fenced dispatch's tool timeout ends, measured from the dispatch's
/// start against its plan's effective timeout. Waits for an owner change in a
/// dispatch end before it, so a wait never turns a refusal into the tool's
/// timeout. A streaming tool's inactivity window is not part of this
/// deadline: the execution policy gate stages inside the streaming
/// supervisor, so a gate wait longer than the window left ends as the tool's
/// inactivity timeout.
#[derive(Debug, Clone, Copy)]
struct DispatchDeadline {
    ends: Option<crate::time_compat::DeadlineInstant>,
}

impl DispatchDeadline {
    fn start(plan: &crate::ResolvedToolExecutionPlan) -> Self {
        Self {
            ends: plan
                .effective_timeout()
                .map(|timeout| crate::time_compat::DeadlineInstant::now() + timeout),
        }
    }

    /// The tool time left now; `None` without a tool timeout.
    fn remaining(self) -> Option<std::time::Duration> {
        self.ends
            .map(|ends| ends.saturating_duration_since(crate::time_compat::DeadlineInstant::now()))
    }
}

/// The owner-change waiting state of one fenced dispatch attempt: its
/// [`crate::authorization::OwnerChangeChurn`] and the dispatch's tool
/// deadline. The attempt's post-verdict check and every entry staging of the
/// attempt (the root staging, and the execution policy gate's staging after
/// its own awaits) share it through the dispatch context, so the attempt's
/// bounds hold however many of them wait.
pub(crate) struct EntryStagingWait {
    churn: tokio::sync::Mutex<crate::authorization::OwnerChangeChurn>,
    deadline: DispatchDeadline,
}

impl EntryStagingWait {
    /// The waiting state of one attempt, whose waits go to `report`.
    fn new(
        deadline: DispatchDeadline,
        report: Option<crate::authorization::WaitReport>,
    ) -> Arc<Self> {
        Arc::new(Self {
            churn: tokio::sync::Mutex::new(crate::authorization::OwnerChangeChurn::reporting(
                report,
            )),
            deadline,
        })
    }

    /// Wait out an owner change in progress on `work`'s own publication
    /// within this attempt's bounds, before the tool deadline; see
    /// [`crate::authorization::OwnerChangeChurn::wait_for_settled_change`].
    async fn wait_for_settled_change(
        &self,
        work: &crate::WorkAuthorizationContext,
        site: &'static str,
        operation: Option<&crate::OperationId>,
    ) -> Option<crate::authorization::OwnerChangeWait> {
        self.churn
            .lock()
            .await
            .wait_for_settled_change(
                work.authorization().as_ref(),
                self.deadline.remaining(),
                site,
                operation,
            )
            .await
    }
}

/// The common prepared entry of a governed dispatch: prepare `binding` from
/// the work's current owners and check it. An owner change still in
/// progress on the work's own publication refuses every preparation until it
/// completes, so it is waited out (bounded by `churn`) and the same operation
/// prepared again.
async fn prepare_current_after_owner_change(
    work: &crate::WorkAuthorizationContext,
    binding: &crate::authorization::PreparedAuthorizationBinding,
    churn: &mut crate::authorization::OwnerChangeChurn,
    deadline: DispatchDeadline,
) -> Result<crate::authorization::PreparedOperationCheck, crate::ToolError> {
    loop {
        let result =
            crate::authorization::PreparedOperationCheck::prepare(work.clone(), binding.clone())
                .and_then(|prepared| prepared.current());
        if let Err(crate::OperationAuthorizationError::Refused(refusal)) = &result
            && refusal.kind() == crate::authorization::OperationRefusalKind::ReprepareRequired
            && let Some(wait) = churn
                .wait_for_settled_change(
                    work.authorization().as_ref(),
                    deadline.remaining(),
                    "tool_dispatch",
                    Some(&binding.facts().operation_id),
                )
                .await
            && wait.settled()
        {
            continue;
        }
        return result.map_err(crate::ToolError::from);
    }
}

/// The owner-change wait's debug site for an entry staging at `boundary`.
fn staging_wait_site(boundary: crate::authorization::OperationEntryBoundary) -> &'static str {
    match boundary {
        crate::authorization::OperationEntryBoundary::ToolExecutionPolicy => "tool_policy_entry",
        _ => "tool_entry",
    }
}

/// Whether a governed step answered a refusal to rebuild
/// (`ReprepareRequired`): the one answer a waiting step waits after.
fn refuses_to_rebuild<T>(answer: &Result<T, crate::ToolError>) -> bool {
    matches!(
        answer,
        Err(crate::ToolError::AuthorizationRefused { refusal })
            if refusal.kind() == crate::authorization::OperationRefusalKind::ReprepareRequired
    )
}

/// The owner-change waiting state of one waiting step of a dispatch's
/// governed operation (the `_after_owner_change` forms of
/// [`GovernedDispatchOperation`] and [`EnteredDispatchOperation`]): a fresh
/// [`crate::authorization::OwnerChangeChurn`] reporting as the call's
/// ([`ToolDispatchContext::owner_change_churn`]), whose one deadline is set
/// at the step's first wait, bounded by the call's remaining tool time read
/// then, and whose free rebuilds bound every retry of the step.
///
/// It is asked only after a synchronous attempt returned, so nothing that
/// attempt took (the work owners' exclusion, an owner lock, a run-custody
/// read, a review step's admission) is held across a wait, and no wait runs
/// inside a work owner's exclusion. Shell synchronization only: it waits
/// and asks again, and every attempt is decided afresh by the owners.
struct StepOwnerChangeWait<'a> {
    context: &'a ToolDispatchContext,
    work: &'a crate::WorkAuthorizationContext,
    site: &'static str,
    churn: crate::authorization::OwnerChangeChurn,
}

impl<'a> StepOwnerChangeWait<'a> {
    fn new(
        context: &'a ToolDispatchContext,
        work: &'a crate::WorkAuthorizationContext,
        site: &'static str,
    ) -> Self {
        Self {
            context,
            work,
            site,
            churn: context.owner_change_churn(),
        }
    }

    /// After an attempt that refused to rebuild ([`refuses_to_rebuild`]):
    /// whether to make it again. The attempt ran nothing, so it takes the
    /// wait after a stale read
    /// ([`crate::authorization::OwnerChangeChurn::wait_after_stale_read`]).
    /// While the work reports an owner change in progress (a held
    /// reservation of the native owner's writer included), wait for it
    /// within the bounds; with none at that read (the holder may have
    /// finished between the attempt and the read), pause one backoff step
    /// first. Again only after a settled wait, one of the step's free
    /// rebuilds; never after an unsettled one, past the deadline or once the
    /// free rebuilds are spent.
    async fn again(&mut self) -> bool {
        let context = self.context;
        let operation = context
            .prepared_authorization()
            .map(|prepared| &prepared.binding().facts().operation_id);
        self.churn
            .wait_after_stale_read(
                self.work.authorization().as_ref(),
                context.remaining_tool_time(),
                self.site,
                operation,
            )
            .await
            .settled()
    }
}

/// Dispatch a plan only through the exact root allocation and exact canonical
/// call identity used during resolution. Under governed work a comms or
/// memory plan without a target of its owner's kind is refused first (see
/// [`ToolDispatchContext::require_authorized_operation_target`]).
///
/// The prepared entry waits out an owner change in progress on the work's
/// own publication with its own [`crate::authorization::OwnerChangeChurn`];
/// each review attempt (one, or two when a superseded model review is
/// reviewed once more) has another, shared through the dispatch context by
/// its post-verdict check and every entry staging of the attempt (the root
/// staging, and the execution policy gate's staging after the gate's own
/// awaits). All are inside the plan's tool timeout: per dispatch at most 3
/// such values, 9 free rebuilds and 6 s of waits. Each asks the dispatch
/// context's work authorization, the run's own, which reports such a change
/// through every wrapper; an exact-control work reports none, so its refusal
/// takes the ordinary path. A tool body's own waits (a delegate operation's,
/// or a waiting governed step's: `run_if_current_within_after_owner_change`
/// and its siblings) take a waiting state of their own from the dispatch
/// context, each bounded by the call's remaining tool time.
///
/// An operation refusal met before the dispatcher is invoked is certified
/// [`crate::authorization::RefusalDispatch::NotDispatched`] at this entry's
/// one boundary (`FencedOutcome`). What the dispatcher returns is
/// forwarded unchanged: its tool code may have produced an effect before a
/// later governed step refused.
///
/// A context carrying an observation barrier
/// ([`ToolDispatchContext::with_observation_barrier`]) awaits it after the
/// root entry staging and before any dispatcher code runs, then checks the
/// decision again (see
/// [`crate::authorization::OperationObservationBarrier`]); its error refuses
/// the call with [`crate::ToolError::OperationObservationUnavailable`]. A
/// context without one is dispatched exactly as before.
///
/// A refused or unavailable result is reported, from that same value, to
/// the operation refusal observer of the host that composed the agent, when
/// the context carries one (the agent loop's and its external tool calls').
pub async fn dispatch_tool_execution_plan_fenced<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
    plan: &crate::ResolvedToolExecutionPlan,
) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
    let operation_id = crate::OperationId::new();
    let result = fenced_dispatch(dispatcher, call, context, plan, &operation_id)
        .await
        .unwrap_or_else(|refused| Err(refused.not_dispatched()));
    if let Err(error) = &result {
        context.observe_tool_refusal(
            call.id,
            context.work_authorization().map(|_| operation_id),
            error,
        );
    }
    result
}

/// How one fenced entry ended, before its certification boundary. `Ok`
/// carries the result of a dispatch whose dispatcher ran, the dispatcher's
/// own, forwarded unchanged. `Err` is a refusal or failure met before any
/// dispatcher code ran: the entry's one boundary certifies such an operation
/// refusal [`crate::authorization::RefusalDispatch::NotDispatched`].
type FencedOutcome = Result<
    Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError>,
    crate::error::ToolError,
>;

/// [`dispatch_tool_execution_plan_fenced`] before its certification
/// boundary.
async fn fenced_dispatch<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
    plan: &crate::ResolvedToolExecutionPlan,
    operation_id: &crate::OperationId,
) -> FencedOutcome {
    let deadline = DispatchDeadline::start(plan);
    validate_fenced_root_dispatch(dispatcher, call, context, plan)?;
    // Retain one binding with the actual resolved call. Its generation is
    // checked here after the dispatch queue, and again by adapters after any
    // further preparation waits, such as application consequence policy.
    let governed = if let Some(work) = context.work_authorization() {
        require_owner_target_for_targeted_sources(dispatcher.as_ref(), call, plan)?;
        let binding = crate::authorization::PreparedAuthorizationBinding::new(
            crate::authorization::OperationAuthorizationFacts {
                operation_id: operation_id.clone(),
                execution_scope: work.execution_scope().clone(),
                run_id: context.run_id().cloned(),
                context_revision: None,
                operation: crate::authorization::AuthorizationOperation::Tool(
                    crate::authorization::ToolAuthorizationFacts {
                        call_id: Arc::from(call.id),
                        name: call.name.into(),
                        arguments: Arc::from(call.args.to_owned()),
                        target: crate::authorization::ToolAuthorizationTarget::Dispatcher(
                            Arc::new(plan.clone()),
                        ),
                    },
                ),
            },
        );
        let prepared = prepare_current_after_owner_change(
            work,
            &binding,
            &mut crate::authorization::OwnerChangeChurn::reporting(
                context.call_wait_report(call.id),
            ),
            deadline,
        )
        .await?;
        Some((work, prepared))
    } else {
        None
    };
    let Some((work, mut prepared)) = governed else {
        // Ungoverned work retains no review (nothing can be superseded) and
        // no decision whose staging could refuse to rebuild.
        return dispatch_admitted_plan(dispatcher, call, context, plan)
            .await
            .settled();
    };
    // Review admission at the common prepared entry: the owner tier is
    // enforced here for every dispatcher, wrapper or not. An allow is
    // retained for the effect leaf; nothing is spent here.
    // A model review that a completed owner publication change superseded
    // before any dispatcher code ran (after its verdict, at its commit, or at
    // this dispatch's own root staging) is admitted again at most once, for
    // the current decision; `ReviewSupersession::readmit` alone decides.
    let mut supersession = crate::approval::review::ReviewSupersession::new();
    let mut material = None;
    loop {
        let (review_call, _) = prepared.tool_dispatch_parts().ok_or_else(|| {
            crate::ToolError::AuthorizationRefused {
                refusal: crate::authorization::OperationRefused::new(
                    crate::authorization::OperationRefusalKind::MalformedFacts,
                ),
            }
        })?;
        // Each review attempt has its own owner-change waiting state, shared
        // by its post-verdict check and every entry staging of the attempt.
        let staging_wait = EntryStagingWait::new(deadline, context.call_wait_report(call.id));
        let admitted = {
            let mut attempt_churn = staging_wait.churn.lock().await;
            crate::approval::review::admit_operation_review_attempt(
                context.operation_review(),
                dispatcher.review_entry_support(call.name),
                review_call,
                &prepared,
                work,
                material.take(),
                &mut attempt_churn,
            )
            .await
        };
        let superseded = match admitted {
            Ok(review_entry) => {
                let governed = context
                    .clone()
                    .with_prepared_authorization(prepared.clone())
                    .with_review_entry(review_entry)
                    .with_staging_wait(staging_wait);
                match dispatch_admitted_plan(dispatcher, call, &governed, plan).await {
                    Admitted::Dispatched(result) => return Ok(result),
                    Admitted::Refused(error) => return Err(error),
                    Admitted::Superseded(superseded) => superseded,
                }
            }
            Err(crate::approval::review::ReviewAdmissionRefusal::Settled(error)) => {
                return Err(error);
            }
            Err(crate::approval::review::ReviewAdmissionRefusal::Superseded(superseded)) => {
                superseded
            }
        };
        let readmission = supersession.readmit(
            context.operation_review(),
            *superseded,
            deadline.remaining(),
        )?;
        prepared = readmission.successor;
        material = Some(readmission.material);
    }
}

/// Fresh explicit attempt of one retained R3 candidate after an
/// authenticated human decision (ADR-001 human consent).
///
/// The approval owner resolves `approval_id` only for this context's own
/// admitted work association and run: a reference copied from another work
/// association or run, or a new call with equal-looking arguments, has no
/// consent. The RETAINED binding-owned call and plan then go through the same
/// fenced entry as any dispatch (root and execution-owner validation, entry
/// staging, the current authorization check), and the effect leaf spends the
/// decision once immediately before its body. A refusal anywhere settles this
/// attempt locally with zero entry; a failure before entry leaves the consent
/// spendable. The decision itself never executes anything.
///
/// As at [`dispatch_tool_execution_plan_fenced`], an operation refusal met
/// before the dispatcher is invoked is certified
/// [`crate::authorization::RefusalDispatch::NotDispatched`] at this entry's
/// one boundary, and the dispatcher's own result is forwarded unchanged.
///
/// When this process rebuilt the agent after the plan was resolved, the
/// retained call is resolved afresh against the current dispatcher (and
/// checked as every call is after its resolution), and the approval owner
/// rebinds the custody to that plan only as the same durable execution (call
/// identity, declaration digest, durable operation targets, tier); anything
/// else refuses with zero entry. The old plan is never
/// dispatched through another allocation. The execution mode is part of
/// that identity: a hybrid tool that selects another mode after the rebuild
/// is a mismatch. A resolution that is not ready yet (the tool not published
/// again, or its owner unavailable now) refuses here with zero entry and
/// leaves the consent spendable, as does an admission or a spend the
/// approval owner could not settle now (`owner_unavailable`, the consent
/// kept); a run's own attempt then counts as not made, and the run's next
/// boundary attempts it again. The admission settles the held decision
/// first (ADR-001 D9): one that is not current is decided afresh by its
/// owners, and the attempt goes on only with a current one, while a related
/// change or their refusal withdraws the consent and refuses the attempt
/// with zero entry.
pub async fn dispatch_retained_consent_attempt<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    context: &ToolDispatchContext,
    approval_id: &crate::ApprovalId,
) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
    let ended = attempt_retained_consent(dispatcher, context, approval_id).await;
    observe_retained_consent_refusal(context, &ended.result, ended.trace.call);
    ended.result
}

/// How one run's retained consent attempt ended.
pub(crate) enum RetainedConsentAttempt {
    /// It ran to its outcome: the dispatcher's own result, or a refusal with
    /// zero entry.
    Settled(Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError>),
    /// The session's rebuilt agent could not resolve the retained call yet,
    /// or the approval owner could not settle the attempt now and kept the
    /// consent (`owner_unavailable`: held, or the decision or the approver's
    /// eligibility not current and not settled for this attempt) while the
    /// attempt spent nothing: nothing entered, the consent stays spendable,
    /// and the attempt counts as not made.
    NotReady,
}

/// [`dispatch_retained_consent_attempt`] for a run's own attempt, which
/// tells a resolution or a spend that is not ready yet apart from a refusal.
/// An `owner_unavailable` answer counts as not made only when the attempt's
/// own review entry spent nothing (or it was refused at its admission,
/// before it had one): an entered effect's own result is its outcome,
/// whatever its kind.
pub(crate) async fn dispatch_retained_consent_in_run<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    context: &ToolDispatchContext,
    approval_id: &crate::ApprovalId,
) -> RetainedConsentAttempt {
    let ended = attempt_retained_consent(dispatcher, context, approval_id).await;
    let owner_unavailable = matches!(
        ended.result,
        Err(crate::error::ToolError::ReviewUnavailable {
            kind: crate::approval::review::ReviewUnavailableKind::OwnerUnavailable,
        })
    );
    if ended.result.is_err()
        && (ended.trace.not_ready || (owner_unavailable && ended.spent_nothing))
    {
        return RetainedConsentAttempt::NotReady;
    }
    observe_retained_consent_refusal(context, &ended.result, ended.trace.call);
    RetainedConsentAttempt::Settled(ended.result)
}

/// What one retained attempt learned on its way to its outcome.
#[derive(Default)]
struct RetainedAttemptTrace {
    /// The retained call's id and operation id, once the candidate resolved:
    /// the call the attempt's refusal is reported for.
    call: Option<(String, crate::OperationId)>,
    /// The rebuilt agent could not resolve the retained call yet.
    not_ready: bool,
    /// The review entry the attempt was admitted with, once it had one.
    entry: Option<crate::approval::review::ReviewEntry>,
}

/// One retained attempt through its certification boundary.
struct RetainedAttemptEnd {
    result: Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError>,
    trace: RetainedAttemptTrace,
    /// No spend of the attempt's own review entry committed (or it never had
    /// one), read once its dispatch ended.
    spent_nothing: bool,
}

/// One retained attempt through its certification boundary, with what it
/// learned on its way.
async fn attempt_retained_consent<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    context: &ToolDispatchContext,
    approval_id: &crate::ApprovalId,
) -> RetainedAttemptEnd {
    let mut trace = RetainedAttemptTrace::default();
    let result = retained_consent_dispatch(dispatcher, context, approval_id, &mut trace)
        .await
        .unwrap_or_else(|refused| Err(refused.not_dispatched()));
    // The entry is read, not kept, once its dispatch ended.
    let spent_nothing = trace
        .entry
        .take()
        .as_ref()
        .is_none_or(crate::approval::review::ReviewEntry::spent_nothing);
    RetainedAttemptEnd {
        result,
        trace,
        spent_nothing,
    }
}

/// Report a retained attempt's refusal to the host's refusal observer.
fn observe_retained_consent_refusal(
    context: &ToolDispatchContext,
    result: &Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError>,
    retained_call: Option<(String, crate::OperationId)>,
) {
    if let (Err(error), Some((tool_use_id, operation_id))) = (result, retained_call) {
        context.observe_tool_refusal(&tool_use_id, Some(operation_id), error);
    }
}

/// [`dispatch_retained_consent_attempt`] before its certification boundary
/// ([`FencedOutcome`]), leaving what it learned in `trace`: once the retained
/// candidate resolves (even when its admission then refuses), its call id
/// and operation id, for the attempt's refusal report; whether it refuses
/// because the rebuilt agent could not resolve the retained call yet; and
/// the review entry it is admitted with.
async fn retained_consent_dispatch<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    context: &ToolDispatchContext,
    approval_id: &crate::ApprovalId,
    trace: &mut RetainedAttemptTrace,
) -> FencedOutcome {
    let malformed = || crate::ToolError::AuthorizationRefused {
        refusal: crate::authorization::OperationRefused::new(
            crate::authorization::OperationRefusalKind::MalformedFacts,
        ),
    };
    let Some(work) = context.work_authorization() else {
        return Err(malformed());
    };
    let (mut review_entry, mut retained) =
        admit_retained_consent(context, work, approval_id, trace)?;
    // This process rebuilt the session's agent after the retained plan was
    // resolved: its root allocation is gone, and only a fresh resolution the
    // approval owner accepts as the same durable execution goes on.
    let (call, plan) = retained.tool_dispatch_parts().ok_or_else(malformed)?;
    if matches!(
        plan.validate_root_dispatch(dispatcher, call),
        Err(crate::ToolExecutionResolutionError::RootDispatcherChanged { .. })
    ) {
        match rebind_retained_consent(dispatcher, context, work, approval_id, &retained) {
            RetainedRebind::Rebound => {
                (review_entry, retained) =
                    admit_retained_consent(context, work, approval_id, trace)?;
            }
            RetainedRebind::NotReady(error) => {
                trace.not_ready = true;
                return Err(error);
            }
            RetainedRebind::Refused(error) => return Err(error),
        }
    }
    trace.entry = Some(review_entry.clone());
    let (call, plan) = retained.tool_dispatch_parts().ok_or_else(malformed)?;
    if dispatcher.review_entry_support(call.name)
        != crate::approval::review::ReviewEntrySupport::ConsumesAtEntry
    {
        return Err(crate::ToolError::ReviewUnavailable {
            kind: crate::approval::review::ReviewUnavailableKind::UnsupportedEntry,
        });
    }
    validate_fenced_root_dispatch(dispatcher, call, context, plan)?;
    // The retained plan keeps the original call's resolved deadline: the
    // attempt runs, and its stagings wait, under the same effective timeout
    // that call had.
    let governed = context
        .clone()
        .with_prepared_authorization(retained.clone())
        .with_review_entry(review_entry)
        .with_staging_wait(EntryStagingWait::new(
            DispatchDeadline::start(plan),
            context.call_wait_report(call.id),
        ));
    // A consent entry is never superseded (its check point only settles), so
    // nothing here is reviewed again.
    let Some(timeout) = plan.effective_timeout() else {
        return dispatch_admitted_plan(dispatcher, call, &governed, plan)
            .await
            .settled();
    };
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    match tokio::time::timeout(
        timeout,
        dispatch_admitted_plan(dispatcher, call, &governed, plan),
    )
    .await
    {
        Ok(admitted) => admitted.settled(),
        // The timeout may have ended a running dispatcher: no claim.
        Err(_) => Ok(Err(crate::error::ToolError::timeout(call.name, timeout_ms))),
    }
}

/// Admit a fresh attempt of the retained candidate `approval_id` for
/// `context`. Once the candidate first resolves (even when its admission
/// then refuses: its held decision could not be settled), its call id and
/// operation id are left in `trace`, for the attempt's refusal report.
fn admit_retained_consent(
    context: &ToolDispatchContext,
    work: &crate::WorkAuthorizationContext,
    approval_id: &crate::ApprovalId,
    trace: &mut RetainedAttemptTrace,
) -> Result<
    (
        crate::approval::review::ReviewEntry,
        crate::authorization::PreparedOperationCheck,
    ),
    crate::error::ToolError,
> {
    let retained_call = |retained: &crate::authorization::PreparedOperationCheck| {
        retained.tool_dispatch_parts().map(|(call, _)| {
            (
                call.id.to_owned(),
                retained.binding().facts().operation_id.clone(),
            )
        })
    };
    let admitted = crate::approval::review::admit_consent_attempt_reporting(
        context.operation_review(),
        approval_id,
        work,
        context.run_id(),
    );
    if trace.call.is_none() {
        trace.call = match &admitted {
            Ok((_, retained)) => retained_call(retained),
            Err(refused) => refused.retained.as_ref().and_then(retained_call),
        };
    }
    admitted.map_err(|refused| refused.error)
}

/// What re-resolving a retained consent attempt for a rebuilt agent did.
enum RetainedRebind {
    /// The approval owner rebound the custody to the fresh plan.
    Rebound,
    /// Not ready now: nothing changed, and the consent stays spendable.
    NotReady(crate::error::ToolError),
    /// Refused with zero entry.
    Refused(crate::error::ToolError),
}

/// Resolve `retained`'s call afresh against `dispatcher`, with the retained
/// plan's own resolution context, and have the approval owner rebind the
/// custody to that plan, only as the same durable execution. The fresh plan
/// passes the root seam every call passes after its resolution
/// ([`AgentToolDispatcher::validate_resolved_execution_plan`]: the caller's
/// deadline chain, the tool published and callable, its mode within the live
/// catalog contract). A tool the rebuilt agent has not published yet, or one
/// unavailable now, is not ready, as is an approval owner that is held now
/// or a fresh decision its owners cannot make now (unavailable, or an owner
/// change in progress); any other resolution or validation failure refuses.
fn rebind_retained_consent<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    context: &ToolDispatchContext,
    work: &crate::WorkAuthorizationContext,
    approval_id: &crate::ApprovalId,
    retained: &crate::authorization::PreparedOperationCheck,
) -> RetainedRebind {
    let owner_changed = |name: &str| {
        RetainedRebind::Refused(crate::error::ToolError::unavailable(
            name,
            crate::ToolUnavailableReason::ExecutionOwnerChanged,
        ))
    };
    let Some((call, plan)) = retained.tool_dispatch_parts() else {
        return RetainedRebind::Refused(crate::ToolError::AuthorizationRefused {
            refusal: crate::authorization::OperationRefused::new(
                crate::authorization::OperationRefusalKind::MalformedFacts,
            ),
        });
    };
    let (Some(resolution_context), Some(review)) =
        (plan.resolution_context(), context.operation_review())
    else {
        return owner_changed(call.name);
    };
    let fresh =
        match resolve_tool_execution_plan_fenced(dispatcher, call, context, resolution_context)
            .and_then(|fresh| {
                dispatcher
                    .validate_resolved_execution_plan(call, resolution_context, &fresh)
                    .map(|()| fresh)
            }) {
            Ok(fresh) => fresh,
            Err(
                error @ (crate::ToolExecutionResolutionError::NotFound { .. }
                | crate::ToolExecutionResolutionError::Unavailable { .. }),
            ) => return RetainedRebind::NotReady(error.into()),
            Err(error) => return RetainedRebind::Refused(error.into()),
        };
    match review.rebind_consent_to_reresolved_plan(
        approval_id,
        work,
        context.run_id(),
        retained,
        fresh,
    ) {
        crate::approval::consent::ConsentRebind::Rebound => RetainedRebind::Rebound,
        crate::approval::consent::ConsentRebind::NotReady => {
            RetainedRebind::NotReady(crate::error::ToolError::ReviewUnavailable {
                kind: crate::approval::review::ReviewUnavailableKind::OwnerUnavailable,
            })
        }
        crate::approval::consent::ConsentRebind::Refused(refusal) => {
            RetainedRebind::Refused(refusal.into_tool_error())
        }
    }
}

/// How the common tail of one admitted fenced dispatch ended.
enum Admitted {
    /// Its dispatcher ran: the dispatcher's own result, settled.
    Dispatched(Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError>),
    /// Refused or failed before any dispatcher code ran.
    Refused(crate::error::ToolError),
    /// Its root staging found the retained model review superseded by a
    /// completed owner publication change, before any dispatcher code ran.
    Superseded(Box<crate::approval::review::SupersededReview>),
}

impl Admitted {
    /// The outcome where no supersession decision follows: a superseded
    /// review settles with its own settlement, before any dispatcher code
    /// ran.
    fn settled(self) -> FencedOutcome {
        match self {
            Self::Dispatched(result) => Ok(result),
            Self::Refused(error) => Err(error),
            Self::Superseded(superseded) => Err(superseded.into_settled()),
        }
    }
}

/// The common tail of a fenced dispatch whose review admission is settled:
/// entry staging and the check point, the body, the outcome observation, and
/// the dispatch-lifetime review guard. A refusal before the dispatcher runs
/// ends it as [`Admitted::Refused`], and a retained model review superseded
/// at the root staging as [`Admitted::Superseded`].
async fn dispatch_admitted_plan<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
    plan: &crate::ResolvedToolExecutionPlan,
) -> Admitted {
    // The guard lives exactly as long as this dispatch. Dropping it here
    // (cancellation, enclosing tool timeout) abandons a retained review and
    // closes every clone a worker or transport may still hold.
    let review_guard = context
        .review_entry
        .clone()
        .map(crate::approval::review::ReviewDispatchGuard::new);
    let outcome = async {
        // Review admission can await an operator. Fresh app custody and its
        // private invocation/scope must still hold immediately before entry,
        // independently of whether a particular leaf performs another check.
        if let Some(control) = context.tool_application_control() {
            control
                .revalidate_async()
                .await
                .map_err(crate::ToolError::from)?;
        }
        // Forward the binding's own immutable payload and plan all the way to the
        // body. Adapters can then validate exact custody by pointer identity after
        // a wait, without rehashing arguments or scanning policy dependencies.
        let (call, plan) = if let Some(prepared) = context.prepared_authorization() {
            prepared.tool_dispatch_parts().ok_or_else(|| {
                crate::ToolError::AuthorizationRefused {
                    refusal: crate::authorization::OperationRefused::new(
                        crate::authorization::OperationRefusalKind::MalformedFacts,
                    ),
                }
            })?
        } else {
            (call, plan)
        };
        let entry_context = match context.stage_root_tool_entry(call, plan).await {
            Ok(entry_context) => entry_context,
            Err(crate::approval::review::ReviewAdmissionRefusal::Settled(error)) => {
                return Err(error);
            }
            Err(crate::approval::review::ReviewAdmissionRefusal::Superseded(superseded)) => {
                return Ok(Admitted::Superseded(superseded));
            }
        };
        // Only a context carrying an observation barrier waits here, so its
        // owner can make the staged decision and entry durable before any
        // dispatcher code runs. Every other context goes straight on.
        let entry_context = match context.observation_barrier.as_ref() {
            None => entry_context,
            Some(barrier) => {
                match await_observation_barrier(barrier, context, entry_context, call, plan).await {
                    Ok(entry_context) => entry_context,
                    Err(crate::approval::review::ReviewAdmissionRefusal::Settled(error)) => {
                        return Err(error);
                    }
                    Err(crate::approval::review::ReviewAdmissionRefusal::Superseded(
                        superseded,
                    )) => return Ok(Admitted::Superseded(superseded)),
                }
            }
        };
        let context = entry_context.as_ref().unwrap_or(context);
        let result = match plan.kind() {
            crate::ResolvedExecutionKind::Streaming(policy) => {
                let absolute_timeout = plan
                    .deadlines()
                    .effective_timeout()
                    .unwrap_or_else(|| policy.absolute_timeout());
                crate::streaming_tool::supervise_streaming_tool(
                    call.name,
                    policy.inactivity_timeout(),
                    absolute_timeout,
                    |streaming| {
                        let streaming_context = context.clone().with_streaming(streaming);
                        async move {
                            dispatcher
                                .dispatch_resolved_with_context(call, &streaming_context, plan)
                                .await
                        }
                    },
                )
                .await
            }
            crate::ResolvedExecutionKind::Fast | crate::ResolvedExecutionKind::Detached(_) => {
                dispatcher
                    .dispatch_resolved_with_context(call, context, plan)
                    .await
            }
        };
        Ok(Admitted::Dispatched(context.observe_tool_outcome(
            dispatcher.live_bridge_effect_kind(call.name),
            result,
        )))
    }
    .await;
    // After the dispatcher returned (or the root staging found the retained
    // review superseded): a retained allow the leaf never spent is released
    // unspent, every clone is closed, and the deferred `Used` projection is
    // delivered only now.
    if let Some(guard) = review_guard {
        guard.complete();
    }
    outcome.unwrap_or_else(Admitted::Refused)
}

/// Rounds of a fenced entry's observation barrier before the entry is
/// refused: each decision prepared again during a wait goes round once more.
const OBSERVATION_BARRIER_ROUNDS: usize = 3;

/// The observation barrier of one fenced entry, after its root staging and
/// before any dispatcher code runs (see
/// [`crate::authorization::OperationObservationBarrier`]).
///
/// After each wait the entering decision is checked again, as the root
/// staging checks it (app custody, then the decision's currentness), with
/// nothing held across the wait. A decision that is still current is the one
/// the barrier just made durable, and the call enters with it. A decision
/// prepared again meanwhile (an owner change during the wait) is staged with
/// a new root entry and goes round once more, at most
/// [`OBSERVATION_BARRIER_ROUNDS`] times, so what the barrier made durable
/// always carries the decision the call enters with. A failed barrier, a
/// refusal or too many changes settle the entry; nothing is sent.
async fn await_observation_barrier(
    barrier: &Arc<dyn crate::authorization::OperationObservationBarrier>,
    root: &ToolDispatchContext,
    mut entering: Option<ToolDispatchContext>,
    call: ToolCallView<'_>,
    plan: &crate::ResolvedToolExecutionPlan,
) -> Result<Option<ToolDispatchContext>, crate::approval::review::ReviewAdmissionRefusal> {
    for _ in 0..OBSERVATION_BARRIER_ROUNDS {
        barrier
            .before_entry()
            .await
            .map_err(|_| crate::ToolError::OperationObservationUnavailable)?;
        let current = entering.as_ref().unwrap_or(root);
        let Some(prepared_again) = current.check_tool_authorization(call, Some(plan))? else {
            return Ok(entering);
        };
        let staged = prepared_again.stage_root_tool_entry(call, plan).await?;
        entering = Some(staged.unwrap_or(prepared_again));
    }
    // The owner kept changing: the entering decision is refused, and the
    // refusal is staged like every other refusal of the decision.
    let refusal = crate::authorization::OperationRefused::new(
        crate::authorization::OperationRefusalKind::ReprepareRequired,
    );
    if let Some(prepared) = entering.as_ref().unwrap_or(root).prepared_authorization() {
        prepared
            .observe_refusal(refusal)
            .map_err(|_| crate::ToolError::OperationObservationUnavailable)?;
    }
    Err(crate::ToolError::AuthorizationRefused { refusal }.into())
}

/// Compute whether the current exact catalog should stay inline or switch to deferred mode.
pub fn select_tool_catalog_mode<T>(dispatcher: &T) -> ToolCatalogMode
where
    T: AgentToolDispatcher + ?Sized,
{
    let capabilities = dispatcher.tool_catalog_capabilities();
    if !capabilities.exact_catalog {
        return ToolCatalogMode::Inline;
    }
    let pending_sources = dispatcher.pending_catalog_sources();
    let catalog = dispatcher.tool_catalog();
    select_catalog_mode_from_snapshot(
        capabilities.exact_catalog,
        catalog.as_ref(),
        pending_sources.as_ref(),
    )
}

/// Compute whether the catalog control plane should be composed for this
/// dispatcher, even if the current adaptive snapshot remains inline.
pub fn should_compose_tool_catalog_control_plane<T>(dispatcher: &T) -> bool
where
    T: AgentToolDispatcher + ?Sized,
{
    let capabilities = dispatcher.tool_catalog_capabilities();
    if !capabilities.exact_catalog {
        return false;
    }
    if capabilities.may_require_catalog_control_plane {
        return true;
    }

    let pending_sources = dispatcher.pending_catalog_sources();
    if !pending_sources.is_empty() {
        return true;
    }

    let catalog = dispatcher.tool_catalog();
    deferred_session_entry_count(catalog.as_ref()) > 0
}

/// Error from [`AgentToolDispatcher::bind_ops_lifecycle`].
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum OpsLifecycleBindError {
    #[error("ops lifecycle binding is unsupported")]
    Unsupported,
    #[error("dispatcher has shared ownership and cannot be rebound")]
    SharedOwnership,
}

/// A tool dispatcher that filters tools based on a policy
///
/// Legacy tool lists are filtered once at construction time based on the
/// allowed_tools list. Exact-catalog dispatchers keep catalog callability live.
/// The inner dispatcher is used for actual dispatch, but only allowed tools are
/// exposed via tools() and dispatch() returns AccessDenied for filtered tools.
pub struct FilteredToolDispatcher<T: AgentToolDispatcher + ?Sized> {
    inner: Arc<T>,
    allowed_tools: ToolNameSet,
    /// Pre-computed filtered tool list for non-exact dispatchers.
    filtered_tools: Arc<[Arc<ToolDef>]>,
}

impl<T: AgentToolDispatcher + ?Sized> FilteredToolDispatcher<T> {
    pub fn new<I, N>(inner: Arc<T>, allowed_tools: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<ToolName>,
    {
        let allowed_set: ToolNameSet = allowed_tools
            .into_iter()
            .map(Into::into)
            .collect::<ToolNameSet>();

        let filtered: Vec<Arc<ToolDef>> = if inner.tool_catalog_capabilities().exact_catalog {
            inner
                .tool_catalog()
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .filter(|t| allowed_set.contains(t.name.as_str()))
                .collect()
        } else {
            inner
                .tools()
                .iter()
                .filter(|t| allowed_set.contains(t.name.as_str()))
                .map(Arc::clone)
                .collect()
        };

        Self {
            inner,
            allowed_tools: allowed_set,
            filtered_tools: filtered.into(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T: AgentToolDispatcher + ?Sized + 'static> AgentToolDispatcher for FilteredToolDispatcher<T> {
    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &crate::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &ToolDispatchContext,
    ) -> Result<crate::tool_application::ToolApplicationResolution, crate::ToolError> {
        if !self.allowed_tools.contains(source_tool) {
            return Err(crate::ToolError::access_denied(source_tool));
        }
        let resolution = self
            .inner
            .resolve_tool_application(source_tool, request, invocation, context)
            .await?;
        if let crate::tool_application::ToolApplicationResolution::Call { name, .. } = &resolution
            && !self.allowed_tools.contains(name.as_str())
        {
            return Err(crate::ToolError::access_denied(name));
        }
        Ok(resolution)
    }

    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        if self.inner.tool_catalog_capabilities().exact_catalog {
            return self
                .inner
                .tool_catalog()
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .filter(|tool| self.allowed_tools.contains(tool.name.as_str()))
                .collect::<Vec<_>>()
                .into();
        }
        Arc::clone(&self.filtered_tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        self.dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        if !self.allowed_tools.contains(call.name) {
            let inner_knows_tool = if self.inner.tool_catalog_capabilities().exact_catalog {
                self.inner
                    .tool_catalog()
                    .iter()
                    .any(|entry| entry.tool.name == call.name)
            } else {
                self.inner.tools().iter().any(|tool| tool.name == call.name)
            };
            if !inner_knows_tool {
                return Err(crate::error::ToolError::not_found(call.name));
            }
            return Err(crate::error::ToolError::access_denied(call.name));
        }
        self.inner.dispatch_with_context(call, context).await
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        if !self.allowed_tools.contains(call.name) {
            let inner_knows_tool = if self.inner.tool_catalog_capabilities().exact_catalog {
                self.inner
                    .tool_catalog()
                    .iter()
                    .any(|entry| entry.tool.name == call.name)
            } else {
                self.inner.tools().iter().any(|tool| tool.name == call.name)
            };
            if !inner_knows_tool {
                return Err(crate::error::ToolError::not_found(call.name));
            }
            return Err(crate::error::ToolError::access_denied(call.name));
        }
        self.inner
            .dispatch_resolved_with_context(call, context, plan)
            .await
    }

    fn tool_mutation_class(&self, tool_name: &str) -> crate::ToolMutationClass {
        // Visibility filtering never changes what a tool does: the owning
        // dispatcher's declaration is forwarded unchanged, and a name this
        // filter hides is denied by the allow-set check before dispatch.
        self.inner.tool_mutation_class(tool_name)
    }

    fn live_bridge_effect_kind(&self, tool_name: &str) -> crate::LiveBridgeEffectKind {
        self.inner.live_bridge_effect_kind(tool_name)
    }

    fn review_entry_support(&self, tool_name: &str) -> crate::approval::review::ReviewEntrySupport {
        self.inner.review_entry_support(tool_name)
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        if !self.inner.tool_catalog_capabilities().exact_catalog {
            return self
                .tools()
                .iter()
                .map(|tool| ToolCatalogEntry::session_inline(Arc::clone(tool), true))
                .collect::<Vec<_>>()
                .into();
        }
        self.inner
            .tool_catalog()
            .iter()
            .filter(|entry| self.allowed_tools.contains(entry.tool.name.as_str()))
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<crate::EphemeralToolBindingFingerprint, crate::ToolExecutionResolutionError> {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        let child = self.inner.execution_binding_fingerprint(tool_name)?;
        Ok(crate::ephemeral_tool_catalog_binding_fingerprint(entry)
            .with_live_authority(0, 0)
            .with_dependency(&child))
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &crate::ToolExecutionResolutionContext,
    ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
        if !self.allowed_tools.contains(call.name) {
            let inner_knows_tool = if self.inner.tool_catalog_capabilities().exact_catalog {
                self.inner
                    .tool_catalog()
                    .iter()
                    .any(|entry| entry.tool.name == call.name)
            } else {
                self.inner.tools().iter().any(|tool| tool.name == call.name)
            };
            return Err(if inner_knows_tool {
                crate::ToolExecutionResolutionError::AccessDenied {
                    tool_name: call.name.to_string(),
                }
            } else {
                crate::ToolExecutionResolutionError::NotFound {
                    tool_name: call.name.to_string(),
                }
            });
        }

        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?;
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(crate::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason,
            });
        }

        self.inner
            .resolve_execution_plan(call, dispatch_context, resolution_context)
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }

    async fn poll_external_updates(&self) -> ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn external_tool_surface_snapshot(&self) -> Option<crate::ExternalToolSurfaceSnapshot> {
        self.inner.external_tool_surface_snapshot()
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        self.inner.capabilities()
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: crate::types::SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        // An inner dispatcher that needs the binding but is still shared
        // cannot be rebound. Leaving it unbound would silently drop its owner
        // session and registry, so the bind is rejected.
        if owned.inner.capabilities().ops_lifecycle && Arc::strong_count(&owned.inner) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        if Arc::strong_count(&owned.inner) == 1 {
            let outcome = owned
                .inner
                .bind_ops_lifecycle(registry, owner_bridge_session_id)?;
            let bound = outcome.was_bound();
            let d = outcome.into_dispatcher();
            let allowed_tools = owned.allowed_tools.into_iter().collect::<Vec<_>>();
            Ok(if bound {
                BindOutcome::Bound(Arc::new(FilteredToolDispatcher::new(d, allowed_tools)))
            } else {
                BindOutcome::Skipped(Arc::new(FilteredToolDispatcher::new(d, allowed_tools)))
            })
        } else {
            Ok(BindOutcome::Skipped(Arc::new(FilteredToolDispatcher {
                inner: owned.inner,
                allowed_tools: owned.allowed_tools,
                filtered_tools: owned.filtered_tools,
            })))
        }
    }

    fn completion_enrichment(
        &self,
    ) -> Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>> {
        self.inner.completion_enrichment()
    }

    fn bind_mcp_server_lifecycle_handle(
        &self,
        handle: Arc<dyn crate::handles::McpServerLifecycleHandle>,
    ) {
        self.inner.bind_mcp_server_lifecycle_handle(handle);
    }

    fn bind_external_tool_surface_handle(
        &self,
        handle: Arc<dyn crate::handles::ExternalToolSurfaceHandle>,
    ) {
        self.inner.bind_external_tool_surface_handle(handle);
    }
}

/// Trait for session stores
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AgentSessionStore: Send + Sync {
    async fn save(&self, session: &Session) -> Result<(), AgentError>;
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;
}

/// Runtime policy for inlining peer lifecycle updates into session context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlinePeerNotificationPolicy {
    /// Always inline batched peer lifecycle updates.
    Always,
    /// Never inline batched peer lifecycle updates.
    Never,
    /// Inline only when post-drain peer count is at or below this threshold.
    AtMost(usize),
}

/// Default inline threshold when no explicit value is configured.
pub const DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS: usize = 50;

impl InlinePeerNotificationPolicy {
    /// Resolve policy from transport/build-layer config representation.
    pub fn try_from_raw(raw: Option<i32>) -> Result<Self, i32> {
        match raw {
            None => Ok(Self::AtMost(DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS)),
            Some(-1) => Ok(Self::Always),
            Some(0) => Ok(Self::Never),
            Some(v) if v > 0 => Ok(Self::AtMost(v as usize)),
            Some(v) => Err(v),
        }
    }
}

/// Error returned when a comms runtime capability is not available.
#[derive(Debug, thiserror::Error)]
pub enum CommsCapabilityError {
    /// The runtime does not support this capability.
    #[error("comms capability not supported: {0}")]
    Unsupported(String),
    /// The runtime supports the capability but could not complete its exact
    /// queue handoff contract.
    #[error("comms capability failed: {0}")]
    HandoffFailed(String),
}

/// Trait for comms runtime that can be used with the agent
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait CommsRuntime: Send + Sync {
    /// Canonical runtime routing identity for this peer, if available.
    ///
    /// `PeerId` is the UUID-shaped routing key used by peer directories and
    /// trust stores. Implementations that only have the legacy string carrier
    /// may return a parsed UUID-shaped `public_key()` value; implementations
    /// with Ed25519 public keys should override this and return the pubkey-
    /// derived canonical [`PeerId`].
    fn peer_id(&self) -> Option<PeerId> {
        self.public_key()
            .as_deref()
            .and_then(|public_key| PeerId::parse(public_key).ok())
    }

    /// Runtime-local transport/auth public key, if available.
    ///
    /// Returns an Ed25519 public key string in `ed25519:<base64>` format.
    /// This is not the canonical routing [`PeerId`]; use [`Self::peer_id`]
    /// for roster/projection identity and peer-directory lookups.
    fn public_key(&self) -> Option<String> {
        None
    }

    /// Runtime-local Ed25519 public key bytes, if available.
    ///
    /// This is the typed form of [`Self::public_key`]. Trust installation
    /// paths that need to verify `PeerId`/pubkey consistency should prefer
    /// this method over reparsing the string carrier.
    fn public_key_bytes(&self) -> Option<[u8; 32]> {
        None
    }

    /// Runtime-local canonical comms routing name, if available.
    ///
    /// This is the peer name used in trusted-peer descriptors and peer
    /// directories. It is separate from the advertised transport address so
    /// callers do not recover identity by parsing transport strings.
    fn comms_name(&self) -> Option<String> {
        None
    }

    /// Release this runtime's published in-process participant route now,
    /// instead of when the last owner drops the runtime.
    ///
    /// A live participant name is never displaced, so an owner that retires a
    /// generation and then builds a successor under the same name in this
    /// process must release it first: the runtime can outlive retirement in
    /// any task still holding it. Release is generation-exact (it can only
    /// remove the route this runtime published, never a successor's) and
    /// idempotent with `Drop`. Returns whether this call removed a route. The
    /// default is for runtimes that publish no in-process route.
    fn retire_inproc_route(&self) -> bool {
        false
    }

    /// Close this runtime's in-process route for peer ingress while the
    /// runtime lives on (its owner stopped admitting peer work): peers then
    /// meet the route's wake owner, which answers typed, instead of an inbox
    /// no drain claims. Generation-exact; the closed route keeps its name
    /// free and its metadata for [`Self::reopen_inproc_route`]. Returns
    /// whether this call closed a route. The default is for runtimes that
    /// publish no in-process route.
    fn close_inproc_route(&self) -> bool {
        false
    }

    /// Reopen the route [`Self::close_inproc_route`] closed, if its name is
    /// still unbound. Returns whether this call reopened it.
    fn reopen_inproc_route(&self) -> bool {
        false
    }

    /// Runtime-local advertised comms address, if available.
    ///
    /// This is the canonical address the runtime expects peers to use when
    /// constructing a [`TrustedPeerDescriptor`]. Implementations that do not
    /// expose a stable advertised address can return `None`.
    fn advertised_address(&self) -> Option<String> {
        None
    }

    /// Runtime-local bootstrap proof for the initial supervisor bind, if
    /// available.
    fn bridge_bootstrap_token(&self) -> Option<String> {
        None
    }

    /// Apply a comms trust projection mutation authorized by generated
    /// machine/composition authority.
    ///
    /// This is the only mutable trust-store seam.
    async fn apply_trust_mutation(
        &self,
        _mutation: CommsTrustMutation,
    ) -> Result<CommsTrustMutationResult, SendError> {
        Err(SendError::Unsupported(
            "apply_trust_mutation not supported for this CommsRuntime".to_string(),
        ))
    }

    /// Bind this target runtime to the generated MobMachine owner token whose
    /// trust handoffs may mutate mob-owned trust rows.
    ///
    /// Mob runtimes call this before submitting a generated mob trust mutation.
    /// Implementations must fail closed when they cannot remember and compare
    /// the owner token during [`Self::apply_trust_mutation`].
    async fn install_generated_mob_trust_owner(
        &self,
        _owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "generated mob trust owner binding not supported for this CommsRuntime".to_string(),
        ))
    }

    /// Read-only preflight for binding this target runtime to a recovered
    /// MobMachine owner token.
    ///
    /// Resume uses this to validate every generated trust repair target before
    /// mutating any trust projection row. Implementations must not change the
    /// stored owner token here; [`Self::install_recovered_generated_mob_trust_owner`]
    /// performs the actual binding after the full batch has passed preflight.
    async fn validate_recovered_generated_mob_trust_owner(
        &self,
        _owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "recovered generated mob trust owner validation not supported for this CommsRuntime"
                .to_string(),
        ))
    }

    /// Rebind this target runtime to the owner token of a recovered
    /// MobMachine authority.
    ///
    /// Recovery reconstructs generated authority from persisted machine state,
    /// which gives it a fresh process-local owner token. Implementations may
    /// bind this owner only when no generated MobMachine owner is already
    /// installed, or when it is the same owner token. They must fail closed
    /// rather than replacing a different live owner through recovery plumbing.
    async fn install_recovered_generated_mob_trust_owner(
        &self,
        _owner: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "recovered generated mob trust owner binding not supported for this CommsRuntime"
                .to_string(),
        ))
    }

    /// Opaque host-acceptor registration material for reverse-lane demux
    /// composition (the runtime's identity pubkey, its ack-signing keypair,
    /// and its inbox sender), encoded by the concrete comms crate.
    ///
    /// A host that composes an acceptor demux in front of this runtime (so
    /// remote peers can dial one shared listener and be routed to this
    /// identity's inbox) decodes the payload where it holds the concrete
    /// comms dependency (`meerkat_comms::HostAcceptorRegistrationMaterial`).
    /// `None` means this runtime exposes no registration material and the
    /// composer must fail closed (no acceptor registration). The default is
    /// `None`; only the concrete comms runtime overrides it — the typed
    /// trait surface itself continues to expose no signing material.
    fn host_acceptor_registration_payload(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        None
    }

    /// Register a peer for admission-only trust without listing it in the
    /// directory.
    ///
    /// Used for control-plane edges — the canonical case is the supervisor
    /// bridge for session-backed mob members: lifecycle notifications
    /// (`mob.peer_added`, `mob.peer_retired`, …) must land at the member's
    /// inbox, but the supervisor must not appear as an ordinary sendable
    /// peer in `comms.peers` / REST / RPC / MCP. The admission gate consults
    /// both the public and private trust sets; `resolve_peer_directory()`
    /// consults only the public set.
    async fn add_private_trusted_peer(
        &self,
        _peer: TrustedPeerDescriptor,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "generated comms private trust mutation authority required".to_string(),
        ))
    }

    /// Remove a previously registered private-trust edge by peer ID.
    ///
    /// Returns `true` if the edge was present and removed, `false` if it
    /// was not.
    async fn remove_private_trusted_peer(&self, _peer_id: &str) -> Result<bool, SendError> {
        Err(SendError::Unsupported(
            "generated comms private trust mutation authority required".to_string(),
        ))
    }

    /// Install the host-owned outbound content-taint declaration.
    ///
    /// The declaration is host-set carrier config, not machine state: the
    /// host owns the "this session's content is tainted" fact and this
    /// runtime stamps it (inside the signed envelope region) on every
    /// outbound content-bearing send until changed. `None` clears the
    /// declaration (subsequent envelopes carry no claim — which receivers
    /// must never coalesce into `Clean`).
    ///
    /// The declaration is in-memory runtime state: a rebuilt runtime (e.g.
    /// a respawned mob member) starts with no declaration, which aligns
    /// with fresh-context taint semantics — hosts re-declare when their
    /// tracker re-marks the new context.
    ///
    /// Fails typed (never a silent no-op — silently dropping a security
    /// declaration would let tainted content ship with a clean-looking
    /// envelope) for runtimes that do not carry outbound comms.
    fn set_outbound_content_taint(
        &self,
        _taint: Option<crate::comms::SenderContentTaint>,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "outbound content-taint declaration not supported by this CommsRuntime".to_string(),
        ))
    }

    /// Dispatch a canonical comms command.
    async fn send(&self, _cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        Err(SendError::Unsupported(
            "send not implemented for this CommsRuntime".to_string(),
        ))
    }

    #[doc(hidden)]
    fn stream(&self, scope: StreamScope) -> Result<EventStream, StreamError> {
        let scope_desc = match scope {
            StreamScope::Session(session_id) => format!("session {session_id}"),
            StreamScope::Interaction(interaction_id) => format!("interaction {}", interaction_id.0),
        };
        Err(StreamError::NotFound(scope_desc))
    }

    /// List peers visible to this runtime.
    async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        Vec::new()
    }

    /// Count peers visible to this runtime.
    ///
    /// Implementations can override this to avoid materializing a full peer list.
    async fn peer_count(&self) -> usize {
        self.peers().await.len()
    }

    #[doc(hidden)]
    async fn send_and_stream(
        &self,
        cmd: CommsCommand,
    ) -> Result<(SendReceipt, EventStream), SendAndStreamError> {
        let receipt = self.send(cmd).await?;
        Err(SendAndStreamError::StreamAttach {
            receipt,
            error: StreamError::Internal(
                "send_and_stream is not implemented for this runtime".to_string(),
            ),
        })
    }

    /// Get a notification when new messages arrive
    fn inbox_notify(&self) -> Arc<tokio::sync::Notify>;
    /// Get an event injector for this runtime's inbox.
    ///
    /// Surfaces use this to push external events into the agent inbox.
    /// Returns `None` if the implementation doesn't support event injection.
    fn event_injector(&self) -> Option<Arc<dyn crate::EventInjector>> {
        None
    }

    /// Internal runtime seam for interaction-scoped streaming.
    #[doc(hidden)]
    fn interaction_event_injector(
        &self,
    ) -> Option<Arc<dyn crate::event_injector::SubscribableInjector>> {
        None
    }

    /// Look up and remove a one-shot subscriber for the given interaction.
    ///
    /// Returns the event sender if a subscriber was registered (via `inject_with_subscription`).
    /// The entry is removed from the registry on lookup (one-shot).
    fn interaction_subscriber(
        &self,
        _id: &crate::interaction::InteractionId,
    ) -> Option<tokio::sync::mpsc::Sender<crate::event::AgentEvent>> {
        None
    }

    /// Take and clear the one-shot sender for an interaction-scoped stream.
    fn take_interaction_stream_sender(
        &self,
        _id: &crate::interaction::InteractionId,
    ) -> Option<tokio::sync::mpsc::Sender<crate::event::AgentEvent>> {
        self.interaction_subscriber(_id)
    }

    /// Signal that an interaction has reached a terminal state (complete or failed).
    ///
    /// Implementations should transition the reservation FSM to `Completed` and
    /// clean up registry entries. Called from the keep-alive loop after sending
    /// terminal events to the tap.
    fn mark_interaction_complete(&self, _id: &crate::interaction::InteractionId) {}

    /// Signal that an interaction stream became unusable for an explicit,
    /// typed reason. Implementations with machine-owned stream lifecycle must
    /// drive `InteractionStreamAbandoned`; transport-only implementations may
    /// clean up their local projection directly.
    fn abandon_interaction_stream(
        &self,
        _id: &crate::interaction::InteractionId,
        _reason: crate::InteractionStreamAbandonReason,
    ) {
    }

    /// Access the session's peer-interaction DSL handle (W1-A).
    ///
    /// Returns `None` for transport-only comms runtimes. A runtime that emits
    /// semantic peer request/response receipts must return `Some` after the
    /// surface installs machine authority.
    fn peer_interaction_handle(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::handles::PeerInteractionHandle>> {
        None
    }

    /// Access peer request/response authority only when the runtime has the
    /// complete machine-owned lifecycle pair.
    ///
    /// Semantic peer request/response ingress requires both the peer
    /// interaction handle and the paired interaction-stream handle. The stream
    /// handle itself stays hidden behind runtime ownership; this witness lets
    /// authority boundaries fail closed instead of treating a lone peer handle
    /// as sufficient.
    fn peer_request_response_authority_handle(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::handles::PeerInteractionHandle>> {
        None
    }

    /// Non-destructively claim at most one classified FIFO-head interaction.
    ///
    /// A DurableRuntime head remains queued until committed with an opaque
    /// runtime admission receipt. A VolatileControl head may be handed off by
    /// exact claim-id CAS. Dropping either claim releases it synchronously.
    async fn claim_classified_inbox_interaction(
        &self,
    ) -> Result<Option<crate::interaction::PeerIngressQueueClaim>, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "claim_classified_inbox_interaction".to_string(),
        ))
    }

    /// Hand off one volatile-control FIFO head by an exact claim.
    ///
    /// A durable-runtime head is retained and reported as no volatile item.
    async fn handoff_one_volatile_peer_input_candidate(
        &self,
    ) -> Result<Option<crate::interaction::PeerInputCandidate>, CommsCapabilityError> {
        let Some(claim) = self.claim_classified_inbox_interaction().await? else {
            return Ok(None);
        };
        if claim.delivery_contract()
            == crate::interaction::PeerIngressDeliveryContract::DurableRuntime
        {
            return Ok(None);
        }
        claim
            .__handoff_volatile()
            .map_err(|error| CommsCapabilityError::HandoffFailed(error.to_string()))
    }

    /// Hand off the contiguous volatile-control FIFO prefix by exact claims.
    ///
    /// Durable-runtime heads remain queued for AcceptWithCompletion. Unlike
    /// the removed drain compatibility seams, this method never turns an
    /// unsupported capability or a failed claim CAS into an empty batch.
    async fn handoff_volatile_peer_input_candidates(
        &self,
    ) -> Result<Vec<crate::interaction::PeerInputCandidate>, CommsCapabilityError> {
        let mut candidates = Vec::new();
        loop {
            let Some(candidate) = self.handoff_one_volatile_peer_input_candidate().await? else {
                break;
            };
            candidates.push(candidate);
        }
        Ok(candidates)
    }

    /// Snapshot the currently queued peer-ingress surface without draining it.
    ///
    /// The result is a read-only projection. Claim ids, correlations, counts,
    /// and ages exposed here cannot authorize queue mutation.
    async fn peer_ingress_queue_snapshot(
        &self,
    ) -> Result<crate::interaction::PeerIngressQueueSnapshot, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "peer_ingress_queue_snapshot".to_string(),
        ))
    }

    /// Snapshot the current peer runtime surface for read-only diagnostics.
    ///
    /// This extends the queued ingress projection with the currently observed
    /// local trust membership. It cannot mutate trust or admission state.
    async fn peer_ingress_runtime_snapshot(
        &self,
    ) -> Result<crate::interaction::PeerIngressRuntimeSnapshot, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "peer_ingress_runtime_snapshot".to_string(),
        ))
    }

    /// Snapshot only the public trust projection owned by generated public
    /// peer authority.
    ///
    /// Private/control-plane trust edges are admitted by separate generated
    /// private authority and must not be reconciled or removed by public peer
    /// projection owners.
    async fn public_trusted_peer_projection_snapshot(
        &self,
    ) -> Result<Vec<crate::comms::TrustedPeerDescriptor>, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "public_trusted_peer_projection_snapshot".to_string(),
        ))
    }

    /// Snapshot the public trust projection owned by one generated source.
    ///
    /// This is the behavior-authority read used by generated trust
    /// reconciliation. Compatibility/public snapshots may still union public
    /// rows for display, but generated removals must diff only against rows
    /// previously installed by the same generated owner.
    async fn trusted_peer_projection_snapshot_for_source(
        &self,
        _source_kind: crate::comms::GeneratedCommsTrustAuthoritySourceKind,
    ) -> Result<Vec<crate::comms::TrustedPeerDescriptor>, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "trusted_peer_projection_snapshot_for_source".to_string(),
        ))
    }

    /// Get a notification that fires only for actionable peer input.
    ///
    /// Default returns `Unsupported`. Comms-enabled runtimes must override.
    /// Used by the factory to bridge into `WaitTool` interrupt.
    fn actionable_input_notify(&self) -> Result<Arc<tokio::sync::Notify>, CommsCapabilityError> {
        Err(CommsCapabilityError::Unsupported(
            "actionable_input_notify".to_string(),
        ))
    }

    /// Stage a one-shot reply endpoint for a Response to a peer outside the
    /// trust store.
    ///
    /// This is the legacy uncorrelated compatibility seam. It is
    /// Response-only, one-shot, and trust-store-losing; callers may supply
    /// only a machine-authorized endpoint already held in runtime state.
    /// Neither a Request's `reply_endpoint` nor any decoded payload/sender
    /// address is authority for this method. New ingress response paths use
    /// [`Self::stage_correlated_reply_endpoint`] instead.
    ///
    /// Parameters are primitives because core cannot name the comms-crate
    /// newtypes (dependency direction). Default fails typed, not no-op:
    /// silently dropping a reply-repair staging would strand the remote
    /// sender in a timeout with no cause. Callers decide policy — reply
    /// drains treat `Unsupported` as "runtime has no staging capability" and
    /// proceed, since in-proc runtimes resolve via the ingress route anyway.
    async fn stage_declared_reply_endpoint(
        &self,
        _dest: PeerId,
        _signer_pubkey: [u8; 32],
        _declared_address: String,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "declared reply endpoint staging not supported".to_string(),
        ))
    }

    /// Stage an authenticated one-shot endpoint for the Response correlated
    /// to `in_reply_to` from `dest`.
    ///
    /// Unlike the legacy uncorrelated staging seam above, this endpoint is
    /// keyed by both peer identity and request id and therefore takes
    /// precedence over durable trust only for that exact Response. This is
    /// the only Request-ingress callback seam. `signer_pubkey` must
    /// come from a signature-verified envelope and derive `dest` in the
    /// concrete runtime. `declared_endpoint` must be the classifier's
    /// source-confined TCP projection: kernel-observed source IP plus the
    /// signed, nonzero declared port. Arbitrary payload addresses,
    /// sender-selected hosts, UDS addresses, and open-auth ingress are never
    /// callback authority.
    async fn stage_correlated_reply_endpoint(
        &self,
        _dest: PeerId,
        _in_reply_to: crate::interaction::InteractionId,
        _signer_pubkey: [u8; 32],
        _declared_endpoint: crate::comms::PeerAddress,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "correlated reply endpoint staging not supported".to_string(),
        ))
    }

    /// Idempotently discard a previously staged correlated endpoint.
    /// Responders call this when validation or response sending fails before
    /// the Router consumes the exact one-shot entry.
    async fn unstage_correlated_reply_endpoint(
        &self,
        _dest: PeerId,
        _in_reply_to: crate::interaction::InteractionId,
    ) -> Result<(), SendError> {
        Err(SendError::Unsupported(
            "correlated reply endpoint cleanup not supported".to_string(),
        ))
    }

    /// One-shot reply waiter for an agent-blocking bridge request (member
    /// upcall lane). Consulted by the comms drain BEFORE session injection: a
    /// taken waiter receives the terminal Response candidate (typed
    /// terminality intact) and the candidate never becomes session input.
    ///
    /// Returns `Some(sender)` only for a live waiter. A tombstoned (timed
    /// out) waiter entry is consumed and `None` is returned — pair with
    /// [`Self::has_bridge_reply_waiter`] to distinguish "tombstone consumed"
    /// (discard the late reply) from "never registered" (ordinary session
    /// path). Default: no registry (a query, not a capability — absence of a
    /// waiter is the universal normal case).
    fn take_bridge_reply_waiter(
        &self,
        _in_reply_to: &crate::interaction::InteractionId,
    ) -> Option<tokio::sync::oneshot::Sender<crate::interaction::PeerInputCandidate>> {
        None
    }

    /// True when a bridge-reply waiter entry (live or tombstoned) is
    /// registered for `in_reply_to`. See [`Self::take_bridge_reply_waiter`].
    fn has_bridge_reply_waiter(&self, _in_reply_to: &crate::interaction::InteractionId) -> bool {
        false
    }
}

/// Session-scoped comms command surface that projects successful peer sends
/// onto the observe-only post-commit hook dispatcher.
pub struct ObservedCommsSender {
    runtime: Arc<dyn CommsRuntime>,
    post_commit_hooks: Arc<crate::hooks::PostCommitHookDispatcher>,
}

impl ObservedCommsSender {
    pub fn new(
        runtime: Arc<dyn CommsRuntime>,
        post_commit_hooks: Arc<crate::hooks::PostCommitHookDispatcher>,
    ) -> Self {
        Self {
            runtime,
            post_commit_hooks,
        }
    }

    pub async fn send(
        &self,
        command: CommsCommand,
    ) -> Result<crate::comms::SendReceipt, crate::comms::SendError> {
        let observation_command = command.clone();
        let receipt = self.runtime.send(command).await?;
        if let Some(observation) =
            crate::hooks::HookObservation::from_committed_peer_send(&observation_command, &receipt)
        {
            self.post_commit_hooks.dispatch(observation);
        }
        Ok(receipt)
    }
}

/// The usage account of a run that suspended for callback results.
#[derive(Debug, Clone)]
pub(crate) struct SuspendedRunUsage {
    /// The suspended run. Only callback results staged for this run admit
    /// the account to continue.
    pub(crate) run_id: RunId,
    pub(crate) baseline: crate::types::Usage,
    pub(crate) rows: Vec<crate::types::TurnUsage>,
    /// Set once this run's staged callback results are applied.
    pub(crate) callback_results_applied: bool,
}

/// Which sinks received the current run's lifecycle events: the event tap
/// and the run's event stream each get an event on its own, the tap before
/// the stream send resolves, so a hard interrupt can drop the run future
/// after one sink got an event and before the other did.
///
/// A dropped run's terminal is published from these facts, per sink: to a
/// sink that got no terminal yet, and only when that sink saw the run start
/// (or the run announces no start). The tap and the stream therefore each see
/// at most one terminal, and never a terminal without its start.
#[derive(Debug, Default)]
pub(crate) struct RunLifecyclePublication {
    /// The run publishes `RunStarted`.
    start_announced: std::sync::atomic::AtomicBool,
    started_tapped: std::sync::atomic::AtomicBool,
    started_streamed: std::sync::atomic::AtomicBool,
    terminal_tapped: std::sync::atomic::AtomicBool,
    terminal_streamed: std::sync::atomic::AtomicBool,
}

/// A run lifecycle event's place in [`RunLifecyclePublication`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunLifecycleEvent {
    Started,
    Terminal,
}

/// Where a dropped run's terminal is still owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwedRunTerminal {
    pub(crate) tap: bool,
    pub(crate) stream: bool,
}

impl RunLifecyclePublication {
    fn flag(&self, event: RunLifecycleEvent, stream: bool) -> &std::sync::atomic::AtomicBool {
        match (event, stream) {
            (RunLifecycleEvent::Started, false) => &self.started_tapped,
            (RunLifecycleEvent::Started, true) => &self.started_streamed,
            (RunLifecycleEvent::Terminal, false) => &self.terminal_tapped,
            (RunLifecycleEvent::Terminal, true) => &self.terminal_streamed,
        }
    }

    fn load(flag: &std::sync::atomic::AtomicBool) -> bool {
        flag.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn announce_start(&self) {
        self.start_announced
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn mark_tapped(&self, event: RunLifecycleEvent) {
        self.flag(event, false)
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) fn mark_streamed(&self, event: RunLifecycleEvent) {
        self.flag(event, true)
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Where a terminal for a run whose future was dropped is still owed.
    pub(crate) fn owed_terminal(&self) -> OwedRunTerminal {
        let announced = Self::load(&self.start_announced);
        OwedRunTerminal {
            tap: !Self::load(&self.terminal_tapped)
                && (Self::load(&self.started_tapped) || !announced),
            stream: !Self::load(&self.terminal_streamed)
                && (Self::load(&self.started_streamed) || !announced),
        }
    }

    /// A fresh run starts with nothing published.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The human consent the current run requested itself or that its staged
/// notice inputs moved into it, and which of it the run already attempted.
/// Scheduling and notice suppression only: at most one in-run attempt per
/// decision per run (an attempt the session's rebuilt agent could not
/// resolve yet does not count), only the run's own requests and the
/// consent moved into it are examined, and the consent left parked for a
/// resumed run is moved again at most once per boundary. Spend and
/// withdrawal authority stays with the approval owner. Reset with each
/// work-authorized run scope.
#[derive(Debug, Default)]
pub(crate) struct RunConsentAttempts {
    requested: BTreeSet<crate::ApprovalId>,
    attempted: BTreeSet<crate::ApprovalId>,
    /// The run's model operation sequence at the boundary that last tried
    /// again to move the consent left parked for this run.
    moves_retried_at: Option<u64>,
}

impl RunConsentAttempts {
    /// Whether this is the first poll of the boundary after the run's model
    /// operation `sequence`, recording it: only that poll tries again to
    /// move the consent left parked for this run (ADR-001 D9), and a repoll
    /// of the same boundary while a gate defers calls no owner again.
    pub(crate) fn first_poll_of_boundary(&mut self, sequence: u64) -> bool {
        self.moves_retried_at.replace(sequence) != Some(sequence)
    }

    /// The run's own dispatch committed a consent request for one call.
    pub(crate) fn requested(&mut self, approval_id: &crate::ApprovalId) {
        self.requested.insert(approval_id.clone());
    }

    /// The approval owner moved `approval_id`'s consent into this resumed
    /// run by the committed receipt of one of its staged notice inputs: the
    /// run examines it as its own, and still attempts it at most once.
    pub(crate) fn moved_in(&mut self, approval_id: &crate::ApprovalId) {
        self.requested.insert(approval_id.clone());
    }

    /// The consent this run requested, or that moved into it, and has not
    /// attempted yet.
    pub(crate) fn unattempted(&self) -> Vec<crate::ApprovalId> {
        self.requested
            .difference(&self.attempted)
            .cloned()
            .collect()
    }

    /// Record `approval_id` as attempted in this run; false when the run
    /// already attempted it.
    pub(crate) fn first_attempt(&mut self, approval_id: &crate::ApprovalId) -> bool {
        self.attempted.insert(approval_id.clone())
    }

    /// The attempt of `approval_id` was not ready (the session's rebuilt
    /// agent could not resolve its retained call yet): it does not count,
    /// and a later boundary of the run attempts it again.
    pub(crate) fn not_attempted(&mut self, approval_id: &crate::ApprovalId) {
        self.attempted.remove(approval_id);
    }
}

/// The main Agent struct
pub struct Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized,
    S: AgentSessionStore + ?Sized,
{
    config: AgentConfig,
    client: Arc<C>,
    tools: Arc<T>,
    tool_scope: ToolScope,
    store: Arc<S>,
    session: Session,
    budget: Budget,
    retry_policy: RetryPolicy,
    depth: u32,
    pub(super) comms_runtime: Option<Arc<dyn CommsRuntime>>,
    pub(super) hook_engine: Option<Arc<dyn HookEngine>>,
    pub(super) hook_run_overrides: HookRunOverrides,
    /// Mandatory hook topology validated at this run's admission. Same-run
    /// feedback recovers only failures of its admitted gates.
    pub(super) admitted_hook_topology: crate::hooks::HookMandatoryTopology,
    pub(super) post_commit_hooks: Arc<crate::hooks::PostCommitHookDispatcher>,
    /// Optional context compaction strategy.
    pub(crate) compactor: Option<Arc<dyn crate::compact::Compactor>>,
    /// Optional host-supplied compaction summary curator. When present it
    /// produces the compaction summary instead of the summarization LLM call.
    pub(crate) compaction_curator: Option<Arc<dyn crate::compact::CompactionCurator>>,
    /// Input tokens from the last LLM response (for compaction trigger).
    pub(crate) last_input_tokens: u64,
    /// Session usage total when the current `run_loop` entry began. The
    /// run's own usage is the session total's delta from this baseline.
    pub(crate) run_usage_baseline: crate::types::Usage,
    /// Per-request usage recorded during the current run.
    pub(crate) run_request_usage: Vec<crate::types::TurnUsage>,
    /// Run account of a run that suspended for callback results. Kept only
    /// until the next run: once that run's staged callback results are
    /// applied, the next run continues it whatever entry point starts it
    /// (`run_pending` or a content turn); a run with no applied results
    /// discards it. In-memory only: an agent rebuilt from storage starts a
    /// fresh account.
    pub(crate) run_usage_suspended_run: Option<SuspendedRunUsage>,
    /// Session-scoped compaction cadence tracked across runs.
    pub(crate) compaction_cadence: SessionCompactionCadence,
    /// Machine-issued compaction check parked until the request has been fully
    /// composed, blob-hydrated, tool-scoped, and provider-lowered.
    pub(crate) pending_compaction_boundary_index: Option<u64>,
    /// Exact pressure witness attached to the parked compaction check.
    pub(crate) pending_compaction_request_pressure: Option<crate::ProviderRequestPressure>,
    /// Whole-request token budget attached to the same parked compaction check.
    ///
    /// Parked and cleared together with the pressure witness above: both are
    /// measurements of one fully composed request, and the trigger decision
    /// needs the request in both units the model boundary is limited in.
    pub(crate) pending_compaction_request_budget: Option<crate::ContextBudgetFact>,
    /// Pre-compaction pressure retained until the rebuilt request proves that
    /// compaction both decreased the body and brought it below the hard cap.
    pub(crate) post_compaction_pressure_check: Option<crate::ProviderRequestPressure>,
    /// Optional memory store for indexing compaction discards.
    pub(crate) memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,
    /// Runtime-owned resultful handoff for durable transcript+memory
    /// compaction pairs. Absent on standalone paths.
    pub(crate) compaction_commit_coordinator:
        Option<Arc<dyn crate::memory::CompactionCommitCoordinator>>,
    /// Typed lifecycle for the current transcript-rewrite + staged-memory
    /// transaction. Runtime reconciliation advances this to commit-only before
    /// touching the memory store; abort is legal only while runtime commit is
    /// still pending.
    pub(crate) compaction_transaction: Option<CompactionTransaction>,
    /// Deterministic projection identity installed immediately before the
    /// durable stage await. A hard interrupt can drop that await before a
    /// receipt reaches the transaction owner, so cleanup must retain the exact
    /// identity rather than infer empty RuntimeStore authority.
    pub(crate) in_flight_compaction_stage: Option<crate::memory::CompactionProjectionId>,
    /// Optional skill engine for per-turn `/skill-ref` activation.
    pub(crate) skill_engine: Option<Arc<crate::skills::SkillRuntime>>,
    /// Skill references to resolve and inject for the next turn.
    /// Set by surfaces before calling `run()`, consumed on run start.
    pub pending_skill_references: Option<Vec<crate::skills::SkillKey>>,
    /// Per-interaction event tap for streaming events to subscribers.
    pub(crate) event_tap: crate::event_tap::EventTap,
    /// Per-instance measurement callback, absent from ordinary builds and
    /// never consulted for authorization or turn control.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) model_preparation_observer: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Per-instance callback after each durable round commit, with the
    /// cursor it wrote; absent from ordinary builds and never consulted for
    /// authorization or turn control.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) round_commit_observer: Option<RoundCommitObserver>,
    /// Request-only exact-boundary context coordinator for this live actor.
    pub(crate) transient_turn_context_state: crate::session::TransientTurnContextStateHandle,
    /// Optional default event channel configured at build time.
    /// Used by run methods when no per-call event channel is provided.
    pub(crate) default_event_tx: Option<tokio::sync::mpsc::Sender<crate::event::AgentEvent>>,
    /// Optional session checkpointer for keep-alive persistence.
    ///
    /// Wired by `AgentBuilder::with_checkpointer`, installed by
    /// `PersistentSessionService`, and consumed only by active-run persistence.
    pub(crate) checkpointer: Option<Arc<dyn crate::SessionCheckpointer>>,
    /// Run-local slot a permanent-routing tool stages its intent into.
    ///
    /// Held here because the agent is the only owner that knows whether a run
    /// reached a clean terminal boundary, which is the sole condition under
    /// which staged intent may become a committed request.
    pub(crate) model_routing_handoff_staging:
        Option<Arc<crate::session::model_routing_handoff_staging::ModelRoutingHandoffStagingSlot>>,
    /// Latest successful provisional physical write for the active run.
    ///
    /// This is actor-local transport state, never Session domain state. The
    /// actor removes it before each checkpoint await and installs only the
    /// exact returned successor.
    pub(crate) latest_run_checkpoint_receipt: Option<crate::RunCheckpointReceipt>,
    /// The active run's per-round commit state: its round index, its bound
    /// inputs, and whether a round commit wrote a run cursor. Run-local.
    pub(crate) round_commits: state::RunRoundCommits,
    /// Input ids the session layer staged for the next run: the runtime
    /// batch's contributing inputs, which bind to the run that starts. Cleared
    /// with the runtime execution kind.
    pub(crate) run_start_input_ids: Vec<crate::lifecycle::InputId>,
    /// The staged round a resumed run takes over at its first model
    /// boundary, set by [`Agent::resume_run`] and consumed there.
    pub(crate) pending_resumed_round: Option<state::ResumedToolRound>,
    pub(crate) tool_application_observations:
        crate::tool_application::ToolApplicationInvocationOwner,
    /// Rows `[0, floor)` are committed in the durable store exactly as they
    /// stand in memory (the document loaded at build, the rows a compaction
    /// rewrite installed, or a committed successor the runtime handed over).
    /// Pre-compaction media externalization never touches them: a committed
    /// inline row's persisted form is pinned by the store's prefix proof, and
    /// mutating it on the actor would make the next commit disagree with the
    /// head row (`TranscriptContinuityViolation`). Rows at or beyond the floor
    /// are process-local until their own checkpoint externalizes them.
    pub(crate) durable_row_floor: usize,
    /// Optional blob store used to hydrate image refs at execution seams.
    pub(crate) blob_store: Option<Arc<dyn crate::BlobStore>>,
    /// Original error detail preserved from `terminalize_fatal_error` so
    /// `build_result` can include the actual failure message (e.g. the API
    /// error body) instead of only the generic terminal-cause description.
    pub(crate) terminal_error_detail: Option<String>,
    /// Structured metadata captured from that concrete error before the
    /// public result is normalized into `AgentError::TerminalFailure`.
    pub(crate) terminal_error_metadata: Option<crate::TurnErrorMetadata>,
    /// True once the current run has accepted `RunCompleted` hooks.
    pub(crate) run_completed_hooks_applied: bool,
    /// True once the current run's public `RunCompleted` event has been
    /// emitted. Extraction may continue afterward as a separate post-run phase.
    pub(crate) run_completed_event_emitted: bool,
    /// Which sinks received the current run's lifecycle events, so a run
    /// whose future a hard interrupt dropped publishes its terminal exactly
    /// where it is still owed (see [`RunLifecyclePublication`]).
    pub(crate) run_lifecycle_publication: RunLifecyclePublication,
    /// The committed assistant message whose text the current run's result
    /// repeats, referenced by `RunCompleted`. Set by the terminal commit and
    /// by `build_result`; reset at every run entry. Run-local, never
    /// persisted, and deliberately not part of `RunResult`.
    pub(crate) run_result_assistant_message: Option<crate::types::AssistantMessageId>,
    /// Comms intents that should be silently injected into the session
    /// without triggering an LLM turn. Matched against `InteractionContent::Request.intent`.
    #[allow(dead_code)] // Used by comms_impl when comms feature is enabled
    pub(crate) silent_comms_intents: Vec<String>,
    /// Optional shared lifecycle registry for async operations.
    pub(crate) ops_lifecycle: Option<Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>>,
    /// Optional completion feed for cursor-based completion delivery.
    pub(crate) completion_feed: Option<Arc<dyn crate::completion_feed::CompletionFeed>>,
    /// Shared epoch cursor state for runtime-backed cursor writeback.
    pub(crate) epoch_cursor_state: Option<Arc<crate::runtime_epoch::EpochCursorState>>,
    /// Local cursor into the completion feed — only the agent boundary advances this.
    pub(crate) applied_cursor: crate::completion_feed::CompletionSeq,
    /// Optional enrichment provider for completion display details.
    pub(crate) completion_enrichment:
        Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>>,
    /// Shared effective mob authority handle. Owned by the agent, passed to
    /// mob tools at construction for authorization reads. Updated by
    /// `apply_session_effects` after each tool batch as a derived projection
    /// of the canonical `session.build_state().mob_tool_authority_context`.
    pub(crate) mob_authority_handle:
        Option<Arc<std::sync::RwLock<crate::service::MobToolAuthorityContext>>>,
    /// Runtime-backed turn-state handle, provided by the session runtime bindings.
    pub(crate) turn_state_handle: Option<Arc<dyn crate::TurnStateHandle>>,
    /// Runtime-backed model-routing authority. Sticky fallback commits route
    /// through this handle in the compensated client/auth/machine transaction.
    pub(crate) model_routing_handle: Option<Arc<dyn crate::handles::ModelRoutingHandle>>,
    /// Runtime-owned durable sticky-fallback transaction coordinator.
    /// Standalone agents leave this absent and consume staged machine commits
    /// synchronously in-process.
    pub(crate) sticky_model_fallback_commit_coordinator:
        Option<Arc<dyn crate::handles::StickyModelFallbackCommitCoordinator>>,
    /// Saga state retained across cancellation while the supervised durable
    /// sticky-fallback transaction is in flight.
    pub(crate) pending_sticky_model_fallback_activation:
        Option<state::PendingStickyModelFallbackActivation>,
    /// Async operation references staged behind an external callback boundary.
    /// They are registered with the fresh continuation run before it can call
    /// the provider, preserving Barrier versus Detached semantics.
    pub(crate) pending_callback_async_ops: Option<Vec<crate::ops::AsyncOpRef>>,
    /// Effective model registry captured by the construction pipeline.
    /// Fallback profile and limit truth is freshly resolved through this exact
    /// registry before it can reach the routing machine.
    pub(crate) effective_model_registry: Option<Arc<crate::ModelRegistry>>,
    /// Registry-minted facts for the active model. This replaces client-local
    /// capability/limit projections as the durable source used by later turns.
    pub(crate) active_model_profile: Option<crate::ModelProfileWitness>,
    /// True when the runtime control plane must stamp execution kind metadata.
    pub(crate) runtime_execution_kind_required: bool,
    /// Resources this agent's construction handed over that must be released
    /// when the agent's owner ends it (for example its MCP router's own
    /// lineages of a session registration's surface).
    pub(crate) owned_releases: Vec<Arc<dyn AgentOwnedRelease>>,
    /// Typed execution intent for the current run, when this turn is owned by
    /// the runtime control plane rather than a direct surface call.
    pub(crate) runtime_execution_kind: Option<crate::lifecycle::RuntimeExecutionKind>,
    /// Exact per-call witness that the core turn machine admitted a runtime
    /// run. A completed future alone is not sufficient evidence: preflight
    /// failures can return before `StartConversationRun` and must never reuse
    /// the previous turn's terminal snapshot.
    pub(crate) runtime_started_run_id: Option<crate::lifecycle::RunId>,
    /// Machine-terminal failure observed for the exact runtime run above.
    /// Kept separate from the public `AgentError` so direct session surfaces
    /// preserve their original typed errors while the runtime can commit a
    /// failed-but-applied turn atomically.
    pub(crate) runtime_terminal_failure_witness:
        Option<Result<crate::TurnErrorMetadata, crate::error::AgentError>>,
    /// Stable transcript identity for the active runtime-owned turn.
    pub(crate) active_transcript_identity: Option<crate::types::TranscriptMessageIdentity>,
    /// Request-only host context for the active runtime-owned logical turn.
    ///
    /// This value is never appended to `session`; request composition projects
    /// it immediately before the admitted conversational user message.
    pub(crate) active_turn_request_contexts:
        Vec<crate::lifecycle::run_primitive::TurnRequestContext>,
    /// The active runtime-owned turn's resolved reasoning preference, lowered
    /// onto each provider attempt's own request copy; never session state.
    pub(crate) active_turn_request_reasoning:
        Option<crate::lifecycle::run_primitive::ReasoningBatchDisposition>,
    /// Runtime-backed external tool-surface diagnostic handle, when provided
    /// by the session runtime bindings.
    pub(crate) external_tool_surface_handle: Option<Arc<dyn crate::ExternalToolSurfaceHandle>>,
    /// This member's live MCP attachment surface: its construction's own
    /// router, when a governed session-owned construction built one.
    pub(crate) live_mcp_attachment:
        Option<Arc<dyn crate::mcp_attachment::LiveMcpAttachmentSurface>>,
    /// Runtime-backed auth lease handle (Phase 1.5-rev).
    pub(crate) auth_lease_handle: Option<crate::handles::GeneratedAuthLeaseHandle>,
    /// Credential authority paired with the currently active LLM route.
    pub(crate) auth_credential_identity: Option<crate::AuthCredentialIdentity>,
    /// Runtime-backed MCP server lifecycle handle (Phase 5G / T5g). When set,
    /// the agent loop reads `pending_server_ids()` at each CallingLlm boundary
    /// to decide whether to emit the `[MCP_PENDING]` system notice.
    pub(crate) mcp_server_lifecycle_handle:
        Option<Arc<dyn crate::handles::McpServerLifecycleHandle>>,
    /// Producer end of the typed cancel-after-boundary command channel.
    ///
    /// Retained so [`Agent::cancel_after_boundary_handle`] can hand cloned
    /// senders to the surface that requests boundary-only cancellation. The
    /// agent never sends on this end itself; it only drains the matching
    /// receiver at turn boundaries.
    pub(crate) cancel_after_boundary_tx: CancelAfterBoundarySender,
    /// Consumer end of the typed cancel-after-boundary command channel.
    ///
    /// Drained (non-blocking) at each turn boundary by
    /// `observe_cancel_after_boundary_request`, replacing the previous
    /// `.swap`-polled `AtomicBool`. A delivered [`CancelAfterBoundaryCommand`]
    /// is observed at most once per boundary, mirroring the prior edge
    /// semantics.
    pub(crate) cancel_after_boundary_rx:
        tokio::sync::mpsc::UnboundedReceiver<CancelAfterBoundaryCommand>,
    /// Optional resolver for model-specific operational defaults (e.g., call timeout).
    /// Consulted at each LLM call for hot-swap-aware profile default resolution.
    pub(crate) model_defaults_resolver:
        Option<Arc<dyn crate::model_defaults::ModelOperationalDefaultsResolver>>,
    /// Explicit call-timeout override from the build/config composition seam.
    /// Takes precedence over profile-derived defaults.
    pub(crate) call_timeout_override: crate::config::CallTimeoutOverride,
    /// Explicit stream-inactivity watchdog override from the config
    /// composition seam. Takes precedence over the current model's own window;
    /// `Inherit` falls back to it, then to the retry policy's window.
    pub(crate) stream_inactivity_timeout_override: crate::config::CallTimeoutOverride,
    /// Structured-output extraction state carried into RunResult.
    pub(crate) extraction_state: extraction::ExtractionState,
    /// Last published hidden deferred-catalog names.
    pub(crate) last_hidden_deferred_catalog_names: BTreeSet<crate::types::ToolName>,
    /// Last published pending catalog sources.
    pub(crate) last_pending_catalog_sources: BTreeSet<String>,
    /// Dispatch-time projection of the current turn input for contextual tools.
    pub(crate) tool_dispatch_context: ToolDispatchContext,
    /// Trusted host operation review composition, forwarded to every tool
    /// dispatch context. It only enables review; the tier stays authorization's.
    pub(crate) operation_review: Option<Arc<crate::approval::review::BoundOperationReview>>,
    /// The host's operation refusal observer, forwarded to every tool
    /// dispatch context with this agent's session. Host-only: it never
    /// reaches the event stream or the model.
    pub(crate) refusal_observer: Option<Arc<dyn crate::authorization::OperationRefusalObserver>>,
    /// Human consent the current run requested or that moved into it, and
    /// its in-run attempts.
    pub(crate) run_consent_attempts: RunConsentAttempts,
    /// Exact per-operation dispatch authority for a noncommitting live bridge
    /// run. Ordinary turns always leave this absent.
    pub(crate) live_bridge_dispatch_admission: Option<LiveBridgeToolDispatchAdmission>,
    /// Immutable provider-visible tool surface captured from the durable
    /// member at bridge preparation.
    pub(crate) live_bridge_tool_defs: Option<Arc<[Arc<ToolDef>]>>,
    /// Suppress every SessionStore/checkpointer write while the live member is
    /// temporarily executing against a retained canonical snapshot clone.
    pub(crate) noncommitting_live_bridge_run: bool,
    /// Runtime-owned dispatch metadata for this turn.
    pub(crate) turn_tool_dispatch_metadata: BTreeMap<String, serde_json::Value>,
    /// This turn's tool-choice plan from the per-turn overlay: entry `k` is
    /// the choice for the run's `k`-th provider call, `Auto` once exhausted.
    /// Set and cleared with the overlay, so it is run-local by construction.
    pub(crate) turn_tool_choice_plan: Vec<crate::lifecycle::run_primitive::ToolChoice>,
    /// Typed tool-execution policy (per-call timeouts + concurrency bound)
    /// applied to the normal LLM-driven tool dispatch loop. Populated by the
    /// composition seam via `AgentBuilder::with_tools_config`; defaults to
    /// `ToolsConfig::default()` for standalone/test construction.
    pub(crate) tools_config: crate::config::ToolsConfig,
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized,
    S: AgentSessionStore + ?Sized,
{
    /// Hand this agent a resource its construction must release when the
    /// agent's owner ends it.
    pub fn retain_owned_release(&mut self, release: Arc<dyn AgentOwnedRelease>) {
        self.owned_releases.push(release);
    }

    /// Release every resource this agent's construction handed it, awaiting
    /// each release. The owner ending the agent calls this before it reports
    /// the agent gone. Limits: the list is taken before the first await, so
    /// a release cancelled midway drops the rest unreleased; a panicked or
    /// aborted owner never calls it; and a completed release (for an MCP
    /// router, its unit-returning shutdown) is not proof that a server
    /// process exited.
    pub async fn release_owned_resources(&mut self) {
        for release in std::mem::take(&mut self.owned_releases) {
            release.release().await;
        }
    }

    /// Retain the existing selection owner's actual immutable client for
    /// native setup. This is process resource custody, never admission.
    /// Mutable selectors must pin their selected child, not themselves.
    pub fn pin_controller_client(&self) -> Option<crate::ControllerModelClient> {
        let controller = Arc::clone(&self.client).pin_controller()?;
        (self.client.controller_model_selection().as_ref() == Some(controller.selection())
            && controller.client().controller_model_selection().as_ref()
                == Some(controller.selection()))
        .then_some(controller)
    }

    /// The installed operation refusal observer, bound to this agent's
    /// session.
    pub(crate) fn refusal_observer_binding(
        &self,
    ) -> Option<crate::authorization::RefusalObserverBinding> {
        self.refusal_observer.as_ref().map(|observer| {
            crate::authorization::RefusalObserverBinding::new(
                Arc::clone(observer),
                self.session.id().clone(),
            )
        })
    }

    /// The context of a tool dispatch this agent runs itself (its loop's
    /// tool batch, an external tool call, an in-run consent attempt) under
    /// `review`, with the installed operation refusal observer bound to this
    /// agent's session. Each of them takes its context here, so none leaves
    /// the observer out.
    pub(crate) fn observed_tool_dispatch_context(
        &self,
        review: Option<Arc<crate::approval::review::BoundOperationReview>>,
    ) -> ToolDispatchContext {
        self.tool_dispatch_context
            .clone()
            .with_operation_review(review)
            .with_refusal_observer(self.refusal_observer_binding())
    }
}

#[derive(Clone)]
pub(crate) struct CompactionRollbackState {
    pub(crate) rollback_session: Session,
    pub(crate) rollback_last_input_tokens: u64,
    pub(crate) rollback_compaction_cadence: SessionCompactionCadence,
    pub(crate) rollback_durable_row_floor: usize,
    /// Durable boundary applies that had happened on this actor when the
    /// rollback session was captured. A restore discards every later apply,
    /// so its delivery witness is marked discarded and the runtime redelivers
    /// that input instead of consuming it.
    pub(crate) rollback_durable_boundary_apply_ordinal: u64,
}

pub(crate) enum CompactionTransactionPhase {
    AwaitingRuntimeCommit(Box<CompactionRollbackState>),
    RuntimeCommitted { bookkeeping_complete: bool },
    AbortPending { cadence_persist_pending: bool },
}

pub(crate) struct CompactionTransaction {
    pub(crate) phase: CompactionTransactionPhase,
    pub(crate) projections: Vec<crate::memory::CompactionProjectionId>,
    /// The intents the session already carried when the transaction began:
    /// a run resumed from a provisional tail that holds an earlier rewrite's
    /// intent, which commits with this transaction's at the run's boundary.
    /// They are expected in the outbox and finalized with it, but their
    /// stages are not this transaction's to abort.
    pub(crate) carried_projections: Vec<crate::memory::CompactionProjectionId>,
    /// A run checkpoint wrote the rewrite, with its intents, into the run's
    /// provisional tail while the transaction awaited the runtime commit.
    /// The tail's settlement decides the rewrite from then on (an
    /// interrupted run's tail is committed with the intents), so an abort no
    /// longer restores the rollback image or aborts the stages.
    pub(crate) tail_carries_rewrite: bool,
}

impl CompactionTransaction {
    /// The projections the run's boundary commits with this transaction: the
    /// intents the session carried when it began, then its own.
    pub(crate) fn committed_projections(&self) -> Vec<crate::memory::CompactionProjectionId> {
        let mut expected = self.carried_projections.clone();
        expected.extend(
            self.projections
                .iter()
                .filter(|projection| !self.carried_projections.contains(projection))
                .cloned(),
        );
        expected
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::{
        AgentToolDispatcher, CommsRuntime, DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS,
        FilteredToolDispatcher, InlinePeerNotificationPolicy, ToolDispatchContext,
    };
    use crate::comms::{
        PeerAddress, PeerId, PeerName, PeerTransport, SendError, TrustedPeerDescriptor,
    };
    use crate::types::{ContentBlock, ContentInput, ToolCallView, ToolDef, ToolResult};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Notify;

    struct NoopCommsRuntime {
        notify: Arc<Notify>,
    }

    struct ContextAwareToolDispatcher;

    struct ExactExecutionDispatcher {
        catalog: Arc<[crate::ToolCatalogEntry]>,
    }

    struct HybridExecutionDispatcher {
        catalog: Arc<[crate::ToolCatalogEntry]>,
    }

    struct StreamingExecutionDispatcher {
        catalog: Arc<[crate::ToolCatalogEntry]>,
        saw_streaming_context: Arc<std::sync::atomic::AtomicBool>,
    }

    struct IdenticalMutationDispatcher {
        tool: ToolDef,
        epoch: std::sync::atomic::AtomicU64,
        mutate_on_resolve: bool,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for ContextAwareToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([Arc::new(ToolDef {
                audience: Default::default(),
                name: "inspect_context".into(),
                description: "inspect context".to_string(),
                input_schema: json!({"type": "object"}),
                provenance: None,
            })])
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": false}).to_string(),
                false,
            )
            .into())
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &ToolDispatchContext,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            let saw_context_image = context
                .current_turn()
                .and_then(|turn| turn.image_ref(0))
                .and_then(|image_ref| context.current_turn_image(image_ref))
                .is_some();
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"saw_context_image": saw_context_image}).to_string(),
                false,
            )
            .into())
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for ExactExecutionDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "ok".to_string(), false).into())
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for HybridExecutionDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .filter(|entry| entry.currently_callable())
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        fn resolve_execution_plan(
            &self,
            call: ToolCallView<'_>,
            _dispatch_context: &ToolDispatchContext,
            resolution_context: &crate::ToolExecutionResolutionContext,
        ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
            let entry = self
                .catalog
                .iter()
                .find(|entry| entry.tool.name == call.name)
                .ok_or_else(|| crate::ToolExecutionResolutionError::NotFound {
                    tool_name: call.name.to_string(),
                })?;
            let arguments: serde_json::Value =
                serde_json::from_str(call.args.get()).map_err(|error| {
                    crate::ToolExecutionResolutionError::InvalidArguments {
                        tool_name: call.name.to_string(),
                        reason: error.to_string(),
                    }
                })?;
            let mode = if arguments["run_detached"] == true {
                crate::ToolExecutionMode::Detached
            } else {
                crate::ToolExecutionMode::Fast
            };
            entry
                .execution
                .resolve(mode, resolution_context.deadlines().clone())
                .map_err(crate::ToolExecutionResolutionError::from)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            Ok(ToolResult::new(
                call.id.to_string(),
                json!({"owner": "filtered-hybrid-owner"}).to_string(),
                false,
            )
            .into())
        }

        async fn dispatch_resolved_with_context(
            &self,
            call: ToolCallView<'_>,
            _context: &ToolDispatchContext,
            plan: &crate::ResolvedToolExecutionPlan,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            if plan.mode() != crate::ToolExecutionMode::Detached {
                return Err(crate::ToolError::execution_failed(
                    "test detached owner received the wrong plan",
                ));
            }
            self.dispatch(call).await
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for StreamingExecutionDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            Err(crate::ToolError::unavailable(
                call.name,
                crate::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
            ))
        }

        async fn dispatch_resolved_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &ToolDispatchContext,
            plan: &crate::ResolvedToolExecutionPlan,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            if plan.mode() != crate::ToolExecutionMode::Streaming {
                return Err(crate::ToolError::execution_failed(
                    "streaming owner received a non-streaming plan",
                ));
            }
            let streaming = context.streaming().ok_or_else(|| {
                crate::ToolError::unavailable(
                    call.name,
                    crate::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
                )
            })?;
            streaming
                .progress()
                .try_report(
                    crate::ToolProgressFrame::message("accepted through wrapper")
                        .map_err(|error| crate::ToolError::other(error.to_string()))?,
                )
                .map_err(|error| crate::ToolError::other(error.to_string()))?;
            self.saw_streaming_context
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolResult::new(call.id.to_string(), "stream complete".to_string(), false).into())
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for IdenticalMutationDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([Arc::new(self.tool.clone())])
        }

        fn tool_catalog_capabilities(&self) -> crate::ToolCatalogCapabilities {
            crate::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[crate::ToolCatalogEntry]> {
            Arc::from([crate::ToolCatalogEntry::session_inline(
                Arc::new(self.tool.clone()),
                true,
            )])
        }

        fn execution_binding_epoch(&self, _tool_name: &str) -> u64 {
            self.epoch.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn resolve_execution_plan(
            &self,
            _call: ToolCallView<'_>,
            _dispatch_context: &ToolDispatchContext,
            resolution_context: &crate::ToolExecutionResolutionContext,
        ) -> Result<crate::ResolvedToolExecutionPlan, crate::ToolExecutionResolutionError> {
            let plan = crate::ToolExecutionContract::default()
                .resolve_default(resolution_context.deadlines().clone())
                .map_err(crate::ToolExecutionResolutionError::from)?;
            if self.mutate_on_resolve {
                self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(plan)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "ok".to_string(), false).into())
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl CommsRuntime for NoopCommsRuntime {
        fn inbox_notify(&self) -> std::sync::Arc<Notify> {
            self.notify.clone()
        }
    }

    #[tokio::test]
    async fn test_comms_runtime_trait_defaults_hide_unimplemented_features() {
        let runtime = NoopCommsRuntime {
            notify: Arc::new(Notify::new()),
        };
        assert!(<NoopCommsRuntime as CommsRuntime>::public_key(&runtime).is_none());
        // The only mutable trust seam is apply_trust_mutation; without a
        // generated handoff it fails closed.
        let peer = TrustedPeerDescriptor {
            peer_id: PeerId::new(),
            name: PeerName::new("peer-a").expect("valid peer name"),
            address: PeerAddress::new(PeerTransport::Inproc, "peer-a"),
            pubkey: [0u8; 32],
        };
        let result =
            <NoopCommsRuntime as CommsRuntime>::add_private_trusted_peer(&runtime, peer).await;
        assert!(matches!(result, Err(SendError::Unsupported(_))));
    }

    /// T-12: bridge-reply waiter + declared-reply-endpoint trait defaults.
    /// `take_bridge_reply_waiter` → None (no registry),
    /// `has_bridge_reply_waiter` → false, and
    /// `stage_declared_reply_endpoint` fails typed (never a silent no-op) so
    /// a caller cannot mistake a dropped security-relevant repair for success.
    #[tokio::test]
    async fn test_comms_runtime_bridge_reply_defaults() {
        let runtime = NoopCommsRuntime {
            notify: Arc::new(Notify::new()),
        };
        let interaction_id = crate::interaction::InteractionId(uuid::Uuid::new_v4());
        assert!(
            <NoopCommsRuntime as CommsRuntime>::take_bridge_reply_waiter(&runtime, &interaction_id)
                .is_none()
        );
        assert!(
            !<NoopCommsRuntime as CommsRuntime>::has_bridge_reply_waiter(&runtime, &interaction_id)
        );
        let staged = <NoopCommsRuntime as CommsRuntime>::stage_declared_reply_endpoint(
            &runtime,
            PeerId::new(),
            [0x11u8; 32],
            "tcp://127.0.0.1:1".to_string(),
        )
        .await;
        assert!(matches!(staged, Err(SendError::Unsupported(_))));
    }

    #[tokio::test]
    async fn filtered_tool_dispatcher_preserves_dispatch_context() {
        let dispatcher =
            FilteredToolDispatcher::new(Arc::new(ContextAwareToolDispatcher), ["inspect_context"]);
        let args = serde_json::value::RawValue::from_string("{}".to_string())
            .expect("empty object should be valid JSON");
        let call = ToolCallView {
            id: "ctx-1",
            name: "inspect_context",
            args: &args,
        };
        let context = ToolDispatchContext::from_current_turn_input(&ContentInput::Blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abc".into(),
            },
        ]));

        let outcome = dispatcher
            .dispatch_with_context(call, &context)
            .await
            .expect("filtered wrapper should dispatch");
        let payload: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).expect("tool result JSON");
        assert_eq!(payload["saw_context_image"], true);
    }

    #[test]
    fn default_execution_plan_resolver_uses_exact_catalog_contract() {
        use crate::{
            DetachedToolExecutionPolicy, IdempotencyScope, RestartClass, RunnerIdentity,
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner, ToolExecutionContract,
            ToolExecutionMode, ToolExecutionResolutionContext,
        };
        use std::collections::BTreeSet;
        use std::time::Duration;

        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("example.security_scan", "v1").unwrap(),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .unwrap();
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .unwrap();
        let dispatcher = ExactExecutionDispatcher {
            catalog: Arc::from([crate::ToolCatalogEntry::session_inline(
                Arc::new(ToolDef::new(
                    "security_scan",
                    "scan",
                    json!({"type": "object"}),
                )),
                true,
            )
            .with_execution_contract(contract)]),
        };
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "call-1",
            name: "security_scan",
            args: &args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );

        let plan = dispatcher
            .resolve_execution_plan(call, &ToolDispatchContext::default(), &resolution)
            .expect("declared plan resolves");

        assert_eq!(plan.mode(), ToolExecutionMode::Detached);
        assert_eq!(
            plan.deadlines().effective_timeout(),
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            plan.deadlines().winner().map(|winner| winner.owner()),
            Some(ToolDeadlineOwner::DetachedSubmission)
        );
    }

    #[tokio::test]
    async fn default_resolved_dispatch_refuses_detached_plan_before_ordinary_dispatch() {
        use crate::{
            DetachedToolExecutionPolicy, IdempotencyScope, RestartClass, RunnerIdentity,
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner, ToolExecutionContract,
            ToolExecutionMode, ToolExecutionResolutionContext,
        };
        use std::collections::BTreeSet;
        use std::time::Duration;

        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("detached.owner", "v1").unwrap(),
            RestartClass::NonResumable,
            IdempotencyScope::ToolCall,
            Duration::from_secs(10),
        )
        .unwrap();
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .unwrap();
        let dispatcher = ExactExecutionDispatcher {
            catalog: Arc::from([crate::ToolCatalogEntry::session_inline(
                Arc::new(ToolDef::new(
                    "security_scan",
                    "scan",
                    json!({"type": "object"}),
                )),
                true,
            )
            .with_execution_contract(contract)]),
        };
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "detached-call",
            name: "security_scan",
            args: &args,
        };
        let context = ToolDispatchContext::default();
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );
        let plan = dispatcher
            .resolve_execution_plan(call, &context, &resolution)
            .expect("detached plan resolves");

        let error = dispatcher
            .dispatch_resolved_with_context(call, &context, &plan)
            .await
            .expect_err("the default dispatcher must not lower detached work to dispatch()");

        assert!(matches!(
            error,
            crate::ToolError::Unavailable {
                reason: crate::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn fenced_streaming_dispatch_mints_context_and_filtered_wrapper_preserves_it() {
        use crate::{
            StreamingToolExecutionPolicy, ToolDeadlineChain, ToolDeadlineContributor,
            ToolDeadlineOwner, ToolExecutionContract, ToolExecutionMode,
            ToolExecutionResolutionContext,
        };
        use std::collections::BTreeSet;
        use std::time::Duration;

        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Streaming]),
            ToolExecutionMode::Streaming,
            Some(
                StreamingToolExecutionPolicy::new(Duration::from_secs(5), Duration::from_secs(30))
                    .unwrap(),
            ),
            None,
        )
        .unwrap();
        let saw_streaming_context = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let owner = StreamingExecutionDispatcher {
            catalog: Arc::from([crate::ToolCatalogEntry::session_inline(
                Arc::new(ToolDef::new(
                    "stream_scan",
                    "stream scan",
                    json!({"type": "object"}),
                )),
                true,
            )
            .with_execution_contract(contract)]),
            saw_streaming_context: Arc::clone(&saw_streaming_context),
        };
        let dispatcher = Arc::new(FilteredToolDispatcher::new(
            Arc::new(owner),
            ["stream_scan"],
        ));
        let filtered_catalog = dispatcher.tool_catalog();
        assert_eq!(
            filtered_catalog[0].execution.default_mode(),
            ToolExecutionMode::Streaming
        );
        let filtered_policy = filtered_catalog[0]
            .execution
            .streaming_policy()
            .expect("wrapper preserves the streaming registration");
        assert_eq!(filtered_policy.inactivity_timeout(), Duration::from_secs(5));
        assert_eq!(filtered_policy.absolute_timeout(), Duration::from_secs(30));
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "stream-call",
            name: "stream_scan",
            args: &args,
        };
        let context = ToolDispatchContext::default();
        assert!(
            context.streaming().is_none(),
            "callers cannot pre-mint the supervised streaming context"
        );
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(60),
            )])
            .unwrap(),
        );
        let plan =
            crate::resolve_tool_execution_plan_fenced(&dispatcher, call, &context, &resolution)
                .expect("streaming plan resolves through wrapper");

        let outcome =
            crate::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan)
                .await
                .expect("streaming dispatch completes");

        assert_eq!(outcome.result.text_content(), "stream complete");
        assert!(
            saw_streaming_context.load(std::sync::atomic::Ordering::SeqCst),
            "the wrapper must preserve the exact supervised context"
        );
    }

    #[tokio::test]
    async fn declared_streaming_without_a_mode_owner_fails_closed_before_plain_dispatch() {
        use crate::{
            StreamingToolExecutionPolicy, ToolDeadlineChain, ToolDeadlineContributor,
            ToolDeadlineOwner, ToolExecutionContract, ToolExecutionMode,
            ToolExecutionResolutionContext,
        };
        use std::collections::BTreeSet;
        use std::time::Duration;

        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Streaming]),
            ToolExecutionMode::Streaming,
            Some(
                StreamingToolExecutionPolicy::new(Duration::from_secs(5), Duration::from_secs(30))
                    .unwrap(),
            ),
            None,
        )
        .unwrap();
        let dispatcher = Arc::new(ExactExecutionDispatcher {
            catalog: Arc::from([crate::ToolCatalogEntry::session_inline(
                Arc::new(ToolDef::new(
                    "ownerless_stream",
                    "ownerless",
                    json!({"type": "object"}),
                )),
                true,
            )
            .with_execution_contract(contract)]),
        });
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "ownerless-call",
            name: "ownerless_stream",
            args: &args,
        };
        let context = ToolDispatchContext::default();
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(60),
            )])
            .unwrap(),
        );
        let plan =
            crate::resolve_tool_execution_plan_fenced(&dispatcher, call, &context, &resolution)
                .expect("declaration resolves");

        let error = crate::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan)
            .await
            .expect_err("missing streaming owner must fail closed");
        assert!(matches!(
            error,
            crate::ToolError::Unavailable {
                reason: crate::ToolUnavailableReason::ExecutionModeOwnerUnavailable,
                ..
            }
        ));
    }

    #[test]
    fn filtered_execution_plan_resolver_rejects_policy_denied_tool() {
        use crate::{
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
            ToolExecutionResolutionContext, ToolExecutionResolutionError,
        };
        use std::time::Duration;

        let dispatcher =
            FilteredToolDispatcher::new(Arc::new(ContextAwareToolDispatcher), Vec::<String>::new());
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "call-hidden",
            name: "inspect_context",
            args: &args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );

        let error = dispatcher
            .resolve_execution_plan(call, &ToolDispatchContext::default(), &resolution)
            .expect_err("hidden tools must not resolve");

        assert_eq!(
            error,
            ToolExecutionResolutionError::AccessDenied {
                tool_name: "inspect_context".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn filtered_execution_plan_forwards_hybrid_resolution_to_visible_owner() {
        use crate::{
            DetachedToolExecutionPolicy, IdempotencyScope, ResolvedExecutionKind, RestartClass,
            RunnerIdentity, ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
            ToolExecutionContract, ToolExecutionMode, ToolExecutionResolutionContext,
        };
        use std::collections::BTreeSet;
        use std::time::Duration;

        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("filtered-hybrid-owner", "v1").unwrap(),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .unwrap();
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Fast, ToolExecutionMode::Detached]),
            ToolExecutionMode::Fast,
            None,
            Some(detached),
        )
        .unwrap();
        let dispatcher = FilteredToolDispatcher::new(
            Arc::new(HybridExecutionDispatcher {
                catalog: Arc::from([crate::ToolCatalogEntry::session_inline(
                    Arc::new(ToolDef::new(
                        "hybrid_scan",
                        "filtered-hybrid-owner catalog",
                        json!({"type": "object"}),
                    )),
                    true,
                )
                .with_execution_contract(contract)]),
            }),
            ["hybrid_scan"],
        );
        let args = serde_json::value::RawValue::from_string(r#"{"run_detached":true}"#.to_string())
            .unwrap();
        let call = ToolCallView {
            id: "call-hybrid",
            name: "hybrid_scan",
            args: &args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );

        let catalog = dispatcher.tool_catalog();
        assert_eq!(catalog[0].execution.default_mode(), ToolExecutionMode::Fast);
        assert_eq!(catalog[0].tool.description, "filtered-hybrid-owner catalog");

        let plan = dispatcher
            .resolve_execution_plan(call, &ToolDispatchContext::default(), &resolution)
            .expect("visible hybrid tool should delegate plan resolution");
        dispatcher
            .validate_resolved_execution_plan(call, &resolution, &plan)
            .expect("hybrid-selected advertised mode must validate");
        let ResolvedExecutionKind::Detached(policy) = plan.kind() else {
            panic!("hybrid resolver should select its non-default detached mode");
        };
        assert_eq!(policy.runner().name(), "filtered-hybrid-owner");

        let outcome = dispatcher
            .dispatch_resolved_with_context(call, &ToolDispatchContext::default(), &plan)
            .await
            .expect("visible hybrid tool should preserve resolved dispatch");
        let payload: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).unwrap();
        assert_eq!(payload["owner"], "filtered-hybrid-owner");
    }

    #[test]
    fn root_validation_rejects_plan_outside_live_advertised_contract() {
        use crate::{
            DetachedToolExecutionPolicy, IdempotencyScope, RestartClass, RunnerIdentity,
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner, ToolExecutionContract,
            ToolExecutionContractError, ToolExecutionMode, ToolExecutionResolutionContext,
            ToolExecutionResolutionError,
        };
        use std::collections::BTreeSet;
        use std::time::Duration;

        let dispatcher = ExactExecutionDispatcher {
            catalog: Arc::from([crate::ToolCatalogEntry::session_inline(
                Arc::new(ToolDef::new(
                    "fast_only",
                    "fast only",
                    json!({"type": "object"}),
                )),
                true,
            )]),
        };
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("dishonest.owner", "v1").unwrap(),
            RestartClass::NonResumable,
            IdempotencyScope::ToolCall,
            Duration::from_secs(10),
        )
        .unwrap();
        let dishonest_contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .unwrap();
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );
        let plan = dishonest_contract
            .resolve_default(resolution.deadlines().clone())
            .unwrap();
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "dishonest-plan",
            name: "fast_only",
            args: &args,
        };

        assert_eq!(
            dispatcher.validate_resolved_execution_plan(call, &resolution, &plan),
            Err(ToolExecutionResolutionError::Contract(
                ToolExecutionContractError::RequestedModeUnsupported {
                    requested_mode: ToolExecutionMode::Detached,
                }
            ))
        );
    }

    #[tokio::test]
    async fn universal_root_fence_accepts_rebuilt_equivalent_catalog_arcs() {
        use crate::{
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
            ToolExecutionResolutionContext,
        };
        use std::time::Duration;

        let dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(IdenticalMutationDispatcher {
            tool: ToolDef::new("rebuilt", "rebuilt", json!({"type": "object"})),
            epoch: std::sync::atomic::AtomicU64::new(0),
            mutate_on_resolve: false,
        });
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "rebuilt-arcs",
            name: "rebuilt",
            args: &args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );

        let plan = crate::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &ToolDispatchContext::default(),
            &resolution,
        )
        .expect("equivalent rebuilt catalog projections resolve");
        crate::dispatch_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &ToolDispatchContext::default(),
            &plan,
        )
        .await
        .expect("equivalent rebuilt catalog projections dispatch");
    }

    #[tokio::test]
    async fn app_action_target_owner_is_fenced_across_resolution_and_dispatch() {
        use crate::{
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
            ToolExecutionResolutionContext, ToolUnavailableReason,
        };

        let concrete = Arc::new(IdenticalMutationDispatcher {
            tool: ToolDef::new("action", "action", json!({"type": "object"})),
            epoch: std::sync::atomic::AtomicU64::new(0),
            mutate_on_resolve: false,
        });
        let dispatcher: Arc<dyn AgentToolDispatcher> = concrete.clone();
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "app-action",
            name: "action",
            args: &args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::unbounded(
                ToolDeadlineOwner::DirectCaller,
            )])
            .unwrap(),
        );
        let selected = crate::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &ToolDispatchContext::default(),
            &resolution,
        )
        .unwrap();
        let witness = selected.owner_witness("root-dispatcher").unwrap().clone();
        let binding = crate::tool_application::ToolApplicationBinding::new(
            "test.app",
            json!({"opaque": true}),
        )
        .with_owner_witness(witness.clone())
        .unwrap();
        assert_eq!(binding.payload, json!({"opaque": true}));
        assert!(binding.clone().with_owner_witness(witness).is_err());
        let context = ToolDispatchContext::default().with_application_binding(binding.clone());

        let unchanged =
            crate::resolve_tool_execution_plan_fenced(&dispatcher, call, &context, &resolution)
                .expect("the selected owner still receives the action");
        crate::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &unchanged)
            .await
            .unwrap();

        // Publication changes after app resolution but before the ordinary
        // execution plan is selected. The new plan is independently valid,
        // but may never replace the app's original selected target.
        concrete
            .epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            crate::resolve_tool_execution_plan_fenced(&dispatcher, call, &context, &resolution),
            Err(crate::ToolExecutionResolutionError::Unavailable {
                reason: ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            })
        ));
        let changed = crate::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &ToolDispatchContext::default(),
            &resolution,
        )
        .unwrap();
        assert!(matches!(
            crate::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &changed).await,
            Err(crate::ToolError::Unavailable {
                reason: ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            })
        ));

        let missing_owner = crate::ToolExecutionOwnerWitness::new(
            "nested-owner",
            "original-leaf",
            dispatcher.execution_binding_fingerprint(call.name).unwrap(),
        )
        .unwrap();
        let nested = binding.with_owner_witness(missing_owner).unwrap();
        assert_eq!(nested.owner_witnesses().len(), 2);
        assert!(
            nested
                .validate_execution_plan(call.name, &selected)
                .is_err()
        );
    }

    #[tokio::test]
    async fn universal_root_fence_binds_canonical_call_identity() {
        use crate::{
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
            ToolExecutionResolutionContext, ToolUnavailableReason,
        };
        use std::time::Duration;

        let dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(IdenticalMutationDispatcher {
            tool: ToolDef::new("bound", "bound", json!({"type": "object"})),
            epoch: std::sync::atomic::AtomicU64::new(0),
            mutate_on_resolve: false,
        });
        let resolved_args =
            serde_json::value::RawValue::from_string(r#"{"a":1,"b":2}"#.to_string()).unwrap();
        let equivalent_args =
            serde_json::value::RawValue::from_string(r#"{ "b": 2, "a": 1 }"#.to_string()).unwrap();
        let changed_args =
            serde_json::value::RawValue::from_string(r#"{"a":1,"b":3}"#.to_string()).unwrap();
        let resolved_call = ToolCallView {
            id: "bound-call",
            name: "bound",
            args: &resolved_args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );
        let plan = crate::resolve_tool_execution_plan_fenced(
            &dispatcher,
            resolved_call,
            &ToolDispatchContext::default(),
            &resolution,
        )
        .unwrap();

        crate::dispatch_tool_execution_plan_fenced(
            &dispatcher,
            ToolCallView {
                args: &equivalent_args,
                ..resolved_call
            },
            &ToolDispatchContext::default(),
            &plan,
        )
        .await
        .expect("canonical JSON-equivalent arguments preserve call identity");

        let error = crate::dispatch_tool_execution_plan_fenced(
            &dispatcher,
            ToolCallView {
                args: &changed_args,
                ..resolved_call
            },
            &ToolDispatchContext::default(),
            &plan,
        )
        .await
        .expect_err("different arguments must not dispatch under the old plan");
        assert!(matches!(
            error,
            crate::ToolError::Unavailable {
                reason: ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn universal_root_fence_rejects_fresh_dispatcher_reconstruction() {
        use crate::{
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
            ToolExecutionResolutionContext, ToolUnavailableReason,
        };
        use std::time::Duration;

        let make_dispatcher = || -> Arc<dyn AgentToolDispatcher> {
            Arc::new(IdenticalMutationDispatcher {
                tool: ToolDef::new("bound", "bound", json!({"type": "object"})),
                epoch: std::sync::atomic::AtomicU64::new(0),
                mutate_on_resolve: false,
            })
        };
        let original = make_dispatcher();
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "reconstructed",
            name: "bound",
            args: &args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );
        let plan = crate::resolve_tool_execution_plan_fenced(
            &original,
            call,
            &ToolDispatchContext::default(),
            &resolution,
        )
        .unwrap();
        let reconstructed = make_dispatcher();

        let error = crate::dispatch_tool_execution_plan_fenced(
            &reconstructed,
            call,
            &ToolDispatchContext::default(),
            &plan,
        )
        .await
        .expect_err("fresh reconstruction must never reproduce ephemeral root authority");
        assert!(matches!(
            error,
            crate::ToolError::Unavailable {
                reason: ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            }
        ));
    }

    #[test]
    fn universal_root_fence_rejects_direct_identical_metadata_replacement() {
        use crate::{
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
            ToolExecutionResolutionContext, ToolExecutionResolutionError, ToolUnavailableReason,
        };
        use std::time::Duration;

        let dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(IdenticalMutationDispatcher {
            tool: ToolDef::new("moving", "identical metadata", json!({"type": "object"})),
            epoch: std::sync::atomic::AtomicU64::new(0),
            mutate_on_resolve: true,
        });
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "direct-identical-replacement",
            name: "moving",
            args: &args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );

        assert!(matches!(
            crate::resolve_tool_execution_plan_fenced(
                &dispatcher,
                call,
                &ToolDispatchContext::default(),
                &resolution,
            ),
            Err(ToolExecutionResolutionError::Unavailable {
                reason: ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            })
        ));
    }

    #[test]
    fn filtered_wrapper_composes_inner_live_binding_epoch() {
        use crate::{
            ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
            ToolExecutionResolutionContext, ToolExecutionResolutionError, ToolUnavailableReason,
        };
        use std::time::Duration;

        let dispatcher: Arc<dyn AgentToolDispatcher> = Arc::new(FilteredToolDispatcher::new(
            Arc::new(IdenticalMutationDispatcher {
                tool: ToolDef::new("moving", "identical metadata", json!({"type": "object"})),
                epoch: std::sync::atomic::AtomicU64::new(0),
                mutate_on_resolve: true,
            }),
            ["moving"],
        ));
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "filtered-identical-replacement",
            name: "moving",
            args: &args,
        };
        let resolution = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_secs(600),
            )])
            .unwrap(),
        );

        assert!(matches!(
            crate::resolve_tool_execution_plan_fenced(
                &dispatcher,
                call,
                &ToolDispatchContext::default(),
                &resolution,
            ),
            Err(ToolExecutionResolutionError::Unavailable {
                reason: ToolUnavailableReason::ExecutionOwnerChanged,
                ..
            })
        ));
    }

    #[test]
    fn test_inline_peer_notification_policy_from_raw() {
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(None),
            Ok(InlinePeerNotificationPolicy::AtMost(
                DEFAULT_MAX_INLINE_PEER_NOTIFICATIONS
            ))
        );
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(Some(-1)),
            Ok(InlinePeerNotificationPolicy::Always)
        );
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(Some(0)),
            Ok(InlinePeerNotificationPolicy::Never)
        );
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(Some(25)),
            Ok(InlinePeerNotificationPolicy::AtMost(25))
        );
        assert_eq!(
            InlinePeerNotificationPolicy::try_from_raw(Some(-42)),
            Err(-42)
        );
    }

    /// UNIT-002: DetachedOpCompletion serializes without operation_id.
    /// The app-facing control noun is job_id (CONTRACT-003).
    #[test]
    fn unit_002_detached_op_completion_has_no_operation_id() {
        use crate::agent::DetachedOpCompletion;
        use crate::ops_lifecycle::{OperationKind, OperationStatus};

        let completion = DetachedOpCompletion {
            job_id: "j_test".into(),
            kind: OperationKind::BackgroundToolOp,
            status: OperationStatus::Completed,
            terminal_outcome: None,
            display_name: "test cmd".into(),
            detail: "ok".into(),
            elapsed_ms: None,
        };
        #[allow(clippy::unwrap_used)]
        let json = serde_json::to_value(&completion).unwrap();
        assert!(
            json.get("operation_id").is_none(),
            "operation_id must not appear in serialized DetachedOpCompletion (CONTRACT-003)"
        );
        assert!(
            json.get("job_id").is_some(),
            "job_id must be the app-facing control noun"
        );
    }

    #[test]
    fn live_bridge_noncommitting_run_permit_is_exact_and_one_use_across_clones() {
        let session_id = crate::SessionId::new();
        let revision = crate::CanonicalContextRevision::from_transcript_revision(
            "test-canonical-revision".to_string(),
        );
        let permit = crate::LiveBridgeNoncommittingRunPermit::__test_new(
            "operation-1",
            session_id.clone(),
            revision.clone(),
        );
        let clone = permit.clone();

        assert!(
            permit
                .consume("operation-2", &session_id, &revision)
                .is_err(),
            "mismatched operation must fail without consuming the permit"
        );
        permit
            .consume("operation-1", &session_id, &revision)
            .expect("exact binding consumes permit once");
        assert!(
            clone
                .consume("operation-1", &session_id, &revision)
                .is_err(),
            "a cloned permit must share the one-use consumption bit"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod governed_operation_tests {
    use super::*;
    use crate::authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
        OperationObservation, OperationObservationError, OperationRefusalKind, OperationRefused,
        PreparedAuthorizationBinding, PreparedOperationAuthorization, PreparedOperationCheck,
        ToolAuthorizationFacts, ToolAuthorizationTarget, WorkAuthorization,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Work(Arc<AtomicBool>);
    struct Prepared(Arc<AtomicBool>);
    impl WorkAuthorization for Work {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            if self.0.load(Ordering::SeqCst) {
                return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
            }
            Ok(Arc::new(Prepared(Arc::clone(&self.0))))
        }
    }
    impl PreparedOperationAuthorization for Prepared {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            crate::authorization::OperationReviewTier::R1
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            if self.0.load(Ordering::SeqCst) {
                return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
            }
            Ok(())
        }

        fn observe(
            &self,
            _binding: &PreparedAuthorizationBinding,
            _event: OperationObservation,
        ) -> Result<(), OperationObservationError> {
            Ok(())
        }
    }

    /// The dispatch's own operation is reachable only with its actual
    /// borrowed call, and stays checked against current owners.
    #[test]
    fn a_governed_operation_requires_the_dispatch_call_and_stays_current() {
        let revoked = Arc::new(AtomicBool::new(false));
        let work = crate::WorkAuthorizationContext::new(
            Arc::new(Work(Arc::clone(&revoked))),
            crate::OperationExecutionScope::Domain,
        );
        let plan = crate::ToolExecutionContract::default()
            .resolve_default(
                crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                    crate::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(1),
                )])
                .unwrap(),
            )
            .unwrap();
        let arguments: Arc<serde_json::value::RawValue> =
            Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: crate::OperationId::new(),
            execution_scope: crate::OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
                call_id: Arc::from("setup-call"),
                name: "setup".into(),
                arguments,
                target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
            }),
        });
        let prepared = PreparedOperationCheck::prepare(work.clone(), binding).unwrap();
        let context = ToolDispatchContext::default()
            .with_work_authorization(Some(work.clone()))
            .with_prepared_authorization(prepared.clone());
        let (call, _) = prepared.tool_dispatch_parts().unwrap();

        assert!(
            ToolDispatchContext::default()
                .governed_operation(call)
                .is_err(),
            "a default context has no governed operation"
        );
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let lookalike = ToolCallView {
            id: "setup-call",
            name: "setup",
            args: &args,
        };
        assert!(
            context.governed_operation(lookalike).is_err(),
            "an equal-looking call is not the dispatch's own call"
        );
        let operation = context
            .governed_operation(call)
            .expect("the dispatch's own call");
        assert!(operation.work_authorization().same_context(&work));
        let current = operation.current().expect("current");
        assert!(current.binding().same_operation(prepared.binding()));
        let mut ran = false;
        assert_eq!(
            operation.run_if_current_within(&(), &mut || ran = true),
            Err(crate::ToolError::OperationAuthorizationUnavailable),
            "a default (foreign) work owner refuses as unavailable"
        );
        assert!(!ran, "and the operation never runs");
        assert_eq!(
            operation
                .issue_child_if_current_within(&(), &())
                .map(|_| ()),
            Err(crate::ToolError::OperationAuthorizationUnavailable),
            "a default (foreign) work owner issues no child"
        );

        revoked.store(true, Ordering::SeqCst);
        assert!(
            operation.current().is_err(),
            "the owned operation is checked against current owners"
        );
    }

    /// An R2 work owner whose child issuance accepts only a `u32` intent.
    struct R2Prepared;
    impl PreparedOperationAuthorization for R2Prepared {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            crate::authorization::OperationReviewTier::R2
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            Ok(())
        }

        fn observe(
            &self,
            _binding: &PreparedAuthorizationBinding,
            _event: OperationObservation,
        ) -> Result<(), OperationObservationError> {
            Ok(())
        }
    }
    struct R2Work;
    impl WorkAuthorization for R2Work {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(R2Prepared))
        }

        fn validate_child_issuance(
            &self,
            _further: &(dyn std::any::Any + Send + Sync),
            intent: &(dyn std::any::Any + Send + Sync),
        ) -> Result<(), OperationAuthorizationError> {
            intent
                .downcast_ref::<u32>()
                .map(|_| ())
                .ok_or_else(|| OperationRefused::new(OperationRefusalKind::Denied).into())
        }

        fn issue_child_if_current_within(
            &self,
            _binding: &PreparedAuthorizationBinding,
            _prepared: Option<&crate::authorization::OperationValidity>,
            _further: &(dyn std::any::Any + Send + Sync),
            intent: &(dyn std::any::Any + Send + Sync),
            entered_tier: crate::authorization::OperationReviewTier,
        ) -> Result<Box<dyn std::any::Any + Send>, OperationAuthorizationError> {
            assert_eq!(entered_tier, crate::authorization::OperationReviewTier::R2);
            Ok(Box::new(*intent.downcast_ref::<u32>().expect("validated")))
        }

        /// The fixture work names no action account and no mandate.
        fn review_operation_context(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<
            crate::authorization::ReviewOperationContextResolution,
            OperationAuthorizationError,
        > {
            Ok(crate::authorization::ReviewOperationContextResolution::NotApplicable)
        }
    }
    struct AllowAll;
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl crate::approval::review::OperationReviewer for AllowAll {
        async fn review(
            &self,
            _candidate: &crate::approval::review::ReviewCandidate<'_>,
        ) -> Result<crate::approval::review::ReviewVerdict, crate::approval::review::ReviewerFailure>
        {
            Ok(crate::approval::review::ReviewVerdict::Allow)
        }
    }

    /// A malformed child intent is refused before the dispatch's admitted R2
    /// review is spent: the corrected intent then enters once, and a second
    /// entry finds the review consumed.
    #[tokio::test]
    async fn a_malformed_child_intent_leaves_the_admitted_review_spendable() {
        let work = crate::WorkAuthorizationContext::new(
            Arc::new(R2Work),
            crate::OperationExecutionScope::Domain,
        );
        let plan = crate::ToolExecutionContract::default()
            .resolve_default(
                crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                    crate::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(1),
                )])
                .unwrap(),
            )
            .unwrap();
        let arguments: Arc<serde_json::value::RawValue> =
            Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: crate::OperationId::new(),
            execution_scope: crate::OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
                call_id: Arc::from("setup-call"),
                name: "setup".into(),
                arguments,
                target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
            }),
        });
        let prepared = PreparedOperationCheck::prepare(work.clone(), binding).unwrap();
        let (call, _) = prepared.tool_dispatch_parts().unwrap();
        let review = Arc::new(crate::approval::review::BoundOperationReview::new(
            Arc::new(AllowAll),
            crate::approval::ApprovalService::new(),
            std::time::Duration::from_secs(30),
        ));
        let entry = crate::approval::review::admit_operation_review(
            Some(&review),
            crate::approval::review::ReviewEntrySupport::ConsumesAtEntry,
            call,
            &prepared,
            &work,
        )
        .await
        .expect("the R2 review is admitted");
        let context = ToolDispatchContext::default()
            .with_work_authorization(Some(work))
            .with_prepared_authorization(prepared.clone())
            .with_review_entry(entry);
        let operation = context.governed_operation(call).expect("governed");

        assert!(
            matches!(
                operation.issue_child_if_current_within(&(), &"not an intent"),
                Err(crate::ToolError::AuthorizationRefused { .. })
            ),
            "a malformed intent is refused"
        );
        let issued = operation
            .issue_child_if_current_within(&(), &7u32)
            .expect("the review was not spent by the malformed call");
        assert_eq!(*issued.downcast::<u32>().expect("typed"), 7);
        assert!(
            matches!(
                operation.issue_child_if_current_within(&(), &7u32),
                Err(crate::ToolError::ReviewUnsatisfied {
                    kind: crate::approval::review::ReviewUnsatisfiedKind::AlreadyConsumed,
                    ..
                })
            ),
            "the admitted review is spent once"
        );
    }

    fn governed_work() -> crate::WorkAuthorizationContext {
        crate::WorkAuthorizationContext::new(
            Arc::new(Work(Arc::new(AtomicBool::new(false)))),
            crate::OperationExecutionScope::Domain,
        )
    }

    /// Declares a comms, a memory and a builtin tool, resolves each with the
    /// default target-less plan, and counts the calls it runs.
    #[derive(Default)]
    struct SourceKindDispatcher(std::sync::atomic::AtomicUsize);

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for SourceKindDispatcher {
        fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
            [
                ("send_message", crate::types::ToolSourceKind::Comms),
                ("memory_search", crate::types::ToolSourceKind::Memory),
                ("read_file", crate::types::ToolSourceKind::Builtin),
            ]
            .map(|(name, kind)| {
                Arc::new(crate::types::ToolDef {
                    audience: Default::default(),
                    name: name.into(),
                    description: String::new(),
                    input_schema: serde_json::json!({"type": "object"}),
                    provenance: Some(crate::types::ToolProvenance {
                        kind,
                        source_id: "fixture".into(),
                        artifact: None,
                    }),
                })
            })
            .into()
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(crate::types::ToolResult::new(call.id.to_string(), "ran".to_string(), false).into())
        }
    }

    async fn fenced<T: AgentToolDispatcher + ?Sized + 'static>(
        dispatcher: &Arc<T>,
        name: &str,
        context: &ToolDispatchContext,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "call-1",
            name,
            args: &args,
        };
        let resolution = crate::ToolExecutionResolutionContext::new(
            crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                crate::ToolDeadlineOwner::CoreToolDispatch,
                std::time::Duration::from_secs(60),
            )])
            .unwrap(),
        );
        let plan =
            crate::resolve_tool_execution_plan_fenced(dispatcher, call, context, &resolution)
                .unwrap();
        crate::dispatch_tool_execution_plan_fenced(dispatcher, call, context, &plan).await
    }

    fn is_malformed<T>(result: &Result<T, crate::ToolError>) -> bool {
        matches!(result, Err(crate::ToolError::AuthorizationRefused { refusal })
            if refusal.kind() == OperationRefusalKind::MalformedFacts)
    }

    /// A governed comms or memory plan must carry its owner's typed target.
    /// One resolved without it, by a wrapper that did not forward the
    /// owner's plan resolution and may drop the dispatch context before the
    /// owner can check it, is refused at the fenced dispatch before any body
    /// runs. The same plans dispatch ungoverned, and a builtin keeps its
    /// target-less plan under governed work.
    #[tokio::test]
    async fn a_governed_comms_or_memory_plan_without_an_owner_target_is_refused() {
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let governed =
            ToolDispatchContext::default().with_work_authorization(Some(governed_work()));
        for name in ["send_message", "memory_search"] {
            let refused = fenced(&dispatcher, name, &governed).await;
            assert!(is_malformed(&refused), "{name}: {refused:?}");
        }
        assert_eq!(dispatcher.0.load(Ordering::SeqCst), 0, "no body ran");

        let ungoverned = ToolDispatchContext::default();
        for name in ["send_message", "memory_search"] {
            let outcome = fenced(&dispatcher, name, &ungoverned).await.expect(name);
            assert!(!outcome.result.is_error, "{name}");
        }
        let outcome = fenced(&dispatcher, "read_file", &governed)
            .await
            .expect("a builtin keeps its target-less plan");
        assert!(!outcome.result.is_error);
        assert_eq!(dispatcher.0.load(Ordering::SeqCst), 3);
    }

    /// A registered bundle attaches its own operation target to a plan that
    /// names none, which does not stand in for a comms or memory owner's
    /// target: a bundle whose dispatcher wraps such a tool without
    /// forwarding its plan resolution (and may drop the context before it)
    /// is refused at the fenced dispatch, while the bundle's builtin
    /// dispatches under the bundle's target.
    #[tokio::test]
    async fn a_bundle_target_does_not_stand_in_for_a_comms_or_memory_owner_target() {
        let inner = Arc::new(SourceKindDispatcher::default());
        let bundle = Arc::new(crate::RegisteredToolBundleDispatcher::new(
            "host-bundle",
            Arc::clone(&inner) as Arc<dyn AgentToolDispatcher>,
        ));
        let governed =
            ToolDispatchContext::default().with_work_authorization(Some(governed_work()));
        for name in ["send_message", "memory_search"] {
            let refused = fenced(&bundle, name, &governed).await;
            assert!(is_malformed(&refused), "{name}: {refused:?}");
        }
        assert_eq!(inner.0.load(Ordering::SeqCst), 0, "no body ran");

        let outcome = fenced(&bundle, "read_file", &governed)
            .await
            .expect("a builtin under the bundle's own target");
        assert!(!outcome.result.is_error);
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);
    }

    /// A leaf's target requirement reads the context as the authorization
    /// check does: neither work nor a prepared check is ungoverned (and no
    /// target is computed); work without its prepared check, or a prepared
    /// check without work, is malformed, never ungoverned.
    #[test]
    fn a_target_requirement_treats_a_mixed_context_as_malformed() {
        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "call-1",
            name: "peers",
            args: &args,
        };
        let computed = std::cell::Cell::new(0_u32);
        let target = || {
            computed.set(computed.get() + 1);
            Ok(crate::ToolOperationTarget::PeerDirectoryRead)
        };

        assert_eq!(
            ToolDispatchContext::default().require_authorized_operation_target(call, target),
            Ok(())
        );
        let work_only =
            ToolDispatchContext::default().with_work_authorization(Some(governed_work()));
        let refused = work_only.require_authorized_operation_target(call, target);
        assert!(
            is_malformed(&refused),
            "work without its check: {refused:?}"
        );

        let plan = crate::ToolExecutionContract::default()
            .resolve_default(
                crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                    crate::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(1),
                )])
                .unwrap(),
            )
            .unwrap();
        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: crate::OperationId::new(),
            execution_scope: crate::OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
                call_id: Arc::from("call-1"),
                name: "peers".into(),
                arguments: Arc::from(
                    serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                ),
                target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
            }),
        });
        let prepared = PreparedOperationCheck::prepare(governed_work(), binding).unwrap();
        let check_only = ToolDispatchContext::default().with_prepared_authorization(prepared);
        let refused = check_only.require_authorized_operation_target(call, target);
        assert!(is_malformed(&refused), "a check without work: {refused:?}");
        assert_eq!(
            computed.get(),
            0,
            "a target is computed only for governed work"
        );
    }

    /// A work owner whose publication changes as the test scripts it. While
    /// a change is in progress every preparation refuses to rebuild, and a
    /// decision whose publication moved on is stale. The first
    /// `refuse_first` preparations begin a change and refuse; the first
    /// decision prepared otherwise begins one after its `begin_after_checks`
    /// current checks. Every change completes after `settle`.
    struct ChurnWork {
        sequence: Arc<std::sync::atomic::AtomicU64>,
        tier: crate::authorization::OperationReviewTier,
        settle: std::time::Duration,
        refuse_first: std::sync::atomic::AtomicUsize,
        begin_after_checks: std::sync::Mutex<Option<usize>>,
        prepares: std::sync::atomic::AtomicUsize,
    }

    fn begin_change(sequence: &Arc<std::sync::atomic::AtomicU64>, settle: std::time::Duration) {
        let odd = sequence.load(Ordering::SeqCst) + 1;
        sequence.store(odd, Ordering::SeqCst);
        let sequence = Arc::clone(sequence);
        tokio::spawn(async move {
            tokio::time::sleep(settle).await;
            sequence.store(odd + 1, Ordering::SeqCst);
        });
    }

    impl ChurnWork {
        fn new(
            tier: crate::authorization::OperationReviewTier,
            refuse_first: usize,
            begin_after_checks: Option<usize>,
        ) -> Arc<Self> {
            Self::settling_after(
                tier,
                refuse_first,
                begin_after_checks,
                std::time::Duration::from_millis(30),
            )
        }

        /// A work whose changes stay in progress for an hour, past every
        /// owner-change wait bound.
        fn held(
            tier: crate::authorization::OperationReviewTier,
            begin_after_checks: Option<usize>,
        ) -> Arc<Self> {
            Self::settling_after(
                tier,
                0,
                begin_after_checks,
                std::time::Duration::from_secs(3600),
            )
        }

        fn settling_after(
            tier: crate::authorization::OperationReviewTier,
            refuse_first: usize,
            begin_after_checks: Option<usize>,
            settle: std::time::Duration,
        ) -> Arc<Self> {
            Arc::new(Self {
                sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                tier,
                settle,
                refuse_first: std::sync::atomic::AtomicUsize::new(refuse_first),
                begin_after_checks: std::sync::Mutex::new(begin_after_checks),
                prepares: std::sync::atomic::AtomicUsize::new(0),
            })
        }
    }

    struct ChurnPrepared {
        sequence: Arc<std::sync::atomic::AtomicU64>,
        at: u64,
        tier: crate::authorization::OperationReviewTier,
        settle: std::time::Duration,
        checks: std::sync::atomic::AtomicUsize,
        begin_after_checks: Option<usize>,
    }

    impl WorkAuthorization for ChurnWork {
        fn owner_change_in_progress(&self) -> Option<crate::authorization::OwnerChangeInProgress> {
            let sequence = self.sequence.load(Ordering::SeqCst);
            (sequence % 2 == 1).then(|| {
                crate::authorization::OwnerChangeInProgress::new(uuid::Uuid::nil(), sequence)
            })
        }

        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            self.prepares.fetch_add(1, Ordering::SeqCst);
            let at = self.sequence.load(Ordering::SeqCst);
            if at % 2 == 1 {
                return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
            }
            if self
                .refuse_first
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
            {
                begin_change(&self.sequence, self.settle);
                return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
            }
            Ok(Arc::new(ChurnPrepared {
                sequence: Arc::clone(&self.sequence),
                at,
                tier: self.tier,
                settle: self.settle,
                checks: std::sync::atomic::AtomicUsize::new(0),
                begin_after_checks: self.begin_after_checks.lock().unwrap().take(),
            }))
        }

        fn review_operation_context(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<
            crate::authorization::ReviewOperationContextResolution,
            OperationAuthorizationError,
        > {
            Ok(crate::authorization::ReviewOperationContextResolution::NotApplicable)
        }
    }

    impl PreparedOperationAuthorization for ChurnPrepared {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            self.tier
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            let checks = self.checks.fetch_add(1, Ordering::SeqCst) + 1;
            if self.sequence.load(Ordering::SeqCst) != self.at {
                return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
            }
            if self.begin_after_checks == Some(checks) {
                begin_change(&self.sequence, self.settle);
            }
            Ok(())
        }

        fn observe(
            &self,
            _binding: &PreparedAuthorizationBinding,
            _event: OperationObservation,
        ) -> Result<(), OperationObservationError> {
            Ok(())
        }
    }

    fn churn_context(work: &Arc<ChurnWork>) -> ToolDispatchContext {
        ToolDispatchContext::default().with_work_authorization(Some(
            crate::WorkAuthorizationContext::new(
                Arc::clone(work) as Arc<dyn WorkAuthorization>,
                crate::OperationExecutionScope::Domain,
            ),
        ))
    }

    /// The common prepared entry refused while an owner change is in
    /// progress waits for it and prepares the same operation again: the body
    /// runs once.
    #[tokio::test(start_paused = true)]
    async fn a_tool_entry_waits_out_an_owner_change_in_progress() {
        let work = ChurnWork::new(crate::authorization::OperationReviewTier::R1, 1, None);
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let started = tokio::time::Instant::now();
        let outcome = fenced(&dispatcher, "read_file", &churn_context(&work))
            .await
            .expect("the entry is prepared once the change completes");
        assert!(!outcome.result.is_error);
        assert_eq!(dispatcher.0.load(Ordering::SeqCst), 1, "the body ran once");
        assert_eq!(work.prepares.load(Ordering::SeqCst), 2);
        assert!(started.elapsed() >= std::time::Duration::from_millis(30));
    }

    /// The body entry's check, refused because a change began after the
    /// prepared entry, waits for it and checks again from current owners:
    /// the body runs once.
    #[tokio::test(start_paused = true)]
    async fn a_tool_body_entry_waits_out_an_owner_change_in_progress() {
        let work = ChurnWork::new(crate::authorization::OperationReviewTier::R1, 0, Some(1));
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let outcome = fenced(&dispatcher, "read_file", &churn_context(&work))
            .await
            .expect("the body entry is checked again once the change completes");
        assert!(!outcome.result.is_error);
        assert_eq!(dispatcher.0.load(Ordering::SeqCst), 1, "the body ran once");
        assert!(
            work.prepares.load(Ordering::SeqCst) >= 2,
            "the stale decision was prepared again"
        );
    }

    /// Forwards a dispatch to its inner dispatcher through the
    /// context-carrying entry point, without the resolved plan, as an adapter
    /// on the dispatch chain does.
    struct Lowering(Arc<dyn AgentToolDispatcher>);

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for Lowering {
        fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
            self.0.tools()
        }

        fn review_entry_support(
            &self,
            tool_name: &str,
        ) -> crate::approval::review::ReviewEntrySupport {
            self.0.review_entry_support(tool_name)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            self.0.dispatch(call).await
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &ToolDispatchContext,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            self.0.dispatch_with_context(call, context).await
        }
    }

    /// `inner` behind the call-level execution policy gate, whose allowing
    /// consequence policy begins one owner change on `work`'s publication
    /// each time it evaluates, completed by another thread 30 ms later. The
    /// gate is reached through its resolved entry point, or through an
    /// adapter that lowers the call ([`Lowering`]).
    fn gate_beginning_an_owner_change<T: AgentToolDispatcher + 'static>(
        work: &ChurnWork,
        inner: Arc<T>,
        resolved: bool,
    ) -> Arc<dyn AgentToolDispatcher> {
        let sequence = Arc::clone(&work.sequence);
        let policy =
            crate::tool_execution_policy::tests::allowing_consequence_policy_running(move || {
                // One owner change, completed by another thread 30 ms later.
                let odd = sequence.fetch_add(1, Ordering::SeqCst) + 1;
                let sequence = Arc::clone(&sequence);
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    sequence.store(odd + 1, Ordering::SeqCst);
                });
            });
        let gated: Arc<dyn AgentToolDispatcher> = Arc::new(
            crate::ExecutionPolicyGatedDispatcher::new(
                inner,
                crate::ToolExecutionPolicy::unrestricted(),
            )
            .with_consequence_policy(policy),
        );
        if resolved {
            gated
        } else {
            Arc::new(Lowering(gated))
        }
    }

    /// The call-level execution policy gate stages the call again after its
    /// own awaits. An owner change its consequence policy began meanwhile,
    /// held 30 ms, is waited out within the dispatch attempt's own bounds,
    /// as at the root staging, on both of the gate's staging paths (resolved
    /// and lowered): the call enters once, under a decision prepared again
    /// after the change.
    #[tokio::test]
    async fn a_policy_gated_entry_waits_out_an_owner_change_its_consequence_policy_began() {
        for resolved in [true, false] {
            let work = ChurnWork::new(crate::authorization::OperationReviewTier::R1, 0, None);
            let inner = Arc::new(SourceKindDispatcher::default());
            let root = gate_beginning_an_owner_change(&work, Arc::clone(&inner), resolved);
            let started = std::time::Instant::now();
            let result = fenced(&root, "read_file", &churn_context(&work)).await;
            assert!(
                result
                    .as_ref()
                    .is_ok_and(|outcome| !outcome.result.is_error),
                "resolved={resolved}: the gate waits out the change, then enters: {result:?}"
            );
            assert_eq!(
                inner.0.load(Ordering::SeqCst),
                1,
                "resolved={resolved}: the body ran once"
            );
            assert!(started.elapsed() >= std::time::Duration::from_millis(30));
            assert_eq!(
                work.sequence.load(Ordering::SeqCst),
                2,
                "one change, completed"
            );
        }
    }

    /// A model reviewer that allows every candidate and counts its reviews.
    struct CountingAllow(std::sync::atomic::AtomicUsize);

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl crate::approval::review::OperationReviewer for CountingAllow {
        async fn review(
            &self,
            _candidate: &crate::approval::review::ReviewCandidate<'_>,
        ) -> Result<crate::approval::review::ReviewVerdict, crate::approval::review::ReviewerFailure>
        {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(crate::approval::review::ReviewVerdict::Allow)
        }
    }

    /// The source-kind tools, entered at their reviewed leaf.
    struct Reviewed(SourceKindDispatcher);

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for Reviewed {
        fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
            self.0.tools()
        }

        fn review_entry_support(&self, _name: &str) -> crate::approval::review::ReviewEntrySupport {
            crate::approval::review::ReviewEntrySupport::ConsumesAtEntry
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            self.0.dispatch(call).await
        }
    }

    /// The reviewed (R2) case of the gate's wait: the gate waits out the
    /// change its consequence policy began, and the retained review's check
    /// point then finds the decision prepared after the change no longer the
    /// reviewed one. The gate's own code already ran, so on both of its
    /// staging paths that settles as `review_unsatisfied` (`context_changed`):
    /// the call is never reviewed again and no body runs.
    #[tokio::test]
    async fn a_policy_gated_reviewed_entry_settles_a_decision_its_wait_saw_change() {
        for resolved in [true, false] {
            let work = ChurnWork::new(crate::authorization::OperationReviewTier::R2, 0, None);
            let reviewer = Arc::new(CountingAllow(std::sync::atomic::AtomicUsize::new(0)));
            let context = churn_context(&work).with_operation_review(Some(Arc::new(
                crate::approval::review::BoundOperationReview::new(
                    Arc::clone(&reviewer) as Arc<dyn crate::approval::review::OperationReviewer>,
                    crate::approval::ApprovalService::new(),
                    std::time::Duration::from_secs(30),
                ),
            )));
            let inner = Arc::new(Reviewed(SourceKindDispatcher::default()));
            let root = gate_beginning_an_owner_change(&work, Arc::clone(&inner), resolved);
            let started = std::time::Instant::now();
            let result = fenced(&root, "read_file", &context).await;
            assert!(
                matches!(
                    &result,
                    Err(crate::ToolError::ReviewUnsatisfied {
                        kind: crate::approval::review::ReviewUnsatisfiedKind::ContextChanged,
                        ..
                    })
                ),
                "resolved={resolved}: the gate waits out the change, then settles the changed decision: {result:?}"
            );
            assert_eq!(
                inner.0.0.load(Ordering::SeqCst),
                0,
                "resolved={resolved}: no body ran"
            );
            assert_eq!(
                reviewer.0.load(Ordering::SeqCst),
                1,
                "resolved={resolved}: reviewed once, never again"
            );
            assert!(
                started.elapsed() >= std::time::Duration::from_millis(30),
                "resolved={resolved}: the gate waited for the change"
            );
            assert_eq!(
                work.sequence.load(Ordering::SeqCst),
                2,
                "one change, completed"
            );
        }
    }

    /// A work owner whose decisions begin one owner change, held for the
    /// next scripted duration, each time an entry staging records its entry
    /// at a boundary with changes left: that staging's final check then finds
    /// the change in progress and refuses to rebuild.
    struct EntryChurnWork {
        sequence: Arc<std::sync::atomic::AtomicU64>,
        /// Holds of the changes left for root stagings (`ToolDispatch`) and
        /// for the execution policy gate's stagings (`ToolExecutionPolicy`).
        root: Arc<std::sync::Mutex<std::collections::VecDeque<std::time::Duration>>>,
        gate: Arc<std::sync::Mutex<std::collections::VecDeque<std::time::Duration>>>,
    }

    struct EntryChurnPrepared {
        sequence: Arc<std::sync::atomic::AtomicU64>,
        at: u64,
        root: Arc<std::sync::Mutex<std::collections::VecDeque<std::time::Duration>>>,
        gate: Arc<std::sync::Mutex<std::collections::VecDeque<std::time::Duration>>>,
    }

    impl EntryChurnWork {
        fn new(root: Vec<std::time::Duration>, gate: Vec<std::time::Duration>) -> Arc<Self> {
            Arc::new(Self {
                sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                root: Arc::new(std::sync::Mutex::new(root.into())),
                gate: Arc::new(std::sync::Mutex::new(gate.into())),
            })
        }
    }

    impl WorkAuthorization for EntryChurnWork {
        fn owner_change_in_progress(&self) -> Option<crate::authorization::OwnerChangeInProgress> {
            let sequence = self.sequence.load(Ordering::SeqCst);
            (sequence % 2 == 1).then(|| {
                crate::authorization::OwnerChangeInProgress::new(uuid::Uuid::nil(), sequence)
            })
        }

        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            let at = self.sequence.load(Ordering::SeqCst);
            if at % 2 == 1 {
                return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
            }
            Ok(Arc::new(EntryChurnPrepared {
                sequence: Arc::clone(&self.sequence),
                at,
                root: Arc::clone(&self.root),
                gate: Arc::clone(&self.gate),
            }))
        }
    }

    impl PreparedOperationAuthorization for EntryChurnPrepared {
        fn review_tier(&self) -> crate::authorization::OperationReviewTier {
            crate::authorization::OperationReviewTier::R1
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            if self.sequence.load(Ordering::SeqCst) != self.at {
                return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
            }
            Ok(())
        }

        fn observe(
            &self,
            _binding: &PreparedAuthorizationBinding,
            event: OperationObservation,
        ) -> Result<(), OperationObservationError> {
            let script = match event {
                OperationObservation::Entry(
                    crate::authorization::OperationEntryBoundary::ToolDispatch,
                ) => Some(&self.root),
                OperationObservation::Entry(
                    crate::authorization::OperationEntryBoundary::ToolExecutionPolicy,
                ) => Some(&self.gate),
                _ => None,
            };
            if let Some(hold) = script.and_then(|script| script.lock().unwrap().pop_front()) {
                begin_change(&self.sequence, hold);
            }
            Ok(())
        }
    }

    /// What one fenced call through the execution policy gate did under an
    /// [`EntryChurnWork`].
    struct GatedEntryChurn {
        result: Result<crate::ops::ToolDispatchOutcome, crate::ToolError>,
        /// How many bodies ran.
        bodies: usize,
        /// The work's publication sequence when the call returned: two per
        /// completed change, and one more while a change is in progress.
        sequence: u64,
        /// The scripted changes never begun, at the root staging and at the
        /// gate's staging.
        left: (usize, usize),
        took: std::time::Duration,
    }

    /// A fenced call through the execution policy gate under an
    /// [`EntryChurnWork`] scripted with changes of `root` holds at the root
    /// staging and `gate` holds at the gate's staging.
    async fn gated_entry_churn(
        root: Vec<std::time::Duration>,
        gate: Vec<std::time::Duration>,
    ) -> GatedEntryChurn {
        let work = EntryChurnWork::new(root, gate);
        let inner = Arc::new(SourceKindDispatcher::default());
        let gated: Arc<dyn AgentToolDispatcher> =
            Arc::new(crate::ExecutionPolicyGatedDispatcher::new(
                Arc::clone(&inner),
                crate::ToolExecutionPolicy::unrestricted(),
            ));
        let context = ToolDispatchContext::default().with_work_authorization(Some(
            crate::WorkAuthorizationContext::new(
                Arc::clone(&work) as Arc<dyn WorkAuthorization>,
                crate::OperationExecutionScope::Domain,
            ),
        ));
        let started = tokio::time::Instant::now();
        let result = fenced(&gated, "read_file", &context).await;
        let took = started.elapsed();
        GatedEntryChurn {
            result,
            bodies: inner.0.load(Ordering::SeqCst),
            sequence: work.sequence.load(Ordering::SeqCst),
            left: (
                work.root.lock().unwrap().len(),
                work.gate.lock().unwrap().len(),
            ),
            took,
        }
    }

    /// Every scripted change began and `waited` of them, all, were waited
    /// out; then the call entered and its body ran once.
    fn assert_waited_and_entered(case: &str, call: &GatedEntryChurn, waited: u64) {
        assert!(
            call.result
                .as_ref()
                .is_ok_and(|outcome| !outcome.result.is_error),
            "{case}: every change waited out, then entered: {:?}",
            call.result
        );
        assert_eq!(call.bodies, 1, "{case}: the body ran once");
        assert_eq!(call.left, (0, 0), "{case}: every scripted change began");
        assert_eq!(
            call.sequence,
            2 * waited,
            "{case}: {waited} changes, all waited out"
        );
    }

    /// Every scripted change began, so the gate staged after the root
    /// staging passed; exactly `waited` waits settled, and the last change,
    /// begun at the gate's last staging, was never waited out: the call
    /// settled there with the typed refusal, before any body ran.
    fn assert_refused_at_the_gate(case: &str, call: &GatedEntryChurn, waited: u64) {
        assert!(
            matches!(&call.result, Err(crate::ToolError::AuthorizationRefused { refusal })
                if refusal.kind() == OperationRefusalKind::ReprepareRequired),
            "{case}: the shared wait state is spent, typed refusal: {:?}",
            call.result
        );
        assert_eq!(call.bodies, 0, "{case}: no body ran");
        assert_eq!(call.left, (0, 0), "{case}: every scripted change began");
        assert_eq!(
            call.sequence,
            2 * waited + 1,
            "{case}: exactly {waited} changes waited out, the last one still in progress"
        );
    }

    /// One review attempt's free rebuilds (3) are shared by its root staging
    /// and the execution policy gate's staging: three changes spread over
    /// both are waited out; with two at the root staging, the gate's second
    /// change finds them spent, and the call settles with the typed refusal
    /// at the gate after exactly three settled waits.
    #[tokio::test(start_paused = true)]
    async fn the_root_staging_and_the_gate_share_one_budget_of_free_rebuilds() {
        let ms = std::time::Duration::from_millis;
        assert_waited_and_entered(
            "one change at the root staging, two at the gate",
            &gated_entry_churn(vec![ms(10)], vec![ms(10), ms(10)]).await,
            3,
        );
        assert_refused_at_the_gate(
            "two changes at each staging",
            &gated_entry_churn(vec![ms(10), ms(10)], vec![ms(10), ms(10)]).await,
            3,
        );
    }

    /// One review attempt's wait deadline (2 s from its first wait) is shared
    /// by its root staging and the execution policy gate's staging: the
    /// gate's wait ends at the deadline the root staging's first wait set,
    /// so a change that outlasts it settles with the typed refusal at the
    /// gate, never waiting past that deadline.
    #[tokio::test(start_paused = true)]
    async fn the_root_staging_and_the_gate_share_one_wait_deadline() {
        let ms = std::time::Duration::from_millis;
        assert_waited_and_entered(
            "0.5 s at the root staging, then 1 s at the gate",
            &gated_entry_churn(vec![ms(500)], vec![ms(1_000)]).await,
            2,
        );
        let spent = gated_entry_churn(vec![ms(1_500)], vec![ms(1_000)]).await;
        assert_refused_at_the_gate("1.5 s at the root staging, then 1 s at the gate", &spent, 1);
        assert!(
            spent.took >= ms(2_000) && spent.took < ms(2_100),
            "the gate waits until the shared deadline, never past it: {:?}",
            spent.took
        );
    }

    /// The reviewed tool the body-entry check refused for a change that
    /// began after its review: the wait ends with a decision prepared from
    /// current owners, the review check point finds the reviewed one
    /// superseded, and the dispatch reviews the current decision once more
    /// (with the same material) instead of reusing the earlier allow.
    #[tokio::test(start_paused = true)]
    async fn a_waited_body_entry_never_reuses_an_earlier_review() {
        // The entry's check, the post-review check and the verdict commit's
        // final check pass; the change begins after the third, before the
        // body entry.
        let work = ChurnWork::new(crate::authorization::OperationReviewTier::R2, 0, Some(3));
        let reviewer = Arc::new(CountingAllow(std::sync::atomic::AtomicUsize::new(0)));
        let context = churn_context(&work).with_operation_review(Some(Arc::new(
            crate::approval::review::BoundOperationReview::new(
                Arc::clone(&reviewer) as Arc<dyn crate::approval::review::OperationReviewer>,
                crate::approval::ApprovalService::new(),
                std::time::Duration::from_secs(30),
            ),
        )));
        let dispatcher = Arc::new(Reviewed(SourceKindDispatcher::default()));
        let outcome = fenced(&dispatcher, "read_file", &context)
            .await
            .expect("the current decision is reviewed once more, then entered");
        assert!(!outcome.result.is_error);
        assert_eq!(
            reviewer.0.load(Ordering::SeqCst),
            2,
            "reviewed again, never by reusing the first allow"
        );
        assert_eq!(
            dispatcher.0.0.load(Ordering::SeqCst),
            1,
            "the body ran once"
        );
    }

    /// The operation refusal a fenced dispatch result carries, with the
    /// `data` its model payload renders; `None` for any other result.
    fn refusal_and_data(
        result: &Result<crate::ops::ToolDispatchOutcome, crate::ToolError>,
    ) -> Option<(OperationRefused, serde_json::Value)> {
        let Err(error) = result else {
            return None;
        };
        let crate::ToolError::AuthorizationRefused { refusal } = error.primary_error() else {
            return None;
        };
        Some((*refusal, error.to_error_payload()["data"].clone()))
    }

    /// The source-kind tools, whose bodies count their effect and then
    /// refuse, as a body whose later governed step refused does.
    #[derive(Default)]
    struct RefusingBody(std::sync::atomic::AtomicUsize);

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for RefusingBody {
        fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
            SourceKindDispatcher::default().tools()
        }

        async fn dispatch(
            &self,
            _call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(crate::ToolError::AuthorizationRefused {
                refusal: OperationRefused::new(OperationRefusalKind::Denied),
            })
        }
    }

    /// A tool application ingress current when the application submitted
    /// the call, whose custody, checked again after the dispatch's waits
    /// immediately before entry, is refused.
    struct CustodyRefusedBeforeEntry;

    impl crate::ToolApplicationIngress for CustodyRefusedBeforeEntry {
        fn revalidate(&self) -> Result<(), OperationAuthorizationError> {
            Ok(())
        }

        fn revalidate_async(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), OperationAuthorizationError>>
                    + Send
                    + '_,
            >,
        > {
            let refused: Result<(), OperationAuthorizationError> =
                Err(OperationRefused::new(OperationRefusalKind::Denied).into());
            Box::pin(async move { refused })
        }

        fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
            self
        }
    }

    /// One certified refusal row: its name, the fenced result, how many
    /// bodies ran, and the closed kind the refusal carries.
    type CertifiedRow = (
        &'static str,
        Result<crate::ops::ToolDispatchOutcome, crate::ToolError>,
        usize,
        OperationRefusalKind,
    );

    /// Every row ended with its operation refusal certified not dispatched:
    /// the model reads the closed kind with `tool_invoked: false`, and no
    /// body ran.
    fn assert_certified_not_dispatched(rows: Vec<CertifiedRow>) {
        for (row, result, bodies, kind) in rows {
            let refused = refusal_and_data(&result);
            assert!(
                refused.is_some(),
                "{row}: an operation refusal, not {result:?}"
            );
            let (refusal, data) = refused.expect("checked above");
            assert_eq!(refusal.kind(), kind, "{row}");
            assert_eq!(
                refusal.dispatch(),
                crate::authorization::RefusalDispatch::NotDispatched,
                "{row}"
            );
            assert_eq!(
                data,
                serde_json::json!({"kind": kind, "tool_invoked": false}),
                "{row}"
            );
            assert_eq!(bodies, 0, "{row}: no body ran");
        }
    }

    /// Every operation refusal a fenced entry meets before it invokes the
    /// dispatcher is certified at that entry's one boundary: the model reads
    /// the closed kind with `tool_invoked: false`, and no body ran. One row
    /// per pre-invocation refusal, on both fenced entries.
    /// - The execution policy gate's own staging refusals are certified in
    ///   `the_policy_gate_certifies_its_own_staging_refusals_not_dispatched`.
    /// - A retained consent attempt refused at its staging, once its
    ///   candidate resolved, is certified in
    ///   `approval::consent::tests::a_refused_consent_attempt_reaches_the_host_observer_as_its_call`,
    ///   which has the approval owner it needs.
    /// - A consent attempt's own admission refuses only with review outcomes,
    ///   never an operation refusal, so it has no row.
    #[tokio::test(start_paused = true)]
    async fn every_pre_invocation_refusal_is_certified_not_dispatched() {
        let mut rows: Vec<CertifiedRow> = Vec::new();

        // A governed comms plan without its owner's target.
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let governed =
            ToolDispatchContext::default().with_work_authorization(Some(governed_work()));
        let result = fenced(&dispatcher, "send_message", &governed).await;
        rows.push((
            "owner target",
            result,
            dispatcher.0.load(Ordering::SeqCst),
            OperationRefusalKind::MalformedFacts,
        ));

        // The prepared entry's denial.
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let denied = ToolDispatchContext::default().with_work_authorization(Some(
            crate::WorkAuthorizationContext::new(
                Arc::new(Work(Arc::new(AtomicBool::new(true)))),
                crate::OperationExecutionScope::Domain,
            ),
        ));
        let result = fenced(&dispatcher, "read_file", &denied).await;
        rows.push((
            "prepared entry",
            result,
            dispatcher.0.load(Ordering::SeqCst),
            OperationRefusalKind::Denied,
        ));

        // The prepared entry's refusal to rebuild once its free rebuilds
        // are spent.
        let work = ChurnWork::new(crate::authorization::OperationReviewTier::R1, 10, None);
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let result = fenced(&dispatcher, "read_file", &churn_context(&work)).await;
        rows.push((
            "prepared entry past its wait bounds",
            result,
            dispatcher.0.load(Ordering::SeqCst),
            OperationRefusalKind::ReprepareRequired,
        ));

        // The root staging's refusal to rebuild past its wait bound.
        let work = ChurnWork::held(crate::authorization::OperationReviewTier::R1, Some(1));
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let result = fenced(&dispatcher, "read_file", &churn_context(&work)).await;
        rows.push((
            "root staging past its wait bound",
            result,
            dispatcher.0.load(Ordering::SeqCst),
            OperationRefusalKind::ReprepareRequired,
        ));

        // The tool application's custody, checked again immediately before
        // entry, refused.
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let control = crate::ToolApplicationControlRequest::from_trusted_ingress(
            crate::types::SessionId::new(),
            crate::ToolApplicationRequest {
                tool_call_id: "call-1".into(),
                extension: "test.app".into(),
                operation: crate::ToolApplicationOperation::CallTool {
                    name: "read_file".into(),
                    arguments: serde_json::json!({}),
                },
            },
            Arc::new(CustodyRefusedBeforeEntry),
        )
        .expect("current when submitted");
        let applied = ToolDispatchContext::default()
            .with_tool_application_control(control)
            .with_work_authorization(Some(governed_work()));
        let result = fenced(&dispatcher, "read_file", &applied).await;
        rows.push((
            "tool application custody before entry",
            result,
            dispatcher.0.load(Ordering::SeqCst),
            OperationRefusalKind::Denied,
        ));

        // The retained consent entry, for a context without work.
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        let result = crate::dispatch_retained_consent_attempt(
            &dispatcher,
            &ToolDispatchContext::default(),
            &crate::ApprovalId::new(),
        )
        .await;
        rows.push((
            "consent entry without work",
            result,
            dispatcher.0.load(Ordering::SeqCst),
            OperationRefusalKind::MalformedFacts,
        ));

        assert_certified_not_dispatched(rows);
    }

    /// The execution policy gate certifies every operation refusal its own
    /// staging meets before it invokes its inner dispatcher, here after its
    /// consequence policy revoked the work, on both of its staging paths
    /// (resolved and lowered). This runs on real time: the policy evaluates
    /// on its own worker thread, and a paused clock would advance to the
    /// evaluation deadline while the worker's answer is still on its way.
    #[tokio::test]
    async fn the_policy_gate_certifies_its_own_staging_refusals_not_dispatched() {
        let mut rows: Vec<CertifiedRow> = Vec::new();
        for (row, resolved) in [
            ("policy gate staging (resolved)", true),
            ("policy gate staging (lowered)", false),
        ] {
            let revoked = Arc::new(AtomicBool::new(false));
            let revoke = Arc::clone(&revoked);
            let policy = crate::tool_execution_policy::tests::allowing_consequence_policy_running(
                move || revoke.store(true, Ordering::SeqCst),
            );
            let inner = Arc::new(SourceKindDispatcher::default());
            let gated: Arc<dyn AgentToolDispatcher> = Arc::new(
                crate::ExecutionPolicyGatedDispatcher::new(
                    Arc::clone(&inner),
                    crate::ToolExecutionPolicy::unrestricted(),
                )
                .with_consequence_policy(policy),
            );
            let root: Arc<dyn AgentToolDispatcher> = if resolved {
                gated
            } else {
                Arc::new(Lowering(gated))
            };
            let context = ToolDispatchContext::default().with_work_authorization(Some(
                crate::WorkAuthorizationContext::new(
                    Arc::new(Work(revoked)),
                    crate::OperationExecutionScope::Domain,
                ),
            ));
            let result = fenced(&root, "read_file", &context).await;
            rows.push((
                row,
                result,
                inner.0.load(Ordering::SeqCst),
                OperationRefusalKind::Denied,
            ));
        }
        assert_certified_not_dispatched(rows);
    }

    /// The host's operation refusal observer: it keeps what it is given.
    #[derive(Default)]
    struct RecordingObserver(
        std::sync::Mutex<Vec<crate::authorization::OperationRefusalObservation>>,
    );

    impl crate::authorization::OperationRefusalObserver for RecordingObserver {
        fn observe(&self, observation: &crate::authorization::OperationRefusalObservation) {
            self.0.lock().unwrap().push(observation.clone());
        }
    }

    impl RecordingObserver {
        fn take(&self) -> Vec<crate::authorization::OperationRefusalObservation> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    /// `context` reporting to `observer` for `session`, as the agent
    /// attaches it to every dispatch.
    fn observed_by(
        context: ToolDispatchContext,
        observer: &Arc<RecordingObserver>,
        session: &crate::types::SessionId,
    ) -> ToolDispatchContext {
        context.with_refusal_observer(Some(crate::authorization::RefusalObserverBinding::new(
            Arc::clone(observer) as Arc<dyn crate::authorization::OperationRefusalObserver>,
            session.clone(),
        )))
    }

    /// A refused tool call reaches the host as that call's result, once,
    /// with the closed kind and whether the tool code ran read from the very
    /// refusal its model payload renders: `NotDispatched` beside
    /// `tool_invoked: false`, `Unknown` beside `"unknown"`. An authorized
    /// call is not observed.
    #[tokio::test]
    async fn the_host_observes_a_refused_tool_call_with_its_payload_dispatch_fact() {
        let observer = Arc::new(RecordingObserver::default());
        let session = crate::types::SessionId::new();
        let denying = ToolDispatchContext::default().with_work_authorization(Some(
            crate::WorkAuthorizationContext::new(
                Arc::new(Work(Arc::new(AtomicBool::new(true)))),
                crate::OperationExecutionScope::Domain,
            ),
        ));
        let governed =
            ToolDispatchContext::default().with_work_authorization(Some(governed_work()));
        let rows: [(
            &str,
            Arc<dyn AgentToolDispatcher>,
            ToolDispatchContext,
            crate::authorization::RefusalDispatch,
            serde_json::Value,
        ); 2] = [
            (
                "refused at the prepared entry",
                Arc::new(SourceKindDispatcher::default()),
                denying,
                crate::authorization::RefusalDispatch::NotDispatched,
                serde_json::json!(false),
            ),
            (
                "refused by the dispatcher",
                Arc::new(RefusingBody::default()),
                governed.clone(),
                crate::authorization::RefusalDispatch::Unknown,
                serde_json::json!("unknown"),
            ),
        ];
        for (row, dispatcher, context, dispatch, tool_invoked) in rows {
            let result = fenced(
                &dispatcher,
                "read_file",
                &observed_by(context, &observer, &session),
            )
            .await;
            let (refusal, data) = refusal_and_data(&result).expect(row);
            assert_eq!(refusal.dispatch(), dispatch, "{row}");
            assert_eq!(
                data,
                serde_json::json!({"kind": "denied", "tool_invoked": tool_invoked}),
                "{row}"
            );
            let observed = observer.take();
            assert_eq!(observed.len(), 1, "{row}: {observed:?}");
            let observation = &observed[0];
            assert_eq!(observation.session_id(), &session, "{row}");
            assert_eq!(
                observation.surface(),
                &crate::authorization::OperationRefusalSurface::ToolCall {
                    tool_use_id: "call-1".into()
                },
                "{row}"
            );
            assert_eq!(
                observation.outcome(),
                crate::authorization::OperationRefusalOutcome::Refused {
                    kind: OperationRefusalKind::Denied,
                    dispatch,
                },
                "{row}: the observation carries the payload's kind and dispatch fact"
            );
            assert_eq!(
                observation.disposition(),
                crate::authorization::OperationRefusalDisposition::ToolResult,
                "{row}"
            );
            assert!(observation.operation_id().is_some(), "{row}: governed");
        }
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        fenced(
            &dispatcher,
            "read_file",
            &observed_by(governed, &observer, &session),
        )
        .await
        .expect("the authorized call runs");
        assert!(
            observer.take().is_empty(),
            "an authorized call is not observed"
        );
    }

    /// The prepared entry's wait for an owner change is reported on the
    /// call, under the call's own operation, and the call then runs.
    #[tokio::test(start_paused = true)]
    async fn the_host_observes_a_tool_entry_waiting_out_an_owner_change() {
        let observer = Arc::new(RecordingObserver::default());
        let session = crate::types::SessionId::new();
        let work = ChurnWork::new(crate::authorization::OperationReviewTier::R1, 1, None);
        let dispatcher = Arc::new(SourceKindDispatcher::default());
        fenced(
            &dispatcher,
            "read_file",
            &observed_by(churn_context(&work), &observer, &session),
        )
        .await
        .expect("the call runs after the change");
        let observed = observer.take();
        assert_eq!(observed.len(), 1, "{observed:?}");
        let observation = &observed[0];
        assert_eq!(
            observation.surface(),
            &crate::authorization::OperationRefusalSurface::ToolCall {
                tool_use_id: "call-1".into()
            }
        );
        assert_eq!(
            observation.outcome(),
            crate::authorization::OperationRefusalOutcome::from(OperationRefused::new(
                OperationRefusalKind::ReprepareRequired
            ))
        );
        assert!(matches!(
            observation.disposition(),
            crate::authorization::OperationRefusalDisposition::WaitedForOwnerChange {
                settled: true,
                ..
            }
        ));
        assert!(observation.operation_id().is_some());
    }

    /// A tool whose body waits out an owner change on a delegate operation
    /// of its own, as the `web_search` helper's model request does, with
    /// the waiting state its dispatch context hands out.
    struct DelegateWaitingBody {
        delegate: Arc<ChurnWork>,
        operation: crate::OperationId,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for DelegateWaitingBody {
        fn tools(&self) -> Arc<[Arc<crate::types::ToolDef>]> {
            SourceKindDispatcher::default().tools()
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            self.dispatch_with_context(call, &ToolDispatchContext::default())
                .await
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &ToolDispatchContext,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
            let mut churn = context.owner_change_churn();
            begin_change(
                &self.delegate.sequence,
                std::time::Duration::from_millis(30),
            );
            let wait = churn
                .wait_for_settled_change(
                    self.delegate.as_ref(),
                    None,
                    "test_delegate",
                    Some(&self.operation),
                )
                .await;
            assert!(
                wait.is_some_and(crate::authorization::OwnerChangeWait::settled),
                "{wait:?}"
            );
            SourceKindDispatcher::default().dispatch(call).await
        }
    }

    /// A delegate operation's wait, made with the waiting state the call's
    /// dispatch context hands out, is reported as that call's, under the
    /// delegate's own operation id.
    #[tokio::test(start_paused = true)]
    async fn a_delegate_wait_inside_a_call_is_reported_as_that_call() {
        let observer = Arc::new(RecordingObserver::default());
        let session = crate::types::SessionId::new();
        let work = ChurnWork::new(crate::authorization::OperationReviewTier::R1, 0, None);
        let delegate = crate::OperationId::new();
        let dispatcher = Arc::new(DelegateWaitingBody {
            delegate: ChurnWork::new(crate::authorization::OperationReviewTier::R1, 0, None),
            operation: delegate.clone(),
        });
        fenced(
            &dispatcher,
            "read_file",
            &observed_by(churn_context(&work), &observer, &session),
        )
        .await
        .expect("the call runs after its delegate's wait");
        let observed = observer.take();
        assert_eq!(observed.len(), 1, "{observed:?}");
        let observation = &observed[0];
        assert_eq!(observation.session_id(), &session);
        assert_eq!(
            observation.surface(),
            &crate::authorization::OperationRefusalSurface::ToolCall {
                tool_use_id: "call-1".into()
            }
        );
        assert_eq!(observation.operation_id(), Some(&delegate));
        assert!(matches!(
            observation.disposition(),
            crate::authorization::OperationRefusalDisposition::WaitedForOwnerChange {
                settled: true,
                ..
            }
        ));
    }

    /// What the dispatcher returns is forwarded with no claim: its body may
    /// have produced an effect before a later governed step refused, so the
    /// model reads `tool_invoked: "unknown"`, under the fenced entry and
    /// through the execution policy gate on both of its paths.
    #[tokio::test]
    async fn a_refusal_the_dispatcher_returned_stays_unknown() {
        let governed =
            ToolDispatchContext::default().with_work_authorization(Some(governed_work()));
        let body = Arc::new(RefusingBody::default());
        let gated: Arc<dyn AgentToolDispatcher> =
            Arc::new(crate::ExecutionPolicyGatedDispatcher::new(
                Arc::clone(&body),
                crate::ToolExecutionPolicy::unrestricted(),
            ));
        let roots: [(&str, Arc<dyn AgentToolDispatcher>); 3] = [
            (
                "fenced entry",
                Arc::clone(&body) as Arc<dyn AgentToolDispatcher>,
            ),
            ("policy gate (resolved)", Arc::clone(&gated)),
            ("policy gate (lowered)", Arc::new(Lowering(gated))),
        ];
        for (row, root) in roots {
            let before = body.0.load(Ordering::SeqCst);
            let result = fenced(&root, "read_file", &governed).await;
            let (refusal, data) = refusal_and_data(&result).expect(row);
            assert_eq!(
                refusal.dispatch(),
                crate::authorization::RefusalDispatch::Unknown,
                "{row}"
            );
            assert_eq!(
                data,
                serde_json::json!({"kind": "denied", "tool_invoked": "unknown"}),
                "{row}"
            );
            assert_eq!(
                body.0.load(Ordering::SeqCst),
                before + 1,
                "{row}: the body ran"
            );
        }
    }
}
