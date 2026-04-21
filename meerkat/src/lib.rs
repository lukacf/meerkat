//! Meerkat - Rust Agentic Interface Kit
//!
//! A minimal, high-performance agent harness for LLM-powered applications.
//!
//! # Architecture
//!
//! All production surfaces (CLI, REST, RPC, MCP) use the **runtime-backed** path:
//! `SessionService` (substrate) + `MeerkatMachine` (control plane).
//! The runtime owns keep-alive, Queue/Steer routing, comms drain, and ingress.
//!
//! `build_ephemeral_service` is available for **testing and embedded use** where
//! runtime semantics (keep-alive, Steer, comms-driven admission) are not needed.
//! It supports Queue-only turns and will reject Steer/render_metadata.
//!
//! For the runtime-backed entry point, see [`meerkat_rpc::SessionRuntime`] or
//! the REST/MCP server crates.

// On wasm32, provide tokio alias backed by tokio_with_wasm.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

// Re-export core types
pub use meerkat_core::{
    // Agent
    Agent,
    AgentBuilder,
    AgentConfig,
    // Errors
    AgentError,
    // Events
    AgentEvent,
    AgentLlmClient,
    AgentSessionStore,
    AgentToolDispatcher,
    ArtifactRef,
    AssistantMessage,
    // Gateway for composing dispatchers
    Availability,
    AvailabilityCheck,
    // Budget
    Budget,
    BudgetConfig,
    BudgetLimits,
    BudgetPool,
    BudgetType,
    CompiledSchema,
    ConcurrencyLimits,
    // Config
    Config,
    ConfigDelta,
    ConfigError,
    // Phase 3 provider-auth redesign — realm-scoped connection identity.
    ConnectionRef,
    ContextStrategy,
    ForkBranch,
    ForkBudgetPolicy,
    HookCapability,
    HookDecision,
    HookEngine,
    HookEngineError,
    HookEntryConfig,
    HookExecutionMode,
    HookExecutionReport,
    HookFailurePolicy,
    HookId,
    HookInvocation,
    HookOutcome,
    HookPatch,
    HookPatchEnvelope,
    HookPoint,
    HookReasonCode,
    HookRevision,
    HookRunOverrides,
    HookRuntimeConfig,
    HookRuntimeKind,
    HooksConfig,
    // Interaction types
    InteractionContent,
    InteractionId,
    LlmStreamResult,
    // State
    LoopState,
    McpServerConfig,
    MeerkatSchema,
    // Types
    Message,
    OpEvent,
    OperationId,
    OperationPolicy,
    OperationResult,
    OperationSpec,
    OutputSchema,
    // Provider
    Provider,
    ResponseStatus,
    ResultShape,
    RetryConfig,
    // Retry
    RetryPolicy,
    RunResult,
    // Runtime epoch types
    RuntimeBuildMode,
    RuntimeEpochId,
    SESSION_VERSION,
    SchemaCompat,
    SchemaError,
    SchemaFormat,
    SchemaWarning,
    // Session
    Session,
    SessionId,
    SessionLlmIdentity,
    SessionMeta,
    SessionMetadata,
    SessionRuntimeBindings,
    SessionTooling,
    SpawnSpec,
    StopReason,
    StorageConfig,
    SystemMessage,
    ToolAccessPolicy,
    ToolCall,
    // Tool category override (tri-state for resume upgrade semantics)
    ToolCategoryOverride,
    ToolDef,
    ToolGateway,
    ToolGatewayBuilder,
    ToolResult,
    Usage,
    UserMessage,
    // Operations
    WorkKind,
};

// Config store types (filesystem-dependent — not available on wasm32)
#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_core::{
    ConfigEnvelope, ConfigResolvedPaths, ConfigRuntime, ConfigRuntimeError, ConfigSnapshot,
    ConfigStore, ConfigStoreMetadata, FileConfigStore, MemoryConfigStore, TaggedConfigStore,
};

// Re-export comms types from meerkat_comms
#[cfg(feature = "comms")]
pub use meerkat_comms::agent::{CommsContent, CommsMessage, CommsStatus};
#[cfg(feature = "comms")]
pub use meerkat_comms::{CommsRuntime, CommsRuntimeError, CommsToolMaterial, CoreCommsConfig};
#[cfg(feature = "comms")]
pub use meerkat_core::SessionServiceCommsExt;
pub use meerkat_core::SessionServiceControlExt;
pub use meerkat_core::SessionServiceHistoryExt;
#[cfg(feature = "comms")]
pub use meerkat_core::{
    CommsCommand, EventStream, InputSource, PeerDirectoryEntry, PeerDirectorySource, PeerName,
    SendError, SendReceipt, StreamError, StreamScope,
};

// Re-export client types
pub use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmResponse};
pub use meerkat_schedule::{
    CalendarFieldSpec, CalendarTriggerSpec, ClaimDueRequest, ClaimDueResult, CreateScheduleRequest,
    DeliveryCompletion, DeliveryDispatch, DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal,
    DisabledScheduleStore, ForkContextSpec, HelperOptionsSpec, IntervalTriggerSpec,
    MemoryScheduleStore, MisfirePolicy, MissingTargetPolicy, MobTargetBinding, Occurrence,
    OccurrenceFailureClass, OccurrenceFilter, OccurrenceId, OccurrenceOrdinal, OccurrencePhase,
    OverlapPolicy, SCHEDULE_TOOL_CAPABILITY_UNAVAILABLE, SCHEDULE_TOOL_INVALID_ARGUMENTS,
    SCHEDULE_TOOL_NOT_FOUND, Schedule, ScheduleDomainError, ScheduleDriver, ScheduleDriverConfig,
    ScheduleFilter, ScheduleId, SchedulePhase, ScheduleRevision, ScheduleService, ScheduleStore,
    ScheduleStoreError, ScheduleStoreKind, ScheduleTargetDelivery, ScheduleTargetProbe,
    ScheduleToolDispatcher, ScheduleToolError, ScheduledMobAction, ScheduledMobBackendKind,
    ScheduledMobRuntimeMode, ScheduledSessionAction, SessionMaterializationSpec,
    SessionTargetBinding, TargetBinding, TargetProbeOutcome, TriggerSpec, UpdateScheduleRequest,
    handle_schedule_tools_call, schedule_tools_list,
};
pub use meerkat_tools::ToolError;

// AgentFactory and build_agent types
mod factory;
pub use factory::{
    AgentBuildConfig, AgentFactory, BuildAgentError, DynAgent,
    decode_llm_client_override_from_service, encode_llm_client_override_for_service, provider_key,
    resolve_provider_api_key,
};

mod persistence;
pub use persistence::PersistenceBundle;
#[cfg(feature = "session-store")]
pub use persistence::PersistenceError;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use persistence::open_realm_persistence_in;

mod meerkat_machine;

// Factory-backed SessionService wiring (substrate — testing/embedded use).
// Production surfaces use runtime-backed paths (see meerkat-rpc, meerkat-rest).
mod service_factory;
pub use service_factory::{FactoryAgent, FactoryAgentBuilder, build_ephemeral_service};
#[cfg(feature = "session-store")]
pub use service_factory::{
    build_persistent_service, build_persistent_service_with_runtime_adapter,
};

// Session service
pub use meerkat_core::{
    AppendSystemContextRequest, AppendSystemContextResult, AppendSystemContextStatus,
    CreateSessionRequest, MobToolsBuildArgs, MobToolsFactory, SessionControlError, SessionError,
    SessionHistoryPage, SessionHistoryQuery, SessionInfo, SessionQuery, SessionService,
    SessionSummary, SessionUsage, SessionView, StartTurnRequest,
};
#[cfg(feature = "session-compaction")]
pub use meerkat_session::DefaultCompactor;
// PersistentSessionService: used by runtime-backed surfaces (REST, RPC, MCP).
// EphemeralSessionService: in-memory substrate for testing/embedded use only.
// Both implement SessionService. Production paths add MeerkatMachine on top.
#[cfg(feature = "session-store")]
pub use meerkat_session::PersistentSessionService;
pub use meerkat_session::{EphemeralSessionService, SessionAgent, SessionAgentBuilder};

#[cfg(feature = "anthropic")]
pub use meerkat_client::AnthropicClient;

#[cfg(feature = "openai")]
pub use meerkat_client::OpenAiClient;

#[cfg(feature = "gemini")]
pub use meerkat_client::GeminiClient;

// Re-export store types (trait + filter + error from core, backend error from meerkat-store)
pub use meerkat_store::{SessionFilter, SessionStore, SessionStoreError, StoreError};

#[cfg(feature = "jsonl-store")]
pub use meerkat_store::JsonlStore;

#[cfg(feature = "memory-store")]
pub use meerkat_store::MemoryStore;

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use meerkat_store::SqliteScheduleStore;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use meerkat_store::SqliteSessionStore;

// Re-export tools
#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_tools::{DispatchError, ToolDispatcher, ToolRegistry, ToolValidationError};

// Embedded skill registration.
//
// `mob-communication` must be available across surfaces because sessions can be
// created under one binary (for example CLI) and resumed under another (for
// example RPC). Register it from the base crate so any skills-enabled surface
// links the embedded skill inventory entry even when `meerkat-mob` itself is
// not compiled into that binary.
#[cfg(feature = "skills")]
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "mob-communication",
        name: "Mob Communication",
        description: "How to communicate with peers in a collaborative mob",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["comms"],
        body: include_str!("../skills/mob-communication/SKILL.md"),
        extensions: &[],
    }
}

// Re-export builtin tools infrastructure
#[cfg(feature = "comms")]
pub use meerkat_tools::CommsToolSurface;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use meerkat_tools::builtin::SqliteTaskStore;
pub use meerkat_tools::{
    BuiltinTool, BuiltinToolConfig, BuiltinToolEntry, BuiltinToolError, CompositeDispatcher,
    CompositeDispatcherError, EnforcedToolPolicy, MemoryTaskStore, ResolvedToolPolicy, TaskStore,
    ToolMode, ToolPolicyLayer,
};
#[cfg(not(target_arch = "wasm32"))]
pub use meerkat_tools::{FileTaskStore, ensure_rkat_dir, find_project_root};

// Re-export MCP client
#[cfg(feature = "mcp")]
pub use meerkat_mcp::{
    McpApplyDelta, McpApplyResult, McpConnection, McpError, McpLifecycleAction, McpLifecyclePhase,
    McpReloadTarget, McpRouter, McpRouterAdapter, McpServerLifecycleState,
};

// Skill types re-exports
pub use meerkat_core::skills::{
    SkillCollection, SkillDescriptor, SkillDocument, SkillFilter, SkillId, SkillIntrospectionEntry,
    SkillRuntime, SkillScope,
};

// Contracts re-exports
pub use meerkat_contracts::{
    self as contracts, CapabilitiesResponse, CapabilityHint, CapabilityId, CapabilityRegistration,
    CapabilityScope, CapabilityStatus, CommsParams, ContractVersion, CoreCreateParams,
    ErrorCategory, ErrorCode, HookParams, Protocol, RealtimeCapabilities,
    RealtimeCapabilitiesParams, RealtimeCapabilitiesResult, RealtimeChannelRole,
    RealtimeChannelState, RealtimeChannelStatus, RealtimeChannelTarget, RealtimeOpenInfo,
    RealtimeOpenRequest, RealtimeReconnectPolicy, RealtimeStatusParams, RealtimeStatusResult,
    RealtimeTurningMode, SkillEntry, SkillInspectResponse, SkillListResponse, SkillsParams,
    StructuredOutputParams, WireError, WireEvent, WireRunResult, WireSessionInfo,
    WireSessionSummary, WireUsage, build_capabilities,
};

// Surface infrastructure
pub mod surface;

mod realtime;
pub use realtime::RealtimeChannel;
#[cfg(not(target_arch = "wasm32"))]
pub use realtime::{
    RealtimeConnection, RealtimeConnectionError, RealtimeConnectionReceiver,
    RealtimeConnectionSender,
};

// Prompt assembly (filesystem-dependent: reads AGENTS.md, system_prompt_file)
#[cfg(not(target_arch = "wasm32"))]
mod prompt_assembly;
#[cfg(not(target_arch = "wasm32"))]
pub use prompt_assembly::assemble_system_prompt;

// SDK helpers (filesystem-dependent: .rkat dir, hook config files)
#[cfg(not(target_arch = "wasm32"))]
mod sdk;
#[cfg(not(target_arch = "wasm32"))]
pub use sdk::*;
#[cfg(not(target_arch = "wasm32"))]
mod sdk_config;
#[cfg(not(target_arch = "wasm32"))]
pub use sdk_config::SdkConfigStore;

// Comms tool composition (available on all platforms including wasm32)
#[cfg(feature = "comms")]
pub fn compose_tools_with_comms(
    base_tools: std::sync::Arc<dyn meerkat_core::AgentToolDispatcher>,
    tool_usage_instructions: String,
    material: meerkat_comms::CommsToolMaterial,
) -> Result<
    (
        std::sync::Arc<dyn meerkat_core::AgentToolDispatcher>,
        String,
    ),
    meerkat_tools::ToolError,
> {
    use meerkat_tools::CommsToolSurface;
    use std::sync::Arc;
    let router = material.router().clone();
    let trusted_peers = material.trusted_peers_shared();
    let self_pubkey = material.self_pubkey();
    let runtime = material.into_runtime();
    let comms_surface = CommsToolSurface::new_with_runtime(router, trusted_peers.clone(), runtime);
    let availability = CommsToolSurface::peer_availability(trusted_peers, self_pubkey);
    // Use DynamicToolComposite instead of a single ToolGateway so that
    // base_tools with dynamic tool lists (e.g. CallbackToolDispatcher backed
    // by a shared registry) can surface late additions via poll_external_updates.
    // The comms surface is wrapped in its own ToolGateway for availability gating.
    let comms_gateway = meerkat_core::ToolGatewayBuilder::new()
        .add_dispatcher_with_availability(Arc::new(comms_surface), availability)
        .build()?;
    let composite =
        meerkat_core::DynamicToolComposite::new(vec![base_tools, Arc::new(comms_gateway)]);
    let mut instructions = tool_usage_instructions;
    if !instructions.is_empty() {
        instructions.push_str("\n\n");
    }
    instructions.push_str(CommsToolSurface::usage_instructions());
    Ok((Arc::new(composite), instructions))
}

// Re-export provider-runtime types for downstream consumers that used to
// reach them via meerkat_providers::* or meerkat_client::*.
pub use meerkat_llm_core::provider_runtime::{
    ExternalAuthResolverHandle, ProviderAuthError, ProviderBindingError, ProviderClientError,
    ProviderRuntime, ProviderRuntimeRegistry, ResolverEnvironment,
};

/// Construct a [`meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry`] populated with the
/// feature-gated provider runtimes from the per-provider crates.
///
/// Replaces `meerkat_llm_core::ProviderRuntimeRegistry::default_registry()`,
/// which is empty post-B2-split because `meerkat-llm-core` cannot reach the
/// per-provider crates (cycle).
pub fn default_provider_registry() -> meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry {
    #[allow(unused_mut)]
    let mut r = meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry::empty();
    // Per-provider runtimes compile on wasm32: the resolver arms the
    // browser path exercises (`CredentialSourceSpec::InlineSecret` from
    // `populate_realm_from_api_keys`, `ExternalResolver` from the WASM
    // host's JS-registered auth callback) are target-neutral. OAuth /
    // IAM / filesystem arms inside `meerkat-auth-core::resolver` and
    // inside each provider's `resolve_binding` stay behind
    // `#[cfg(not(target_arch = "wasm32"))]`. Registering the runtimes
    // on all targets is what lets `build_agent` resolve provider
    // credentials in the browser (closes the smoke-lane Cluster C
    // "API key not set for provider 'anthropic'" failure for s47/s48).
    #[cfg(feature = "anthropic")]
    {
        r = r.with_runtime(std::sync::Arc::new(
            meerkat_anthropic::AnthropicProviderRuntime,
        ));
    }
    #[cfg(feature = "openai")]
    {
        r = r.with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    }
    #[cfg(feature = "gemini")]
    {
        r = r.with_runtime(std::sync::Arc::new(meerkat_gemini::GoogleProviderRuntime));
    }
    r
}

/// Prelude module for convenient imports
pub mod prelude {
    pub use super::{
        AgentConfig, AgentError, AgentEvent, AssistantMessage, Budget, BudgetLimits, BudgetType,
        Config, LlmClient, LlmError, LlmEvent, LlmRequest, Message, RealtimeChannel,
        RealtimeChannelRole, RealtimeReconnectPolicy, RealtimeTurningMode, RetryPolicy, RunResult,
        Session, SessionFilter, SessionId, SessionMeta, SessionStore, StopReason, SystemMessage,
        ToolCall, ToolDef, ToolResult, Usage, UserMessage,
    };

    #[cfg(feature = "anthropic")]
    pub use super::AnthropicClient;

    #[cfg(feature = "openai")]
    pub use super::OpenAiClient;

    #[cfg(feature = "gemini")]
    pub use super::GeminiClient;
}
