//! Agent builder.

use crate::budget::{Budget, BudgetLimits};
use crate::config::{AgentConfig, CallTimeoutOverride, HookRunOverrides};
use crate::hooks::HookEngine;
use crate::model_defaults::ModelOperationalDefaultsResolver;
use crate::ops::ConcurrencyLimits;
#[cfg(not(target_arch = "wasm32"))]
use crate::prompt::SystemPromptConfig;
use crate::retry::RetryPolicy;
use crate::session::{SESSION_TOOL_VISIBILITY_STATE_KEY, Session, SessionToolVisibilityState};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_catalog::{ToolCatalogDeferredEligibility, ToolCatalogMode, ToolPlaneClass};
use crate::tool_scope::{
    EXTERNAL_TOOL_FILTER_METADATA_KEY, INHERITED_TOOL_FILTER_METADATA_KEY,
    LocalToolVisibilityOwner, ToolFilter, ToolScope, ToolVisibilityOwner,
    validate_inherited_filter_witnesses,
};
use crate::types::{Message, OutputSchema};
use serde_json::Value;
#[cfg(meerkat_internal_agent_factory_build)]
use std::any::Any;
#[cfg(meerkat_internal_agent_factory_build)]
use std::future::Future;
#[cfg(meerkat_internal_agent_factory_build)]
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{
    Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime,
    select_tool_catalog_mode,
};

/// Builder for creating an Agent
#[derive(Default)]
pub struct AgentBuilder {
    pub(super) config: AgentConfig,
    pub(super) system_prompt: Option<String>,
    pub(super) budget_limits: Option<BudgetLimits>,
    pub(super) retry_policy: RetryPolicy,
    pub(super) session: Option<Session>,
    pub(super) concurrency_limits: ConcurrencyLimits,
    pub(super) depth: u32,
    pub(super) comms_runtime: Option<Arc<dyn CommsRuntime>>,
    pub(super) hook_engine: Option<Arc<dyn HookEngine>>,
    pub(super) hook_run_overrides: HookRunOverrides,
    pub(super) compactor: Option<Arc<dyn crate::compact::Compactor>>,
    pub(super) memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,
    pub(super) skill_engine: Option<Arc<crate::skills::SkillRuntime>>,
    pub(super) checkpointer: Option<Arc<dyn crate::checkpoint::SessionCheckpointer>>,
    pub(super) blob_store: Option<Arc<dyn crate::BlobStore>>,
    pub(super) silent_comms_intents: Vec<String>,
    pub(super) ops_lifecycle: Option<Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>>,
    pub(super) completion_feed: Option<Arc<dyn crate::completion_feed::CompletionFeed>>,
    pub(super) completion_enrichment:
        Option<Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>>,
    pub(super) max_inline_peer_notifications: Option<i32>,
    pub(super) event_tap: Option<crate::event_tap::EventTap>,
    pub(super) default_event_tx: Option<mpsc::Sender<crate::event::AgentEvent>>,
    pub(super) model_defaults_resolver: Option<Arc<dyn ModelOperationalDefaultsResolver>>,
    pub(super) call_timeout_override: CallTimeoutOverride,
    pub(super) epoch_cursor_state: Option<Arc<crate::runtime_epoch::EpochCursorState>>,
    pub(super) tool_visibility_owner: Option<Arc<dyn ToolVisibilityOwner>>,
    pub(super) turn_state_handle: Option<Arc<dyn crate::TurnStateHandle>>,
    pub(super) runtime_execution_kind_required: bool,
    #[allow(dead_code)]
    pub(super) runtime_execution_kind: Option<crate::lifecycle::RuntimeExecutionKind>,
    pub(super) external_tool_surface_handle: Option<Arc<dyn crate::ExternalToolSurfaceHandle>>,
    pub(super) auth_lease_handle: Option<Arc<dyn crate::handles::AuthLeaseHandle>>,
    pub(super) mcp_server_lifecycle_handle:
        Option<Arc<dyn crate::handles::McpServerLifecycleHandle>>,
}

/// Error returned when the canonical factory has not composed the policy state
/// required before crossing into core agent construction.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentBuildPolicyError {
    #[error("factory policy build requires an explicit session")]
    MissingSession,
    #[error("factory policy build requires session metadata")]
    MissingSessionMetadata,
    #[error("factory policy build requires session build state metadata")]
    MissingSessionBuildState,
    #[error("factory policy build requires a runtime turn-state handle")]
    MissingTurnStateHandle,
    #[error("runtime-backed agent build requires a canonical tool visibility owner")]
    MissingToolVisibilityOwner,
    #[error("runtime-backed agent build received legacy inherited tool visibility metadata")]
    LegacyInheritedToolFilterMetadata,
    #[error("runtime-backed agent build received inherited tool visibility without witnesses")]
    MissingInheritedToolVisibilityWitnesses,
    #[error("factory policy build requires the canonical factory bridge token")]
    InvalidFactoryBridgeToken,
    #[error("failed to restore canonical tool visibility state: {message}")]
    ToolVisibilityRestore { message: String },
    #[error("failed to persist canonical tool visibility state during restore: {message}")]
    ToolVisibilityPersist { message: String },
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(meerkat_internal_agent_factory_build)]
type AgentFactoryBuildFuture = Pin<
    Box<
        dyn Future<
                Output = Result<
                    Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>,
                    AgentBuildPolicyError,
                >,
            > + Send,
    >,
>;

#[cfg(target_arch = "wasm32")]
#[cfg(meerkat_internal_agent_factory_build)]
type AgentFactoryBuildFuture = Pin<
    Box<
        dyn Future<
            Output = Result<
                Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>,
                AgentBuildPolicyError,
            >,
        >,
    >,
>;

#[cfg(meerkat_internal_agent_factory_build)]
#[allow(improper_ctypes, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_agent_factory_policy_bridge_token_is_valid_v1_",
        env!("MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn facade_agent_factory_policy_bridge_token_is_valid(
        factory_bridge_token: &(dyn Any + Send + Sync),
    ) -> bool;
}

#[cfg(meerkat_internal_agent_factory_build)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_agent_factory_policy_build_v3_",
    env!("MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) unsafe extern "Rust" fn exported_agent_factory_policy_build(
    factory_bridge_token: &'static (dyn Any + Send + Sync),
    builder: AgentBuilder,
    client: Arc<dyn AgentLlmClient>,
    tools: Arc<dyn AgentToolDispatcher>,
    store: Arc<dyn AgentSessionStore>,
) -> AgentFactoryBuildFuture {
    Box::pin(async move {
        validate_factory_bridge_token(factory_bridge_token)?;
        builder.validate_factory_policy()?;
        builder.build_inner(client, tools, store).await
    })
}

#[cfg(meerkat_internal_agent_factory_build)]
fn validate_factory_bridge_token(
    token: &(dyn Any + Send + Sync),
) -> Result<(), AgentBuildPolicyError> {
    // The only authority core accepts is a positive check from the linked
    // facade crate. Public inventory/source metadata is intentionally not part
    // of this proof because downstream crates can spoof package names, source
    // paths, and fingerprints.
    #[allow(unsafe_code)]
    let is_valid = unsafe { facade_agent_factory_policy_bridge_token_is_valid(token) };
    if is_valid {
        Ok(())
    } else {
        Err(AgentBuildPolicyError::InvalidFactoryBridgeToken)
    }
}

impl AgentBuilder {
    /// Create a new agent builder with default config
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            system_prompt: None,
            budget_limits: None,
            retry_policy: RetryPolicy::default(),
            session: None,
            concurrency_limits: ConcurrencyLimits::default(),
            depth: 0,
            comms_runtime: None,
            hook_engine: None,
            hook_run_overrides: HookRunOverrides::default(),
            compactor: None,
            memory_store: None,
            skill_engine: None,
            checkpointer: None,
            blob_store: None,
            silent_comms_intents: Vec::new(),
            ops_lifecycle: None,
            completion_feed: None,
            completion_enrichment: None,
            max_inline_peer_notifications: None,
            event_tap: None,
            default_event_tx: None,
            model_defaults_resolver: None,
            call_timeout_override: CallTimeoutOverride::default(),
            epoch_cursor_state: None,
            tool_visibility_owner: None,
            turn_state_handle: None,
            runtime_execution_kind_required: false,
            runtime_execution_kind: None,
            external_tool_surface_handle: None,
            auth_lease_handle: None,
            mcp_server_lifecycle_handle: None,
        }
    }

    /// Set concurrency limits for delegated branches
    pub fn concurrency_limits(mut self, limits: ConcurrencyLimits) -> Self {
        self.concurrency_limits = limits;
        self
    }

    /// Set the nesting depth for delegated branches
    pub fn depth(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }

    /// Set the model to use
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set the system prompt
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set max tokens per turn
    pub fn max_tokens_per_turn(mut self, tokens: u32) -> Self {
        self.config.max_tokens_per_turn = tokens;
        self
    }

    /// Set temperature
    pub fn temperature(mut self, temp: f32) -> Self {
        self.config.temperature = Some(temp);
        self
    }

    /// Set budget limits
    pub fn budget(mut self, limits: BudgetLimits) -> Self {
        self.budget_limits = Some(limits);
        self
    }

    /// Set provider-specific parameters
    pub fn provider_params(mut self, params: Value) -> Self {
        self.config.provider_params = Some(params);
        self
    }

    /// Set provider-native tool defaults (resolved at build time, not persisted).
    pub fn provider_tool_defaults(mut self, defaults: Value) -> Self {
        self.config.provider_tool_defaults = Some(defaults);
        self
    }

    /// Set retry policy for LLM calls
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Set output schema for structured output extraction
    pub fn output_schema(mut self, schema: OutputSchema) -> Self {
        self.config.output_schema = Some(schema);
        self
    }

    /// Set the memory store for indexing compaction discards.
    pub fn memory_store(mut self, store: Arc<dyn crate::memory::MemoryStore>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Set maximum retries for structured output validation
    pub fn structured_output_retries(mut self, retries: u32) -> Self {
        self.config.structured_output_retries = retries;
        self
    }

    /// Resume from an existing session
    pub fn resume_session(mut self, session: Session) -> Self {
        self.session = Some(session);
        self
    }

    /// Set the comms runtime.
    pub fn with_comms_runtime(mut self, runtime: Arc<dyn CommsRuntime>) -> Self {
        self.comms_runtime = Some(runtime);
        self
    }

    /// Set the hook engine.
    pub fn with_hook_engine(mut self, hook_engine: Arc<dyn HookEngine>) -> Self {
        self.hook_engine = Some(hook_engine);
        self
    }

    /// Set run-scoped hook overrides.
    pub fn with_hook_run_overrides(mut self, overrides: HookRunOverrides) -> Self {
        self.hook_run_overrides = overrides;
        self
    }

    /// Set the context compactor.
    pub fn compactor(mut self, compactor: Arc<dyn crate::compact::Compactor>) -> Self {
        self.compactor = Some(compactor);
        self
    }

    /// Build a standalone low-level agent without facade/factory policy.
    ///
    /// This is an explicit escape hatch for core tests that own every loop
    /// primitive themselves. Production-facing Meerkat surfaces route through
    /// `AgentFactory::build_agent`.
    #[cfg(test)]
    #[allow(clippy::panic)]
    pub async fn build_standalone<C, T, S>(
        self,
        client: Arc<C>,
        tools: Arc<T>,
        store: Arc<S>,
    ) -> Agent<C, T, S>
    where
        C: AgentLlmClient + ?Sized,
        T: AgentToolDispatcher + ?Sized,
        S: AgentSessionStore + ?Sized,
    {
        match self.try_build_standalone(client, tools, store).await {
            Ok(agent) => agent,
            Err(err) => panic!("standalone agent build failed: {err}"),
        }
    }

    /// Build a standalone low-level agent and surface core build failures.
    #[cfg(test)]
    pub async fn try_build_standalone<C, T, S>(
        self,
        client: Arc<C>,
        tools: Arc<T>,
        store: Arc<S>,
    ) -> Result<Agent<C, T, S>, AgentBuildPolicyError>
    where
        C: AgentLlmClient + ?Sized,
        T: AgentToolDispatcher + ?Sized,
        S: AgentSessionStore + ?Sized,
    {
        self.build_inner(client, tools, store).await
    }

    #[cfg(meerkat_internal_agent_factory_build)]
    fn validate_factory_policy(&self) -> Result<(), AgentBuildPolicyError> {
        let session = self
            .session
            .as_ref()
            .ok_or(AgentBuildPolicyError::MissingSession)?;
        if session.session_metadata().is_none() {
            return Err(AgentBuildPolicyError::MissingSessionMetadata);
        }
        if session.build_state().is_none() {
            return Err(AgentBuildPolicyError::MissingSessionBuildState);
        }
        if self.turn_state_handle.is_none() {
            return Err(AgentBuildPolicyError::MissingTurnStateHandle);
        }
        if self.requires_explicit_runtime_tool_visibility_owner()
            && self.tool_visibility_owner.is_none()
        {
            return Err(AgentBuildPolicyError::MissingToolVisibilityOwner);
        }
        Ok(())
    }

    fn requires_explicit_runtime_tool_visibility_owner(&self) -> bool {
        // Session-owned runtime builds always carry one of these markers. The
        // standalone test seam may still use local visibility, but a
        // runtime-backed build must pass its machine-owned visibility owner
        // explicitly instead of receiving a hidden local fallback.
        self.runtime_execution_kind_required || self.epoch_cursor_state.is_some()
    }

    #[allow(dead_code)]
    async fn build_inner<C, T, S>(
        self,
        client: Arc<C>,
        tools: Arc<T>,
        store: Arc<S>,
    ) -> Result<Agent<C, T, S>, AgentBuildPolicyError>
    where
        C: AgentLlmClient + ?Sized,
        T: AgentToolDispatcher + ?Sized,
        S: AgentSessionStore + ?Sized,
    {
        let runtime_tool_visibility_owner_required =
            self.requires_explicit_runtime_tool_visibility_owner();
        let tool_visibility_owner = match self.tool_visibility_owner.clone() {
            Some(owner) => owner,
            None if runtime_tool_visibility_owner_required => {
                return Err(AgentBuildPolicyError::MissingToolVisibilityOwner);
            }
            None => Arc::new(LocalToolVisibilityOwner::new()),
        };
        let mut session = self.session.unwrap_or_default();
        let system_context_state = Arc::new(std::sync::Mutex::new(
            session.system_context_state().unwrap_or_default(),
        ));

        // Apply system prompt: use builder's prompt if set, otherwise compose default for new sessions
        let has_system_prompt = matches!(session.messages().first(), Some(Message::System(_)));
        if let Some(prompt) = self.system_prompt {
            session.set_system_prompt(prompt);
        } else if !has_system_prompt {
            // Only set default prompt for new sessions without an existing system prompt
            #[cfg(not(target_arch = "wasm32"))]
            {
                session.set_system_prompt(SystemPromptConfig::new().compose().await);
            }
            #[cfg(target_arch = "wasm32")]
            {
                session.set_system_prompt(String::new());
            }
        }

        let budget = Budget::new(self.budget_limits.unwrap_or_default());
        let catalog_mode = select_tool_catalog_mode(tools.as_ref());
        let (control_tool_names, deferred_tool_names) =
            if tools.tool_catalog_capabilities().exact_catalog {
                let catalog = tools.tool_catalog();
                let control_names = catalog
                    .iter()
                    .filter(|entry| entry.plane == ToolPlaneClass::Control)
                    .map(|entry| entry.tool.name.to_string())
                    .collect::<std::collections::HashSet<_>>();
                let deferred_names = if !control_names.is_empty()
                    && matches!(catalog_mode, ToolCatalogMode::Deferred)
                {
                    catalog
                        .iter()
                        .filter(|entry| entry.plane == ToolPlaneClass::Session)
                        .filter(|entry| {
                            matches!(
                                entry.deferred_eligibility,
                                ToolCatalogDeferredEligibility::DeferredEligible { .. }
                            )
                        })
                        .map(|entry| entry.tool.name.to_string())
                        .collect()
                } else {
                    std::collections::HashSet::new()
                };
                (control_names, deferred_names)
            } else {
                (
                    std::collections::HashSet::new(),
                    std::collections::HashSet::new(),
                )
            };
        let tool_scope = ToolScope::new_with_visibility_owner(
            tools.tools(),
            control_tool_names,
            deferred_tool_names,
            tool_visibility_owner,
        );
        let compaction_cadence = crate::agent::compact::load_compaction_cadence(&session);

        let mut agent = Agent {
            config: self.config,
            client,
            tools,
            tool_scope,
            store,
            session,
            budget,
            retry_policy: self.retry_policy,
            depth: self.depth,
            comms_runtime: self.comms_runtime,
            hook_engine: self.hook_engine,
            hook_run_overrides: self.hook_run_overrides,
            compactor: self.compactor,
            last_input_tokens: 0,
            compaction_cadence,
            memory_store: self.memory_store,
            skill_engine: self.skill_engine,
            pending_skill_references: None,
            terminal_error_detail: None,
            run_completed_hooks_applied: false,
            run_completed_event_emitted: false,
            silent_comms_intents: self.silent_comms_intents,
            checkpointer: self.checkpointer,
            blob_store: self.blob_store,
            event_tap: self
                .event_tap
                .unwrap_or_else(crate::event_tap::new_event_tap),
            system_context_state,
            default_event_tx: self.default_event_tx,
            ops_lifecycle: self.ops_lifecycle,
            // Seed from epoch cursor state if available (runtime-backed surfaces),
            // otherwise fall back to the feed watermark to avoid replaying retained
            // completions from prior agent lifetimes (stop/resume, live reattach).
            // Same pattern as runtime_loop.rs line 276. Computed before move.
            applied_cursor: self
                .epoch_cursor_state
                .as_ref()
                .map(|cs| {
                    cs.agent_applied_cursor
                        .load(std::sync::atomic::Ordering::Acquire)
                })
                .unwrap_or_else(|| self.completion_feed.as_ref().map_or(0, |f| f.watermark())),
            completion_feed: self.completion_feed,
            epoch_cursor_state: self.epoch_cursor_state,
            completion_enrichment: self.completion_enrichment,
            mob_authority_handle: None,
            runtime_execution_kind_required: self.runtime_execution_kind_required,
            runtime_execution_kind: self.runtime_execution_kind,
            turn_state_handle: self.turn_state_handle,
            external_tool_surface_handle: self.external_tool_surface_handle,
            auth_lease_handle: self.auth_lease_handle,
            mcp_server_lifecycle_handle: self.mcp_server_lifecycle_handle,
            cancel_after_boundary_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            model_defaults_resolver: self.model_defaults_resolver,
            call_timeout_override: self.call_timeout_override,
            extraction_state: super::extraction::ExtractionState::default(),
            last_hidden_deferred_catalog_names: Default::default(),
            last_pending_catalog_sources: Default::default(),
            tool_dispatch_context: Default::default(),
        };

        let has_canonical_visibility_state = agent
            .session
            .metadata()
            .contains_key(SESSION_TOOL_VISIBILITY_STATE_KEY);
        let mut visibility_state = match agent.session.tool_visibility_state() {
            Ok(Some(state)) => state,
            Ok(None) => SessionToolVisibilityState::default(),
            Err(err) => {
                return Err(AgentBuildPolicyError::ToolVisibilityRestore {
                    message: format!(
                        "failed to decode canonical session metadata `{SESSION_TOOL_VISIBILITY_STATE_KEY}`: {err}"
                    ),
                });
            }
        };
        if runtime_tool_visibility_owner_required
            && agent
                .session
                .metadata()
                .contains_key(INHERITED_TOOL_FILTER_METADATA_KEY)
        {
            tracing::error!(
                metadata_key = INHERITED_TOOL_FILTER_METADATA_KEY,
                "runtime-backed agent build rejected legacy inherited tool visibility metadata"
            );
            return Err(AgentBuildPolicyError::LegacyInheritedToolFilterMetadata);
        }
        if runtime_tool_visibility_owner_required
            && let Err(err) = validate_inherited_filter_witnesses(
                &visibility_state.inherited_base_filter,
                &visibility_state.filter_witnesses,
            )
        {
            tracing::error!(
                error = %err,
                "runtime-backed agent build rejected inherited tool visibility without witnesses"
            );
            return Err(AgentBuildPolicyError::MissingInheritedToolVisibilityWitnesses);
        }

        if !has_canonical_visibility_state && !runtime_tool_visibility_owner_required {
            if let Some(raw_filter) = agent
                .session
                .metadata()
                .get(EXTERNAL_TOOL_FILTER_METADATA_KEY)
                .cloned()
            {
                match serde_json::from_value::<ToolFilter>(raw_filter) {
                    Ok(filter) => {
                        visibility_state.active_filter = filter.clone();
                        visibility_state.staged_filter = filter;
                    }
                    Err(err) => {
                        return Err(AgentBuildPolicyError::ToolVisibilityRestore {
                            message: format!(
                                "failed to decode legacy session metadata `{EXTERNAL_TOOL_FILTER_METADATA_KEY}`: {err}"
                            ),
                        });
                    }
                }
            }

            if let Some(raw_filter) = agent
                .session
                .metadata()
                .get(INHERITED_TOOL_FILTER_METADATA_KEY)
                .cloned()
            {
                match serde_json::from_value::<ToolFilter>(raw_filter) {
                    Ok(filter) => {
                        visibility_state.inherited_base_filter = filter;
                    }
                    Err(err) => {
                        return Err(AgentBuildPolicyError::ToolVisibilityRestore {
                            message: format!(
                                "failed to decode legacy session metadata `{INHERITED_TOOL_FILTER_METADATA_KEY}`: {err}"
                            ),
                        });
                    }
                }
            }
        }

        if visibility_state != SessionToolVisibilityState::default()
            || has_canonical_visibility_state
        {
            if let Err(err) = agent
                .tool_scope
                .set_visibility_state(visibility_state.clone())
            {
                return Err(AgentBuildPolicyError::ToolVisibilityRestore {
                    message: err.to_string(),
                });
            }
            if !runtime_tool_visibility_owner_required {
                if let Err(err) = agent.session.set_tool_visibility_state(visibility_state) {
                    return Err(AgentBuildPolicyError::ToolVisibilityPersist {
                        message: err.to_string(),
                    });
                }
                agent
                    .session
                    .remove_metadata(EXTERNAL_TOOL_FILTER_METADATA_KEY);
                agent
                    .session
                    .remove_metadata(INHERITED_TOOL_FILTER_METADATA_KEY);
            }
        }

        Ok(agent)
    }

    /// Set the session checkpointer for keep-alive persistence.
    pub fn with_checkpointer(
        mut self,
        cp: Arc<dyn crate::checkpoint::SessionCheckpointer>,
    ) -> Self {
        self.checkpointer = Some(cp);
        self
    }

    /// Set the blob store used to hydrate image refs before execution.
    pub fn with_blob_store(mut self, blob_store: Arc<dyn crate::BlobStore>) -> Self {
        self.blob_store = Some(blob_store);
        self
    }

    /// Set comms intents that should be silently injected into the session
    /// without triggering an LLM turn.
    pub fn with_silent_comms_intents(mut self, intents: Vec<String>) -> Self {
        self.silent_comms_intents = intents;
        self
    }

    /// Set max peer-count threshold for inline peer lifecycle notification injection.
    pub fn with_max_inline_peer_notifications(mut self, threshold: Option<i32>) -> Self {
        self.max_inline_peer_notifications = threshold;
        self
    }

    /// Set the ops lifecycle registry for async operation tracking.
    pub fn with_ops_lifecycle(
        mut self,
        registry: Arc<dyn crate::ops_lifecycle::OpsLifecycleRegistry>,
    ) -> Self {
        self.ops_lifecycle = Some(registry);
        self
    }

    /// Set the completion feed for cursor-based completion delivery.
    pub fn with_completion_feed(
        mut self,
        feed: Arc<dyn crate::completion_feed::CompletionFeed>,
    ) -> Self {
        self.completion_feed = Some(feed);
        self
    }

    /// Set the enrichment provider for completion display details.
    pub fn with_completion_enrichment(
        mut self,
        enrichment: Arc<dyn crate::completion_feed::CompletionEnrichmentProvider>,
    ) -> Self {
        self.completion_enrichment = Some(enrichment);
        self
    }

    /// Set the skill engine for per-turn `/skill-ref` activation.
    pub fn with_skill_engine(mut self, engine: Arc<crate::skills::SkillRuntime>) -> Self {
        self.skill_engine = Some(engine);
        self
    }

    /// Set the event tap for interaction-scoped streaming.
    pub fn with_event_tap(mut self, tap: crate::event_tap::EventTap) -> Self {
        self.event_tap = Some(tap);
        self
    }

    /// Set a default event channel used when run methods are called without
    /// per-call event channels.
    pub fn with_default_event_tx(
        mut self,
        event_tx: mpsc::Sender<crate::event::AgentEvent>,
    ) -> Self {
        self.default_event_tx = Some(event_tx);
        self
    }

    /// Set the model operational defaults resolver for profile-derived call timeouts.
    ///
    /// The resolver is consulted at each LLM call to look up model-specific
    /// operational defaults (e.g., call timeout) for the current effective
    /// model/provider. This enables hot-swap-aware default resolution.
    pub fn with_model_defaults_resolver(
        mut self,
        resolver: Arc<dyn ModelOperationalDefaultsResolver>,
    ) -> Self {
        self.model_defaults_resolver = Some(resolver);
        self
    }

    /// Set the shared epoch cursor state for runtime-backed cursor writeback.
    pub fn with_epoch_cursor_state(
        mut self,
        state: Arc<crate::runtime_epoch::EpochCursorState>,
    ) -> Self {
        self.epoch_cursor_state = Some(state);
        self
    }

    /// Set the canonical durable tool-visibility owner for this build.
    pub fn with_tool_visibility_owner(mut self, owner: Arc<dyn ToolVisibilityOwner>) -> Self {
        self.tool_visibility_owner = Some(owner);
        self
    }

    /// Set the runtime-backed turn-state diagnostic handle for this build.
    pub fn with_turn_state_handle(mut self, handle: Arc<dyn crate::TurnStateHandle>) -> Self {
        self.turn_state_handle = Some(handle);
        self
    }

    /// Require runtime-stamped execution kind metadata before executing turns.
    pub fn require_runtime_execution_kind_stamp(mut self) -> Self {
        self.runtime_execution_kind_required = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_runtime_execution_kind_for_test(
        mut self,
        execution_kind: crate::lifecycle::RuntimeExecutionKind,
    ) -> Self {
        self.runtime_execution_kind = Some(execution_kind);
        self
    }

    /// Set the runtime-backed external tool-surface diagnostic handle for this build.
    pub fn with_external_tool_surface_handle(
        mut self,
        handle: Arc<dyn crate::ExternalToolSurfaceHandle>,
    ) -> Self {
        self.external_tool_surface_handle = Some(handle);
        self
    }

    /// Set the runtime-backed auth lease handle for this build (Phase 1.5-rev).
    pub fn with_auth_lease_handle(
        mut self,
        handle: Arc<dyn crate::handles::AuthLeaseHandle>,
    ) -> Self {
        self.auth_lease_handle = Some(handle);
        self
    }

    /// Set the runtime-backed MCP server lifecycle handle for this build
    /// (Phase 5G / T5g).
    ///
    /// When set, the agent loop reads `pending_server_ids()` from this handle
    /// to decide whether to emit the `[MCP_PENDING]` system notice at each
    /// CallingLlm boundary — authoritative DSL state replaces the shell-level
    /// `ext.pending` check.
    pub fn with_mcp_server_lifecycle_handle(
        mut self,
        handle: Arc<dyn crate::handles::McpServerLifecycleHandle>,
    ) -> Self {
        self.mcp_server_lifecycle_handle = Some(handle);
        self
    }

    /// Set the explicit call-timeout override from the build/config composition seam.
    ///
    /// - `Inherit`: defer to profile-derived default via the resolver
    /// - `Disabled`: explicitly suppress call timeout
    /// - `Value(d)`: explicitly set call timeout to `d`
    pub fn with_call_timeout_override(mut self, override_value: CallTimeoutOverride) -> Self {
        self.call_timeout_override = override_value;
        self
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::LlmStreamResult;
    use crate::connection::{AuthBindingRef, BindingId, RealmId};
    use crate::error::{AgentError, ToolError};
    use crate::event::AgentEvent;
    use crate::event_tap::EventTapState;
    use crate::handles::{
        AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition,
        DslTransitionError, LeaseKey,
    };
    use crate::types::{AssistantBlock, StopReason, ToolCallView, ToolDef, UserMessage};
    use async_trait::async_trait;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    struct MockClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for MockClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<LlmStreamResult, AgentError> {
            Ok(LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "Done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                crate::types::Usage::default(),
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct MockTools;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for MockTools {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Err(ToolError::NotFound {
                name: call.name.to_string(),
            })
        }
    }

    struct MockStore;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentSessionStore for MockStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }
        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    fn explicit_test_visibility_owner() -> Arc<dyn ToolVisibilityOwner> {
        Arc::new(LocalToolVisibilityOwner::new())
    }

    struct RestoreFailingVisibilityOwner {
        fallback_state: LocalToolVisibilityOwner,
        replace_calls: AtomicUsize,
    }

    impl RestoreFailingVisibilityOwner {
        fn new() -> Self {
            Self {
                fallback_state: LocalToolVisibilityOwner::new(),
                replace_calls: AtomicUsize::new(0),
            }
        }

        fn replace_calls(&self) -> usize {
            self.replace_calls.load(Ordering::SeqCst)
        }
    }

    impl ToolVisibilityOwner for RestoreFailingVisibilityOwner {
        fn visibility_state(
            &self,
        ) -> Result<SessionToolVisibilityState, crate::ToolScopeApplyError> {
            self.fallback_state.visibility_state()
        }

        fn replace_visibility_state(
            &self,
            _visibility_state: SessionToolVisibilityState,
        ) -> Result<(), crate::ToolScopeApplyError> {
            self.replace_calls.fetch_add(1, Ordering::SeqCst);
            Err(crate::ToolScopeApplyError::Owner {
                message: "restore fixture rejected canonical visibility state".to_string(),
            })
        }

        fn stage_persistent_filter(
            &self,
            filter: ToolFilter,
            witnesses: BTreeMap<String, crate::ToolVisibilityWitness>,
        ) -> Result<crate::ToolScopeRevision, crate::ToolScopeStageError> {
            self.fallback_state
                .stage_persistent_filter(filter, witnesses)
        }

        fn stage_requested_deferred_names(
            &self,
            names: BTreeSet<String>,
        ) -> Result<crate::ToolScopeRevision, crate::ToolScopeStageError> {
            self.fallback_state.stage_requested_deferred_names(names)
        }

        fn request_deferred_tools(
            &self,
            authorities: Vec<crate::DeferredToolLoadAuthority>,
        ) -> Result<crate::ToolScopeRevision, crate::ToolScopeStageError> {
            self.fallback_state.request_deferred_tools(authorities)
        }

        fn replace_deferred_tool_authority_catalog(
            &self,
            catalog: BTreeMap<String, crate::ToolVisibilityWitness>,
        ) {
            self.fallback_state
                .replace_deferred_tool_authority_catalog(catalog);
        }

        fn boundary_applied(
            &self,
        ) -> Result<SessionToolVisibilityState, crate::ToolScopeApplyError> {
            self.fallback_state.boundary_applied()
        }
    }

    struct RestoreFailureCatalogDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        catalog: Arc<[crate::ToolCatalogEntry]>,
    }

    impl RestoreFailureCatalogDispatcher {
        fn new() -> Self {
            let visible = Arc::new(ToolDef::new(
                "visible",
                "visible session tool",
                serde_json::json!({ "type": "object" }),
            ));
            let secret = Arc::new(ToolDef::new(
                "secret",
                "policy-hidden session tool",
                serde_json::json!({ "type": "object" }),
            ));
            let deferred_a = Arc::new(
                ToolDef::new(
                    "deferred_a",
                    "deferred session tool",
                    serde_json::json!({ "type": "object" }),
                )
                .with_provenance(crate::ToolProvenance {
                    kind: crate::ToolSourceKind::Callback,
                    source_id: "restore-fixture".into(),
                }),
            );
            let deferred_b = Arc::new(
                ToolDef::new(
                    "deferred_b",
                    "second deferred session tool",
                    serde_json::json!({ "type": "object" }),
                )
                .with_provenance(crate::ToolProvenance {
                    kind: crate::ToolSourceKind::Callback,
                    source_id: "restore-fixture".into(),
                }),
            );
            let control = Arc::new(ToolDef::new(
                "tool_catalog_load",
                "deferred catalog control tool",
                serde_json::json!({ "type": "object" }),
            ));

            Self {
                tools: vec![
                    Arc::clone(&visible),
                    Arc::clone(&secret),
                    Arc::clone(&deferred_a),
                    Arc::clone(&deferred_b),
                    Arc::clone(&control),
                ]
                .into(),
                catalog: vec![
                    crate::ToolCatalogEntry::session_inline(visible, true),
                    crate::ToolCatalogEntry::session_inline(secret, true),
                    crate::ToolCatalogEntry::session_deferred(
                        deferred_a,
                        true,
                        "callback:restore-fixture".to_string(),
                    ),
                    crate::ToolCatalogEntry::session_deferred(
                        deferred_b,
                        true,
                        "callback:restore-fixture".to_string(),
                    ),
                    crate::ToolCatalogEntry::control_inline(control, true),
                ]
                .into(),
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for RestoreFailureCatalogDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
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
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Err(ToolError::access_denied(call.name))
        }
    }

    #[derive(Default)]
    struct RecordingAuthLeaseHandle {
        snapshots: Mutex<BTreeMap<LeaseKey, AuthLeaseSnapshot>>,
    }

    impl RecordingAuthLeaseHandle {
        fn seed(&self, key: LeaseKey, snapshot: AuthLeaseSnapshot) {
            self.snapshots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key, snapshot);
        }
    }

    impl AuthLeaseHandle for RecordingAuthLeaseHandle {
        fn acquire_lease(
            &self,
            lease_key: &LeaseKey,
            expires_at: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            let mut snapshots = self
                .snapshots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let generation = snapshots
                .get(lease_key)
                .map(|snapshot| snapshot.generation + 1)
                .unwrap_or(1);
            snapshots.insert(
                lease_key.clone(),
                AuthLeaseSnapshot {
                    phase: Some(AuthLeasePhase::Valid),
                    expires_at: (expires_at != u64::MAX).then_some(expires_at),
                    credential_present: true,
                    generation,
                    credential_published_at_millis: None,
                },
            );
            Ok(AuthLeaseTransition {
                generation,
                credential_published_at_millis: None,
            })
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn complete_refresh(
            &self,
            lease_key: &LeaseKey,
            new_expires_at: u64,
            _now: u64,
        ) -> Result<AuthLeaseTransition, DslTransitionError> {
            self.acquire_lease(lease_key, new_expires_at)
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            self.snapshots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(lease_key);
            Ok(())
        }

        fn snapshot(&self, lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            self.snapshots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(lease_key)
                .cloned()
                .unwrap_or(AuthLeaseSnapshot {
                    phase: None,
                    expires_at: None,
                    credential_present: false,
                    generation: 0,
                    credential_published_at_millis: None,
                })
        }
    }

    fn auth_binding(binding: &str) -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse("dev").expect("valid realm fixture"),
            binding: BindingId::parse(binding).expect("valid binding fixture"),
            profile: None,
        }
    }

    #[tokio::test]
    async fn builder_fails_closed_when_canonical_visibility_restore_fails() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(RestoreFailureCatalogDispatcher::new());
        let store = Arc::new(MockStore);
        let visibility_owner = Arc::new(RestoreFailingVisibilityOwner::new());
        let mut session = Session::new();
        let hidden_filter = match serde_json::to_value(ToolFilter::Deny(
            ["secret".to_string()].into_iter().collect(),
        )) {
            Ok(value) => value,
            Err(err) => panic!("filter should serialize: {err}"),
        };
        session.set_metadata(EXTERNAL_TOOL_FILTER_METADATA_KEY, hidden_filter);

        let result = AgentBuilder::new()
            .resume_session(session)
            .with_tool_visibility_owner(visibility_owner.clone())
            .try_build_standalone(client, tools, store)
            .await;

        match result {
            Err(AgentBuildPolicyError::ToolVisibilityRestore { message }) => {
                assert!(
                    message.contains("restore fixture rejected"),
                    "restore error should preserve owner failure details: {message}"
                );
            }
            Ok(agent) => {
                let visible_names = agent
                    .tool_scope()
                    .visible_tool_names()
                    .unwrap_or_else(|_| Default::default());
                panic!(
                    "builder must not complete after canonical restore failure; visible names from stale/default authority: {visible_names:?}"
                );
            }
            Err(err) => panic!("unexpected build error: {err}"),
        }
        assert_eq!(
            visibility_owner.replace_calls(),
            1,
            "builder should attempt the canonical restore once, then fail closed"
        );
    }

    #[tokio::test]
    async fn builder_fails_closed_when_canonical_visibility_metadata_is_malformed() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(RestoreFailureCatalogDispatcher::new());
        let store = Arc::new(MockStore);
        let mut session = Session::new();
        session.set_metadata(
            SESSION_TOOL_VISIBILITY_STATE_KEY,
            serde_json::json!({
                "active_filter": {
                    "unexpected_filter_kind": ["secret"]
                }
            }),
        );

        let result = AgentBuilder::new()
            .resume_session(session)
            .try_build_standalone(client, tools, store)
            .await;

        match result {
            Err(AgentBuildPolicyError::ToolVisibilityRestore { message }) => {
                assert!(
                    message.contains("failed to decode canonical session metadata"),
                    "restore error should identify malformed canonical metadata: {message}"
                );
            }
            Ok(agent) => {
                let visible_names = agent
                    .tool_scope()
                    .visible_tool_names()
                    .unwrap_or_else(|_| Default::default());
                panic!(
                    "builder must not complete with default visibility after malformed canonical metadata; visible names: {visible_names:?}"
                );
            }
            Err(err) => panic!("unexpected build error: {err}"),
        }
    }

    #[tokio::test]
    async fn builder_fails_closed_when_legacy_visibility_filter_metadata_is_malformed() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(RestoreFailureCatalogDispatcher::new());
        let store = Arc::new(MockStore);
        let mut session = Session::new();
        session.set_metadata(
            EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::json!({
                "unexpected_filter_kind": ["secret"]
            }),
        );

        let result = AgentBuilder::new()
            .resume_session(session)
            .try_build_standalone(client, tools, store)
            .await;

        match result {
            Err(AgentBuildPolicyError::ToolVisibilityRestore { message }) => {
                assert!(
                    message.contains("failed to decode legacy session metadata"),
                    "restore error should identify malformed legacy visibility metadata: {message}"
                );
            }
            Ok(agent) => {
                let visible_names = agent
                    .tool_scope()
                    .visible_tool_names()
                    .unwrap_or_else(|_| Default::default());
                panic!(
                    "builder must not complete with default visibility after malformed legacy visibility metadata; visible names: {visible_names:?}"
                );
            }
            Err(err) => panic!("unexpected build error: {err}"),
        }
    }

    #[tokio::test]
    async fn auth_lease_rotation_preserves_existing_target_expiry() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);
        let auth_lease = Arc::new(RecordingAuthLeaseHandle::default());
        let previous = auth_binding("previous");
        let target = auth_binding("target");
        let target_key = LeaseKey::from_auth_binding(&target);
        auth_lease.seed(
            target_key.clone(),
            AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(1_900_000_000),
                credential_present: true,
                generation: 7,
                credential_published_at_millis: None,
            },
        );
        let agent = AgentBuilder::new()
            .with_auth_lease_handle(auth_lease.clone())
            .build_standalone(client, tools, store)
            .await;

        agent
            .rotate_auth_lease_auth_binding(Some(&previous), Some(&target))
            .unwrap();

        let snapshot = auth_lease.snapshot(&target_key);
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
        assert_eq!(snapshot.expires_at, Some(1_900_000_000));
        assert_eq!(snapshot.generation, 7);
    }

    /// Regression test: AgentBuilder should apply system_prompt to new sessions
    #[tokio::test]
    async fn test_regression_builder_applies_system_prompt_to_new_session() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let agent = AgentBuilder::new()
            .system_prompt("Custom system prompt")
            .build_standalone(client, tools, store)
            .await;

        // Check that the system prompt was applied
        let messages = agent.session().messages();
        assert!(!messages.is_empty(), "Session should have messages");

        match &messages[0] {
            Message::System(sys) => {
                assert_eq!(sys.content, "Custom system prompt");
            }
            other => panic!("First message should be System, got: {other:?}"),
        }
    }

    /// Regression test: AgentBuilder should apply system_prompt to resumed sessions
    /// Previously, system_prompt was ignored when resuming a session.
    #[tokio::test]
    async fn test_regression_builder_applies_system_prompt_to_resumed_session() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        // Create a session with an existing system prompt
        let mut existing_session = Session::new();
        existing_session.set_system_prompt("Original system prompt".to_string());
        existing_session.push(Message::User(UserMessage::text("Hello".to_string())));

        // Resume the session with a NEW system prompt
        let agent = AgentBuilder::new()
            .resume_session(existing_session)
            .system_prompt("Updated system prompt")
            .build_standalone(client, tools, store)
            .await;

        // Check that the system prompt was UPDATED
        let messages = agent.session().messages();
        assert!(!messages.is_empty(), "Session should have messages");

        match &messages[0] {
            Message::System(sys) => {
                assert_eq!(
                    sys.content, "Updated system prompt",
                    "System prompt should be updated when resuming with a new prompt"
                );
            }
            other => panic!("First message should be System, got: {other:?}"),
        }

        // User message should still be preserved
        assert!(messages.len() >= 2, "Should have system + user messages");
        match &messages[1] {
            Message::User(user) => {
                assert_eq!(user.text_content(), "Hello");
            }
            other => panic!("Second message should be User, got: {other:?}"),
        }
    }

    /// Regression test: Resumed sessions without explicit system_prompt should keep their original
    #[tokio::test]
    async fn test_builder_preserves_existing_system_prompt_on_resume() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        // Create a session with an existing system prompt
        let mut existing_session = Session::new();
        existing_session.set_system_prompt("Original system prompt".to_string());

        // Resume WITHOUT specifying a new system prompt
        let agent = AgentBuilder::new()
            .resume_session(existing_session)
            // Note: no .system_prompt() call
            .build_standalone(client, tools, store)
            .await;

        // Original system prompt should be preserved
        let messages = agent.session().messages();
        match &messages[0] {
            Message::System(sys) => {
                assert_eq!(
                    sys.content, "Original system prompt",
                    "Original system prompt should be preserved when not overridden"
                );
            }
            other => panic!("First message should be System, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_backed_builder_requires_explicit_visibility_owner() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let result = AgentBuilder::new()
            .with_epoch_cursor_state(Arc::new(crate::runtime_epoch::EpochCursorState::new()))
            .build_inner(client, tools, store)
            .await;

        assert!(matches!(
            result,
            Err(AgentBuildPolicyError::MissingToolVisibilityOwner)
        ));
    }

    #[tokio::test]
    async fn runtime_backed_builder_restore_failure_returns_error_and_keeps_owner_state() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);
        let mut session = Session::new();
        session
            .set_tool_visibility_state(SessionToolVisibilityState {
                active_filter: ToolFilter::Deny(["secret".to_string()].into_iter().collect()),
                staged_filter: ToolFilter::Deny(["secret".to_string()].into_iter().collect()),
                active_revision: 7,
                staged_revision: 7,
                ..Default::default()
            })
            .expect("visibility state should serialize");
        let owner = Arc::new(RestoreFailingVisibilityOwner::new());
        let owner_trait: Arc<dyn ToolVisibilityOwner> = owner.clone();

        let result = AgentBuilder::new()
            .resume_session(session)
            .with_epoch_cursor_state(Arc::new(crate::runtime_epoch::EpochCursorState::new()))
            .with_tool_visibility_owner(owner_trait)
            .build_inner(client, tools, store)
            .await;

        match result {
            Err(AgentBuildPolicyError::ToolVisibilityRestore { message }) => {
                assert!(
                    message.contains("restore fixture rejected"),
                    "restore error should preserve owner failure details: {message}"
                );
            }
            Ok(_) => panic!("runtime-backed builder must fail when visibility restore fails"),
            Err(err) => panic!("unexpected build error: {err}"),
        }
        assert_eq!(
            owner.visibility_state().unwrap(),
            SessionToolVisibilityState::default(),
            "failed runtime restore must leave the owner state unchanged"
        );
        assert_eq!(
            owner.replace_calls(),
            1,
            "builder should attempt the runtime restore once, then fail closed"
        );
    }

    #[tokio::test]
    async fn runtime_backed_builder_rejects_malformed_canonical_visibility_state() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);
        let mut session = Session::new();
        session.set_metadata(
            SESSION_TOOL_VISIBILITY_STATE_KEY,
            serde_json::json!("not-a-visibility-state"),
        );
        let original_state = SessionToolVisibilityState {
            active_filter: ToolFilter::Deny(["secret".to_string()].into_iter().collect()),
            staged_filter: ToolFilter::Deny(["secret".to_string()].into_iter().collect()),
            active_revision: 3,
            staged_revision: 3,
            ..Default::default()
        };
        let owner = Arc::new(LocalToolVisibilityOwner::new());
        owner
            .replace_visibility_state(original_state.clone())
            .expect("test owner should accept initial state");
        let owner_trait: Arc<dyn ToolVisibilityOwner> = owner.clone();

        let result = AgentBuilder::new()
            .resume_session(session)
            .with_epoch_cursor_state(Arc::new(crate::runtime_epoch::EpochCursorState::new()))
            .with_tool_visibility_owner(owner_trait)
            .build_inner(client, tools, store)
            .await;

        match result {
            Err(AgentBuildPolicyError::ToolVisibilityRestore { message }) => {
                assert!(
                    message.contains("failed to decode canonical session metadata"),
                    "restore error should identify malformed canonical metadata: {message}"
                );
            }
            Ok(_) => panic!("runtime-backed builder must reject malformed canonical visibility"),
            Err(err) => panic!("unexpected build error: {err}"),
        }
        assert_eq!(
            owner.visibility_state().unwrap(),
            original_state,
            "failed malformed restore must not install default visibility"
        );
    }

    #[tokio::test]
    async fn runtime_backed_builder_ignores_legacy_local_visibility_metadata() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);
        let mut session = Session::new();
        session.set_metadata(
            EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect("legacy filter should serialize"),
        );
        let owner = Arc::new(LocalToolVisibilityOwner::new());
        let owner_trait: Arc<dyn ToolVisibilityOwner> = owner.clone();

        let result = AgentBuilder::new()
            .resume_session(session)
            .with_epoch_cursor_state(Arc::new(crate::runtime_epoch::EpochCursorState::new()))
            .with_tool_visibility_owner(owner_trait)
            .build_inner(client, tools, store)
            .await;

        assert!(result.is_ok());
        let state = owner.visibility_state().unwrap();
        assert_eq!(state.active_filter, ToolFilter::All);
        assert_eq!(state.staged_filter, ToolFilter::All);
    }

    #[tokio::test]
    async fn runtime_backed_builder_rejects_legacy_inherited_visibility_metadata() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);
        let mut session = Session::new();
        session.set_metadata(
            INHERITED_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect("legacy inherited filter should serialize"),
        );
        let owner = Arc::new(LocalToolVisibilityOwner::new());
        let owner_trait: Arc<dyn ToolVisibilityOwner> = owner.clone();

        let result = AgentBuilder::new()
            .resume_session(session)
            .with_epoch_cursor_state(Arc::new(crate::runtime_epoch::EpochCursorState::new()))
            .with_tool_visibility_owner(owner_trait)
            .build_inner(client, tools, store)
            .await;

        assert!(matches!(
            result,
            Err(AgentBuildPolicyError::LegacyInheritedToolFilterMetadata)
        ));
        assert_eq!(
            owner.visibility_state().unwrap(),
            SessionToolVisibilityState::default(),
            "failed legacy inherited metadata restore must leave owner visibility unchanged"
        );
    }

    #[tokio::test]
    async fn runtime_backed_builder_rejects_name_only_canonical_inherited_visibility_state() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);
        let mut session = Session::new();
        session
            .set_tool_visibility_state(SessionToolVisibilityState {
                inherited_base_filter: ToolFilter::Allow(
                    ["secret".to_string()].into_iter().collect(),
                ),
                ..Default::default()
            })
            .expect("visibility state should serialize");
        let owner = Arc::new(LocalToolVisibilityOwner::new());
        let owner_trait: Arc<dyn ToolVisibilityOwner> = owner.clone();

        let result = AgentBuilder::new()
            .resume_session(session)
            .with_epoch_cursor_state(Arc::new(crate::runtime_epoch::EpochCursorState::new()))
            .with_tool_visibility_owner(owner_trait)
            .build_inner(client, tools, store)
            .await;

        assert!(matches!(
            result,
            Err(AgentBuildPolicyError::MissingInheritedToolVisibilityWitnesses)
        ));
        assert_eq!(
            owner.visibility_state().unwrap(),
            SessionToolVisibilityState::default(),
            "failed name-only inherited restore must leave owner visibility unchanged"
        );
    }

    #[tokio::test]
    async fn runtime_backed_builder_restores_canonical_inherited_visibility_state() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);
        let inherited_filter = ToolFilter::Deny(["secret".to_string()].into_iter().collect());
        let mut session = Session::new();
        session
            .set_tool_visibility_state(SessionToolVisibilityState {
                inherited_base_filter: inherited_filter.clone(),
                filter_witnesses: [(
                    "secret".to_string(),
                    crate::ToolVisibilityWitness {
                        stable_owner_key: Some("test-owner:secret".to_string()),
                        last_seen_provenance: None,
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            })
            .expect("visibility state should serialize");
        let owner = Arc::new(LocalToolVisibilityOwner::new());
        let owner_trait: Arc<dyn ToolVisibilityOwner> = owner.clone();

        let result = AgentBuilder::new()
            .resume_session(session)
            .with_epoch_cursor_state(Arc::new(crate::runtime_epoch::EpochCursorState::new()))
            .with_tool_visibility_owner(owner_trait)
            .build_inner(client, tools, store)
            .await;

        assert!(result.is_ok());
        assert_eq!(
            owner.visibility_state().unwrap().inherited_base_filter,
            inherited_filter,
            "canonical inherited metadata should restore through the visibility owner"
        );
    }

    #[tokio::test]
    async fn runtime_backed_builder_does_not_seed_execution_kind() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let agent = AgentBuilder::new()
            .with_turn_state_handle(Arc::new(
                crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            ))
            .with_tool_visibility_owner(explicit_test_visibility_owner())
            .require_runtime_execution_kind_stamp()
            .build_standalone(client, tools, store)
            .await;

        assert_eq!(agent.runtime_execution_kind, None);
        assert!(agent.runtime_execution_kind_required);
    }

    #[tokio::test]
    async fn runtime_backed_run_rejects_missing_execution_kind() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(Arc::new(
                crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            ))
            .with_tool_visibility_owner(explicit_test_visibility_owner())
            .require_runtime_execution_kind_stamp()
            .build_standalone(client, tools, store)
            .await;

        let err = agent
            .run("hello".to_string().into())
            .await
            .expect_err("runtime-backed runs must be stamped before execution");

        assert!(
            err.to_string().contains("runtime_execution_kind not set"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn runtime_backed_run_pending_rejects_missing_execution_kind() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("resume me".to_string())));

        let mut agent = AgentBuilder::new()
            .resume_session(session)
            .with_turn_state_handle(Arc::new(
                crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            ))
            .with_tool_visibility_owner(explicit_test_visibility_owner())
            .require_runtime_execution_kind_stamp()
            .build_standalone(client, tools, store)
            .await;

        let err = agent
            .run_pending()
            .await
            .expect_err("runtime-backed pending runs must be stamped before execution");

        assert!(
            err.to_string().contains("runtime_execution_kind not set"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn runtime_backed_run_consumes_execution_kind_stamp() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(Arc::new(
                crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            ))
            .with_tool_visibility_owner(explicit_test_visibility_owner())
            .require_runtime_execution_kind_stamp()
            .build_standalone(client, tools, store)
            .await;
        agent.set_runtime_execution_kind(Some(crate::lifecycle::RuntimeExecutionKind::ContentTurn));

        agent
            .run("hello".to_string().into())
            .await
            .expect("stamped runtime-backed run should succeed");

        let err = agent
            .run("raw follow-up".to_string().into())
            .await
            .expect_err("runtime-backed follow-up run must require a fresh stamp");

        assert!(
            err.to_string().contains("runtime_execution_kind not set"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn runtime_backed_cancel_consumes_execution_kind_stamp() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(Arc::new(
                crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            ))
            .with_tool_visibility_owner(explicit_test_visibility_owner())
            .require_runtime_execution_kind_stamp()
            .build_standalone(client, tools, store)
            .await;
        agent.set_runtime_execution_kind(Some(crate::lifecycle::RuntimeExecutionKind::ContentTurn));

        agent.cancel();

        let err = agent
            .run("raw follow-up".to_string().into())
            .await
            .expect_err("runtime-backed follow-up run must require a fresh stamp after cancel");

        assert!(
            err.to_string().contains("runtime_execution_kind not set"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn test_builder_event_tap_receives_turn_started_without_primary_event_tx() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let tap = crate::event_tap::new_event_tap();
        let (tap_tx, mut tap_rx) = mpsc::channel(128);
        {
            let mut guard = tap.lock();
            *guard = Some(EventTapState {
                tx: tap_tx,
                truncated: AtomicBool::new(false),
            });
        }

        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(Arc::new(
                crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            ))
            .with_event_tap(tap)
            .build_standalone(client, tools, store)
            .await;
        agent.set_runtime_execution_kind(Some(crate::lifecycle::RuntimeExecutionKind::ContentTurn));

        let result = agent.run("hello".to_string().into()).await;
        assert!(result.is_ok());

        let mut saw_turn_started = false;
        while let Ok(event) = tap_rx.try_recv() {
            if matches!(event, AgentEvent::TurnStarted { .. }) {
                saw_turn_started = true;
                break;
            }
        }
        assert!(
            saw_turn_started,
            "tap should receive TurnStarted even without primary event channel"
        );
    }

    /// Regression: agent builder must seed applied_cursor from the feed's
    /// current watermark, not from 0. Starting from 0 replays every retained
    /// completion as new after stop/resume or live reattachment.
    #[tokio::test]
    async fn test_builder_seeds_applied_cursor_from_feed_watermark() {
        use crate::completion_feed::tests::MockCompletionFeed;

        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        // Feed already has activity at watermark 42.
        let feed = Arc::new(MockCompletionFeed::with_watermark(42));

        let agent = AgentBuilder::new()
            .with_completion_feed(feed)
            .build_standalone(client, tools, store)
            .await;

        assert_eq!(
            agent.applied_cursor, 42,
            "applied_cursor must seed from feed watermark, not 0"
        );
    }

    /// Regression: without a feed, applied_cursor must be 0.
    #[tokio::test]
    async fn test_builder_applied_cursor_zero_without_feed() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let agent = AgentBuilder::new()
            .build_standalone(client, tools, store)
            .await;

        assert_eq!(agent.applied_cursor, 0);
    }
}
