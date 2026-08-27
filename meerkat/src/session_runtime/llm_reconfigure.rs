//! Surface-agnostic LLM hot-swap support.
//!
//! Hosts the [`SessionRuntimeLlmReconfigureHost`] struct + its
//! [`SessionLlmReconfigureHost`] implementation. The generated runtime
//! adapter owns the hot-swap transition for idle, attached, and running
//! sessions. The cross-surface
//! `meerkat-rpc::SessionRuntime::hot_swap_llm_client` thin wrapper stays
//! in `meerkat-rpc` because it adapts the RPC `TurnOverrides` struct onto
//! [`SessionLlmReconfigureRequest`] and translates the `RuntimeDriverError`
//! into `RpcError`; this module is the surface-agnostic core it
//! delegates to.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]

use std::sync::Arc;

use crate::LlmClient;
use meerkat_core::error::AgentError;
use meerkat_core::handles::GeneratedAuthLeaseHandle;
use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
use meerkat_core::service::SessionError;
use meerkat_core::types::SessionId;
use meerkat_core::{
    AgentLlmClient, AgentLlmClientDecorator, Config, ConfigRuntime, ModelRegistry,
    SessionLlmIdentity, SessionToolVisibilityState,
};
use meerkat_runtime::{
    HydratedSessionLlmState, ResolvedSessionLlmReconfigure, RuntimeDriverError,
    SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus, SessionLlmReconfigureHost,
    SessionLlmReconfigureRequest,
};
use meerkat_session::{EphemeralSessionService, PersistentSessionService};

use crate::StagedSessionRegistry;
use crate::factory::AgentFactory;
use crate::service_factory::FactoryAgentBuilder;
use crate::session_runtime::recovery::parse_provider_override;

/// Convert a session-service error into the runtime-driver error shape
/// expected by [`SessionLlmReconfigureHost`] callers.
pub fn session_error_to_runtime_driver(err: SessionError) -> RuntimeDriverError {
    match err {
        SessionError::NotFound { .. } => RuntimeDriverError::NotReady {
            state: meerkat_runtime::RuntimeState::Destroyed,
        },
        other => RuntimeDriverError::Internal(other.to_string()),
    }
}

/// Convert a runtime-driver error back into a session-service error.
pub fn runtime_driver_error_to_session_error(err: RuntimeDriverError) -> SessionError {
    SessionError::Agent(AgentError::InternalError(err.to_string()))
}

/// Resolve a model profile into the typed capability surface a session
/// LLM identity carries through reconfigurations.
pub fn profile_to_capability_surface(
    profile: &meerkat_core::model_profile::ModelProfile,
) -> SessionLlmCapabilitySurface {
    SessionLlmCapabilitySurface {
        supports_temperature: profile.supports_temperature,
        supports_thinking: profile.supports_thinking,
        supports_reasoning: profile.supports_reasoning,
        inline_video: profile.inline_video,
        vision: profile.vision,
        image_input: profile.image_input,
        image_tool_results: profile.image_tool_results,
        supports_web_search: profile.supports_web_search,
        supports_mid_conversation_system_messages: profile
            .supports_mid_conversation_system_messages,
        image_generation: profile.image_generation,
        realtime: profile.realtime,
        call_timeout_secs: profile.call_timeout_secs,
    }
}

/// Validate that the registered model entry for `(provider, model)` is
/// consistent with the request override; returns a human-readable
/// rejection reason on mismatch.
pub fn registered_model_provider_mismatch_reason(
    registry: &ModelRegistry,
    provider: meerkat_core::Provider,
    model: &str,
) -> Option<String> {
    registry.provider_override_mismatch_reason(provider, model)
}

/// Adapt the runtime request's wire-facing provider string onto the core-owned
/// session identity resolver. Model/provider ownership, self-hosted alias
/// resolution, metadata tri-state, and stale-auth clearing remain singular in
/// `meerkat_core::resolve_session_llm_identity_override`.
fn resolve_reconfigure_target_llm_identity(
    registry: &ModelRegistry,
    current: &SessionLlmIdentity,
    request: &SessionLlmReconfigureRequest,
) -> Result<SessionLlmIdentity, RuntimeDriverError> {
    let provider = request
        .provider
        .as_deref()
        .map(parse_provider_override)
        .transpose()
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

    meerkat_core::resolve_session_llm_identity_override(
        current,
        registry,
        meerkat_core::SessionLlmIdentityOverride {
            model: request.model.as_deref(),
            provider,
            self_hosted_server_id: request.self_hosted_server_id.as_deref(),
            provider_params: request
                .provider_params
                .as_ref()
                .map(TurnMetadataOverride::as_ref),
            auth_binding: request
                .auth_binding
                .as_ref()
                .map(TurnMetadataOverride::as_ref),
        },
    )
    .map_err(|error| RuntimeDriverError::ValidationFailed {
        reason: error.to_string(),
    })
}

fn preserve_credential_account_affinity(
    config: &Config,
    current: &SessionLlmIdentity,
    request: &SessionLlmReconfigureRequest,
    target: &mut SessionLlmIdentity,
) -> Result<(), RuntimeDriverError> {
    if request.auth_binding.is_some()
        || target.auth_binding.is_some()
        || target.provider == current.provider
    {
        return Ok(());
    }
    let Some(meerkat_core::AuthCredentialIdentity::Account(account)) =
        AgentFactory::credential_identity_for_llm_identity(config, current).map_err(|error| {
            RuntimeDriverError::ValidationFailed {
                reason: error.to_string(),
            }
        })?
    else {
        return Ok(());
    };
    let route = meerkat_core::resolve_credential_account_binding_for_provider(
        config,
        target.provider,
        &account,
    )
    .map_err(|error| RuntimeDriverError::ValidationFailed {
        reason: error.to_string(),
    })?
    .ok_or_else(|| RuntimeDriverError::ValidationFailed {
        reason: format!(
            "provider switch to '{}' has no route sharing credential account '{}:{}'; set auth_binding explicitly to change accounts",
            target.provider.as_str(),
            account.realm,
            account.account
        ),
    })?;
    target.auth_binding = Some(route.auth_binding);
    Ok(())
}

/// Live-session operations required by the runtime-owned LLM reconfigure
/// transaction.
///
/// Keeping this as a surface-agnostic service capability lets embedded hosts
/// install the same canonical reconfigure host for persistent and ephemeral
/// session services. Persistence remains owned by the concrete service:
/// persistent sessions checkpoint the new identity, while ephemeral sessions
/// intentionally complete that phase as a no-op.
#[async_trait::async_trait]
pub trait SessionRuntimeLlmReconfigureService: Send + Sync {
    /// Acquire the stable outer boundary that serializes live identity changes
    /// with runtime-turn finalization for this exact session.
    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>;

    async fn live_llm_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionLlmIdentity, SessionError>;

    /// Whether the live ordered transcript contains typed instruction
    /// activation rows whose placement the target model must preserve.
    async fn live_session_has_instruction_activations(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError>;

    /// Return the exact realm that owned the live session's initial build.
    ///
    /// Hot-swap credential resolution must begin from this per-session realm,
    /// not from the service-wide config head, because one runtime can host
    /// sessions (notably mob members) from several child realms.
    async fn live_realm_id(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::RealmId>, SessionError>;

    async fn live_tool_visibility_state(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionToolVisibilityState>, SessionError>;

    async fn live_web_search_override(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::ToolCategoryOverride, SessionError>;

    async fn live_tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError>;

    async fn apply_live_llm_identity_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
        client: Arc<dyn AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError>;

    async fn apply_live_tool_visibility_state_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
        state: Option<SessionToolVisibilityState>,
    ) -> Result<(), SessionError>;

    async fn persist_live_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError>;

    async fn discard_live_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError>;
}

async fn preferred_hot_swap_realm(
    service: &dyn SessionRuntimeLlmReconfigureService,
    session_id: &SessionId,
    fallback_realm: Option<meerkat_core::RealmId>,
) -> Result<Option<meerkat_core::RealmId>, RuntimeDriverError> {
    Ok(service
        .live_realm_id(session_id)
        .await
        .map_err(session_error_to_runtime_driver)?
        .or(fallback_realm))
}

#[async_trait::async_trait]
impl SessionRuntimeLlmReconfigureService for PersistentSessionService<FactoryAgentBuilder> {
    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        Ok(Box::new(
            PersistentSessionService::<FactoryAgentBuilder>::acquire_runtime_turn_finalization_guard(
                self,
                session_id,
            )
            .await,
        ))
    }

    async fn live_llm_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionLlmIdentity, SessionError> {
        self.live_session_llm_identity(session_id).await
    }

    async fn live_session_has_instruction_activations(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(self
            .export_live_session(session_id)
            .await?
            .messages()
            .iter()
            .any(|message| {
                matches!(
                    message,
                    meerkat_core::Message::System(system)
                        if system.instruction_activation.is_some()
                )
            }))
    }

    async fn live_realm_id(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::RealmId>, SessionError> {
        Ok(self
            .export_live_session(session_id)
            .await?
            .session_metadata()
            .and_then(|metadata| metadata.realm_id))
    }

    async fn live_tool_visibility_state(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionToolVisibilityState>, SessionError> {
        self.export_live_session(session_id)
            .await?
            .try_tool_visibility_state()
            .map_err(|error| {
                SessionError::Agent(AgentError::InternalError(format!(
                    "invalid canonical tool visibility state: {error}"
                )))
            })
    }

    async fn live_web_search_override(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::ToolCategoryOverride, SessionError> {
        Ok(self
            .export_live_session(session_id)
            .await?
            .session_metadata()
            .map(|metadata| metadata.tooling.web_search)
            .unwrap_or(meerkat_core::ToolCategoryOverride::Inherit))
    }

    async fn live_tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
        self.tool_scope_snapshot(session_id).await
    }

    async fn apply_live_llm_identity_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
        client: Arc<dyn AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        self.apply_runtime_session_llm_identity_under_runtime_turn_boundary(
            session_id,
            client,
            identity,
            request_policy,
        )
        .await
    }

    async fn apply_live_tool_visibility_state_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
        state: Option<SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        self.apply_runtime_session_tool_visibility_state_under_runtime_turn_boundary(
            session_id, state,
        )
        .await
    }

    async fn persist_live_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.persist_live_session_now_under_runtime_turn_boundary(session_id)
            .await
            .map(|_| ())
    }

    async fn discard_live_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.discard_live_session_under_runtime_turn_boundary(session_id)
            .await
    }
}

#[async_trait::async_trait]
impl SessionRuntimeLlmReconfigureService for EphemeralSessionService<FactoryAgentBuilder> {
    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        Ok(Box::new(
            EphemeralSessionService::<FactoryAgentBuilder>::acquire_runtime_turn_finalization_guard(
                self,
                session_id,
            )
            .await,
        ))
    }

    async fn live_llm_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionLlmIdentity, SessionError> {
        self.live_session_llm_identity(session_id).await
    }

    async fn live_session_has_instruction_activations(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        Ok(self
            .export_session(session_id)
            .await?
            .messages()
            .iter()
            .any(|message| {
                matches!(
                    message,
                    meerkat_core::Message::System(system)
                        if system.instruction_activation.is_some()
                )
            }))
    }

    async fn live_realm_id(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::RealmId>, SessionError> {
        Ok(self
            .export_session(session_id)
            .await?
            .session_metadata()
            .and_then(|metadata| metadata.realm_id))
    }

    async fn live_tool_visibility_state(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionToolVisibilityState>, SessionError> {
        self.export_session(session_id)
            .await?
            .try_tool_visibility_state()
            .map_err(|error| {
                SessionError::Agent(AgentError::InternalError(format!(
                    "invalid canonical tool visibility state: {error}"
                )))
            })
    }

    async fn live_web_search_override(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::ToolCategoryOverride, SessionError> {
        Ok(self
            .export_session(session_id)
            .await?
            .session_metadata()
            .map(|metadata| metadata.tooling.web_search)
            .unwrap_or(meerkat_core::ToolCategoryOverride::Inherit))
    }

    async fn live_tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
        self.tool_scope_snapshot(session_id).await
    }

    async fn apply_live_llm_identity_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
        client: Arc<dyn AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        self.apply_runtime_session_llm_identity_under_runtime_turn_boundary(
            session_id,
            client,
            identity,
            request_policy,
        )
        .await
    }

    async fn apply_live_tool_visibility_state_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
        state: Option<SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        self.apply_runtime_session_tool_visibility_state_under_runtime_turn_boundary(
            session_id, state,
        )
        .await
    }

    async fn persist_live_under_runtime_turn_boundary(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), SessionError> {
        Ok(())
    }

    async fn discard_live_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        self.discard_live_session(session_id).await
    }
}

/// Captured construction inputs for installing the canonical runtime LLM
/// reconfigure host after a concrete session service has taken ownership of
/// its [`FactoryAgentBuilder`].
///
/// Embedded hosts create this blueprint immediately before moving the builder
/// into a persistent or ephemeral service, then call [`Self::install`] with
/// that concrete service. This keeps adapter/config/auth wiring in Meerkat
/// instead of duplicating it across MobKit and desktop surfaces.
pub struct SessionRuntimeLlmReconfigureHostBlueprint {
    factory: AgentFactory,
    config_store: Arc<dyn meerkat_core::ConfigStore>,
    config_state_path: std::path::PathBuf,
    default_llm_client: Arc<std::sync::RwLock<Option<Arc<dyn LlmClient>>>>,
    agent_llm_client_decorator: Arc<std::sync::RwLock<Option<AgentLlmClientDecorator>>>,
    realm_inheritance: Arc<std::sync::RwLock<Option<crate::RealmInheritance>>>,
}

impl SessionRuntimeLlmReconfigureHostBlueprint {
    pub fn new(
        builder: &FactoryAgentBuilder,
        config_state_path: std::path::PathBuf,
        default_llm_client: Arc<std::sync::RwLock<Option<Arc<dyn LlmClient>>>>,
    ) -> Self {
        Self {
            factory: builder.factory().clone(),
            config_store: builder.runtime_config_store(),
            config_state_path,
            default_llm_client,
            agent_llm_client_decorator: Arc::clone(&builder.default_agent_llm_client_decorator),
            realm_inheritance: Arc::clone(&builder.realm_inheritance),
        }
    }

    fn config_runtime(&self) -> Arc<ConfigRuntime> {
        Arc::new(ConfigRuntime::new(
            Arc::clone(&self.config_store),
            self.config_state_path.clone(),
        ))
    }

    pub fn install(
        self,
        runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
        service: Arc<dyn SessionRuntimeLlmReconfigureService>,
    ) {
        let config_runtime = self.config_runtime();
        runtime_adapter.set_session_llm_reconfigure_host(Arc::new(
            SessionRuntimeLlmReconfigureHost {
                service,
                staged_sessions: Arc::new(StagedSessionRegistry::new()),
                factory: self.factory,
                auth_lease: runtime_adapter.generated_auth_lease_handle(),
                default_llm_client: self.default_llm_client,
                agent_llm_client_decorator: self.agent_llm_client_decorator,
                config_runtime: Arc::new(std::sync::RwLock::new(Some(config_runtime))),
                realm_inheritance: self.realm_inheritance,
            },
        ));
    }
}

/// Surface-agnostic implementation of [`SessionLlmReconfigureHost`].
///
/// Surfaces construct one of these per-call (RPC, REST, MCP, …) so the
/// generated runtime-adapter reconfigure path can hydrate the live session, resolve target
/// identities, build adapters, and apply the swap without depending on
/// any RPC-specific wire shape.
pub struct SessionRuntimeLlmReconfigureHost {
    /// Live session service. Both persistent and ephemeral embedded runtimes
    /// implement the same reconfigure transaction contract.
    pub service: Arc<dyn SessionRuntimeLlmReconfigureService>,
    /// Staged session registry; consulted when the live session is
    /// missing but a staged identity is available.
    pub staged_sessions: Arc<StagedSessionRegistry>,
    /// Agent factory used to build LLM clients/adapters.
    pub factory: AgentFactory,
    /// Auth lease handle threaded into freshly-built clients.
    pub auth_lease: GeneratedAuthLeaseHandle,
    /// Override LLM client (test injection slot).
    pub default_llm_client: Arc<std::sync::RwLock<Option<Arc<dyn LlmClient>>>>,
    /// Default decorator applied to every freshly-built client.
    pub agent_llm_client_decorator: Arc<std::sync::RwLock<Option<AgentLlmClientDecorator>>>,
    /// Optional config runtime for resolving the model registry.
    pub config_runtime: Arc<std::sync::RwLock<Option<Arc<ConfigRuntime>>>>,
    /// Realm parent-chain inheritance (the same shared slot the builder reads).
    /// When populated, the hot-swap / reconfigure path composes the active realm
    /// chain over the raw head config so an inherited (global-owned) credential
    /// binding and self-hosted/provider capabilities resolve on a model swap —
    /// matching the initial agent build. Empty slot => no composition.
    pub realm_inheritance: Arc<std::sync::RwLock<Option<crate::RealmInheritance>>>,
}

impl SessionRuntimeLlmReconfigureHost {
    async fn capability_surface_for_identity(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<
        (
            Option<SessionLlmCapabilitySurface>,
            SessionLlmCapabilitySurfaceStatus,
        ),
        RuntimeDriverError,
    > {
        let registry = self.model_registry().await?;
        Ok(
            match registry.profile_for_provider(identity.provider, &identity.model) {
                Some(profile) => (
                    Some(profile_to_capability_surface(&profile)),
                    SessionLlmCapabilitySurfaceStatus::Resolved,
                ),
                None => (None, SessionLlmCapabilitySurfaceStatus::Unresolved),
            },
        )
    }

    async fn hydrate_staged_session_llm_state(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<HydratedSessionLlmState>, RuntimeDriverError> {
        let Some(current_identity) = self
            .staged_sessions
            .effective_llm_identity(session_id)
            .await
            .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?
        else {
            return Ok(None);
        };
        let (current_capability_surface, capability_surface_status) = self
            .capability_surface_for_identity(&current_identity)
            .await?;
        Ok(Some(HydratedSessionLlmState {
            current_identity,
            current_visibility_state: Default::default(),
            current_capability_surface,
            capability_surface_status,
            base_tool_names: std::collections::BTreeSet::new(),
        }))
    }

    async fn model_registry(&self) -> Result<ModelRegistry, RuntimeDriverError> {
        // Compose the realm chain so an inherited self-hosted/custom model entry
        // (e.g. defined in `global`) is visible to capability resolution on a
        // hot-swap, matching the agent build path.
        let config = self.load_config_for_hot_swap().await?;

        config
            .model_registry(meerkat_models::canonical())
            .map_err(|e| {
                RuntimeDriverError::Internal(format!("Failed to resolve model registry: {e}"))
            })
    }

    /// Build the per-identity LLM adapter used by the hot-swap and live
    /// orchestration flows. Public so surfaces (RPC, REST, …) can call
    /// it directly when they need to materialize an adapter outside the
    /// `SessionLlmReconfigureHost` trait surface.
    pub async fn build_adapter_for_llm_identity(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn AgentLlmClient>, RuntimeDriverError> {
        let preferred_realm = self.inheritance_head_realm();
        self.build_adapter_for_llm_identity_in_realm(identity, preferred_realm.as_ref())
            .await
    }

    async fn build_adapter_for_session_llm_identity(
        &self,
        session_id: &SessionId,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn AgentLlmClient>, RuntimeDriverError> {
        let preferred_realm = preferred_hot_swap_realm(
            self.service.as_ref(),
            session_id,
            self.inheritance_head_realm(),
        )
        .await?;
        self.build_adapter_for_llm_identity_in_realm(identity, preferred_realm.as_ref())
            .await
    }

    fn inheritance_head_realm(&self) -> Option<meerkat_core::RealmId> {
        self.realm_inheritance
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|inheritance| inheritance.head().clone())
    }

    async fn build_adapter_for_llm_identity_in_realm(
        &self,
        identity: &SessionLlmIdentity,
        preferred_realm: Option<&meerkat_core::RealmId>,
    ) -> Result<Arc<dyn AgentLlmClient>, RuntimeDriverError> {
        let default_llm_client = self
            .default_llm_client
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let raw_client = if let Some(default) = default_llm_client {
            default
        } else {
            let config = self.load_config_for_hot_swap().await?;
            self.factory
                .build_llm_client_for_identity_with_auth_lease_in_realm(
                    &config,
                    identity,
                    Some(self.auth_lease.clone()),
                    preferred_realm,
                )
                .await
                .map_err(|e| {
                    RuntimeDriverError::Internal(format!(
                        "Failed to build LLM client for session identity hot-swap: {e}"
                    ))
                })?
        };

        let adapter = self
            .factory
            .build_llm_adapter_for_identity(raw_client, identity)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "Failed to bind LLM client to session identity hot-swap: {error}"
                ))
            })?;
        let adapter = Arc::new(adapter) as Arc<dyn AgentLlmClient>;
        let decorator = self
            .agent_llm_client_decorator
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Ok(AgentFactory::decorate_agent_llm_client(
            adapter,
            decorator.as_ref(),
        ))
    }

    async fn load_config_for_hot_swap(&self) -> Result<Config, RuntimeDriverError> {
        let config_runtime = self
            .config_runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let head_config = if let Some(runtime) = config_runtime {
            runtime
                .get()
                .await
                .map(|snapshot| snapshot.config)
                .map_err(|e| {
                    RuntimeDriverError::Internal(format!("Failed to load config for hot-swap: {e}"))
                })?
        } else {
            Config::default()
        };

        // Compose the active realm chain over the head snapshot so a model
        // hot-swap resolves the same inherited (e.g. global-owned) credential
        // binding and self-hosted/provider capabilities as the initial agent
        // build. Without this the swap rebuilds the LLM client from the RAW head
        // config and an inherited binding yields no candidate. Fail-closed: a
        // compose error propagates rather than silently using the raw head.
        let inheritance = self
            .realm_inheritance
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(inheritance) = inheritance {
            return inheritance.compose_over(head_config).await.map_err(|e| {
                RuntimeDriverError::Internal(format!(
                    "Failed to compose realm config chain for hot-swap: {e}"
                ))
            });
        }
        Ok(head_config)
    }

    async fn build_request_policy_for_llm_identity(
        &self,
        session_id: &SessionId,
        identity: &SessionLlmIdentity,
    ) -> Result<meerkat_core::SessionLlmRequestPolicy, RuntimeDriverError> {
        let config = self.load_config_for_hot_swap().await?;
        // The session's persisted web-search disable intent
        // (`SessionMetadata.tooling.web_search`) must survive a model hot-swap —
        // otherwise reconfigure would silently re-enable the provider-native
        // web-search body that `--no-web-search` suppressed. Read it from the
        // live session metadata; fail closed to `Inherit` only when the metadata
        // is genuinely unavailable.
        let web_search = match self.service.live_web_search_override(session_id).await {
            Ok(web_search) => web_search,
            Err(_) => meerkat_core::ToolCategoryOverride::Inherit,
        };
        self.factory
            .request_policy_for_session_llm_identity(&config, identity, web_search, session_id)
            .map_err(|e| {
                RuntimeDriverError::Internal(format!(
                    "Failed to build LLM request policy for session {session_id} identity hot-swap: {e}"
                ))
            })
    }

    /// Resolve the target [`SessionLlmIdentity`] for a hot-swap request,
    /// validating provider/model overrides against the model registry.
    /// Public so surfaces that need to peek the resolved identity
    /// (e.g. live orchestration in W2-A) can call into it directly.
    pub async fn resolve_target_llm_identity(
        &self,
        current: &SessionLlmIdentity,
        request: &SessionLlmReconfigureRequest,
    ) -> Result<SessionLlmIdentity, RuntimeDriverError> {
        let registry = self.model_registry().await?;
        let mut target = resolve_reconfigure_target_llm_identity(&registry, current, request)?;
        let config = self.load_config_for_hot_swap().await?;
        preserve_credential_account_affinity(&config, current, request, &mut target)?;
        Ok(target)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SessionLlmReconfigureHost for SessionRuntimeLlmReconfigureHost {
    async fn acquire_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<
        Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>,
        RuntimeDriverError,
    > {
        self.service
            .acquire_runtime_turn_finalization_guard(session_id)
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn hydrate_session_llm_state(
        &self,
        session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
        let current_identity = match self.service.live_llm_identity(session_id).await {
            Ok(identity) => identity,
            Err(err) => {
                if let Some(hydrated) = self.hydrate_staged_session_llm_state(session_id).await? {
                    return Ok(hydrated);
                }
                return Err(session_error_to_runtime_driver(err));
            }
        };
        let current_visibility_state =
            match self.service.live_tool_visibility_state(session_id).await {
                Ok(state) => state.unwrap_or_default(),
                Err(err) => {
                    if let Some(hydrated) =
                        self.hydrate_staged_session_llm_state(session_id).await?
                    {
                        return Ok(hydrated);
                    }
                    return Err(session_error_to_runtime_driver(err));
                }
            };
        let base_tool_names = self
            .service
            .live_tool_scope_snapshot(session_id)
            .await
            .map_err(session_error_to_runtime_driver)?
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "session {session_id} missing live tool scope snapshot during llm reconfiguration"
                ))
            })?
            .known_base_names
            .into_iter()
            .collect();

        let (current_capability_surface, capability_surface_status) = self
            .capability_surface_for_identity(&current_identity)
            .await?;

        Ok(HydratedSessionLlmState {
            current_identity,
            current_visibility_state,
            current_capability_surface,
            capability_surface_status,
            base_tool_names,
        })
    }

    async fn resolve_target_session_llm_identity(
        &self,
        request: &SessionLlmReconfigureRequest,
        current_identity: &SessionLlmIdentity,
    ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
        let target_identity = self
            .resolve_target_llm_identity(current_identity, request)
            .await?;
        let registry = self.model_registry().await?;
        let profile = registry
            .profile_for_provider(target_identity.provider, &target_identity.model)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "no capability profile is registered for provider '{}' and model '{}'",
                    target_identity.provider.as_str(),
                    target_identity.model
                ),
            })?;

        Ok(ResolvedSessionLlmReconfigure {
            target_identity,
            target_capability_surface: profile_to_capability_surface(&profile),
        })
    }

    async fn apply_live_session_llm_identity(
        &self,
        session_id: &SessionId,
        identity: &SessionLlmIdentity,
        capability_surface: Option<&SessionLlmCapabilitySurface>,
    ) -> Result<(), RuntimeDriverError> {
        if self
            .service
            .live_session_has_instruction_activations(session_id)
            .await
            .map_err(session_error_to_runtime_driver)?
        {
            let supports_mid_conversation_system_messages = capability_surface
                .is_some_and(|surface| surface.supports_mid_conversation_system_messages);
            if !supports_mid_conversation_system_messages {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "model '{}' cannot represent the ordered instruction activations already recorded for session {session_id}",
                        identity.model
                    ),
                });
            }
        }
        let adapter = self
            .build_adapter_for_session_llm_identity(session_id, identity)
            .await?;
        let request_policy = self
            .build_request_policy_for_llm_identity(session_id, identity)
            .await?;
        self.service
            .apply_live_llm_identity_under_runtime_turn_boundary(
                session_id,
                adapter,
                identity.clone(),
                request_policy,
            )
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn apply_live_session_tool_visibility_state(
        &self,
        session_id: &SessionId,
        visibility_state: Option<SessionToolVisibilityState>,
    ) -> Result<(), RuntimeDriverError> {
        self.service
            .apply_live_tool_visibility_state_under_runtime_turn_boundary(
                session_id,
                visibility_state,
            )
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn persist_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .persist_live_under_runtime_turn_boundary(session_id)
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .discard_live_under_runtime_turn_boundary(session_id)
            .await
            .map_err(session_error_to_runtime_driver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{
        AuthBindingRef, BindingId, BindingOrigin, ConfigStore as _, Provider, RealmId,
    };

    struct RealmOnlyService {
        realm_id: Option<RealmId>,
    }

    #[async_trait::async_trait]
    impl SessionRuntimeLlmReconfigureService for RealmOnlyService {
        async fn acquire_runtime_turn_finalization_guard(
            &self,
            _session_id: &SessionId,
        ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
        {
            unreachable!("realm selection does not acquire the turn boundary")
        }

        async fn live_llm_identity(
            &self,
            _session_id: &SessionId,
        ) -> Result<SessionLlmIdentity, SessionError> {
            unreachable!("realm selection does not read the LLM identity")
        }

        async fn live_session_has_instruction_activations(
            &self,
            _session_id: &SessionId,
        ) -> Result<bool, SessionError> {
            unreachable!("realm selection does not inspect the transcript")
        }

        async fn live_realm_id(
            &self,
            _session_id: &SessionId,
        ) -> Result<Option<RealmId>, SessionError> {
            Ok(self.realm_id.clone())
        }

        async fn live_tool_visibility_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<Option<SessionToolVisibilityState>, SessionError> {
            unreachable!("realm selection does not read tool visibility")
        }

        async fn live_web_search_override(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_core::ToolCategoryOverride, SessionError> {
            unreachable!("realm selection does not read web-search policy")
        }

        async fn live_tool_scope_snapshot(
            &self,
            _session_id: &SessionId,
        ) -> Result<Option<meerkat_core::ToolScopeSnapshot>, SessionError> {
            unreachable!("realm selection does not read the tool scope")
        }

        async fn apply_live_llm_identity_under_runtime_turn_boundary(
            &self,
            _session_id: &SessionId,
            _client: Arc<dyn AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: meerkat_core::SessionLlmRequestPolicy,
        ) -> Result<(), SessionError> {
            unreachable!("realm selection does not mutate the LLM identity")
        }

        async fn apply_live_tool_visibility_state_under_runtime_turn_boundary(
            &self,
            _session_id: &SessionId,
            _state: Option<SessionToolVisibilityState>,
        ) -> Result<(), SessionError> {
            unreachable!("realm selection does not mutate tool visibility")
        }

        async fn persist_live_under_runtime_turn_boundary(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), SessionError> {
            unreachable!("realm selection does not persist")
        }

        async fn discard_live_under_runtime_turn_boundary(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), SessionError> {
            unreachable!("realm selection does not discard")
        }
    }

    fn anthropic_binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse("tenant_a").unwrap(),
            binding: BindingId::parse("anthropic_default").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        }
    }

    fn anthropic_identity() -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(anthropic_binding()),
        }
    }

    fn model_registry() -> ModelRegistry {
        ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())
            .expect("canonical model registry")
    }

    fn reconfigure_request(
        model: Option<&str>,
        provider: Option<&str>,
        auth_binding: Option<TurnMetadataOverride<AuthBindingRef>>,
    ) -> SessionLlmReconfigureRequest {
        SessionLlmReconfigureRequest {
            model: model.map(str::to_string),
            provider: provider.map(str::to_string),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding,
        }
    }

    #[tokio::test]
    async fn live_session_realm_overrides_runtime_head_for_hot_swap() {
        let session_realm = RealmId::parse("mob.project.member").unwrap();
        let runtime_head = RealmId::parse("project").unwrap();
        let service = RealmOnlyService {
            realm_id: Some(session_realm.clone()),
        };
        let selected =
            preferred_hot_swap_realm(&service, &SessionId::new(), Some(runtime_head.clone()))
                .await
                .expect("select preferred realm");
        assert_eq!(selected, Some(session_realm));

        let service = RealmOnlyService { realm_id: None };
        let selected =
            preferred_hot_swap_realm(&service, &SessionId::new(), Some(runtime_head.clone()))
                .await
                .expect("fall back to runtime head");
        assert_eq!(selected, Some(runtime_head));
    }

    #[tokio::test]
    async fn embedded_blueprint_reads_store_updates_after_construction() {
        let initial_snapshot = Config {
            max_tokens: Some(11),
            ..Config::default()
        };
        let config_at_construction = Config {
            max_tokens: Some(22),
            ..Config::default()
        };
        let live_store = Arc::new(meerkat_core::MemoryConfigStore::new(
            config_at_construction,
            meerkat_models::canonical(),
        ));
        let builder = FactoryAgentBuilder::new_with_config_store(
            AgentFactory::minimal(),
            initial_snapshot,
            live_store.clone(),
        );
        let temp = tempfile::tempdir().expect("temporary config-runtime state root");
        let blueprint = SessionRuntimeLlmReconfigureHostBlueprint::new(
            &builder,
            temp.path().join("config_state.json"),
            Arc::new(std::sync::RwLock::new(None)),
        );

        let updated = Config {
            max_tokens: Some(33),
            ..Config::default()
        };
        live_store
            .set(updated)
            .await
            .expect("update canonical config store after blueprint construction");

        let snapshot = blueprint
            .config_runtime()
            .get()
            .await
            .expect("blueprint config runtime reads canonical store");
        assert_eq!(snapshot.config.max_tokens, Some(33));
    }

    #[tokio::test]
    async fn embedded_blueprint_lowers_snapshot_only_builder_to_memory_store() {
        let snapshot = Config {
            max_tokens: Some(44),
            ..Config::default()
        };
        let builder = FactoryAgentBuilder::new(AgentFactory::minimal(), snapshot);
        let temp = tempfile::tempdir().expect("temporary config-runtime state root");
        let blueprint = SessionRuntimeLlmReconfigureHostBlueprint::new(
            &builder,
            temp.path().join("config_state.json"),
            Arc::new(std::sync::RwLock::new(None)),
        );

        let snapshot = blueprint
            .config_runtime()
            .get()
            .await
            .expect("snapshot-only blueprint config runtime");
        assert_eq!(snapshot.config.max_tokens, Some(44));
    }

    #[test]
    fn model_only_reconfigure_uses_catalog_provider_and_clears_stale_binding() {
        let current = anthropic_identity();
        let request = reconfigure_request(Some("gpt-5.5"), None, None);

        let resolved =
            resolve_reconfigure_target_llm_identity(&model_registry(), &current, &request)
                .expect("catalog-owned model-only switch");

        assert_eq!(resolved.model, "gpt-5.5");
        assert_eq!(resolved.provider, Provider::OpenAI);
        assert!(
            resolved.auth_binding.is_none(),
            "provider switches must not inherit a binding from the previous provider"
        );
    }

    #[test]
    fn provider_switch_preserves_shared_credential_account_route() {
        let account =
            meerkat_core::CredentialAccountId::parse("github_copilot").expect("valid account");
        let mut section = meerkat_core::RealmConfigSection::default();
        for (route, provider, backend_kind, auth_method) in [
            (
                "copilot_anthropic",
                Provider::Anthropic,
                "copilot",
                "github_copilot_oauth",
            ),
            (
                "copilot_openai",
                Provider::OpenAI,
                "copilot",
                "github_copilot_oauth",
            ),
        ] {
            section.backend.insert(
                route.to_string(),
                meerkat_core::BackendProfileConfig {
                    provider: provider.as_str().to_string(),
                    backend_kind: backend_kind.to_string(),
                    base_url: None,
                    options: serde_json::Value::Null,
                    server: None,
                },
            );
            section.auth.insert(
                route.to_string(),
                meerkat_core::AuthProfileConfig {
                    provider: provider.as_str().to_string(),
                    auth_method: auth_method.to_string(),
                    source: meerkat_core::CredentialSourceSpec::ManagedStore,
                    constraints: meerkat_core::AuthConstraints {
                        allow_interactive_login: true,
                        ..Default::default()
                    },
                    metadata_defaults: Default::default(),
                },
            );
            section.binding.insert(
                route.to_string(),
                meerkat_core::ProviderBindingConfig {
                    backend_profile: route.to_string(),
                    auth_profile: route.to_string(),
                    credential_account: Some(account.clone()),
                    default_model: None,
                    policy: Default::default(),
                    provider_default: false,
                },
            );
        }
        let mut config = Config::default();
        config.realm.insert("global".to_string(), section);
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(AuthBindingRef {
                realm: RealmId::global(),
                binding: BindingId::parse("copilot_anthropic").unwrap(),
                profile: None,
                origin: BindingOrigin::Configured,
            }),
        };
        let request = reconfigure_request(Some("gpt-5.5"), None, None);
        let mut target =
            resolve_reconfigure_target_llm_identity(&model_registry(), &current, &request)
                .expect("catalog provider switch");

        preserve_credential_account_affinity(&config, &current, &request, &mut target)
            .expect("shared account route");

        assert_eq!(
            target
                .auth_binding
                .as_ref()
                .map(|binding| binding.binding.as_str()),
            Some("copilot_openai")
        );
    }

    #[test]
    fn same_provider_without_explicit_binding_inherits_durable_binding() {
        let current = anthropic_identity();
        let request = reconfigure_request(Some("claude-opus-4-8"), None, None);

        let resolved =
            resolve_reconfigure_target_llm_identity(&model_registry(), &current, &request)
                .expect("same-provider model-only switch");

        assert_eq!(resolved.provider, Provider::Anthropic);
        assert_eq!(resolved.auth_binding, Some(anthropic_binding()));
    }

    #[test]
    fn explicit_clear_drops_binding_even_without_provider_change() {
        let current = anthropic_identity();
        let request = reconfigure_request(None, None, Some(TurnMetadataOverride::Clear));

        let resolved =
            resolve_reconfigure_target_llm_identity(&model_registry(), &current, &request)
                .expect("explicit auth clear");

        assert!(resolved.auth_binding.is_none());
    }

    #[test]
    fn explicit_set_overrides_binding_across_provider_change() {
        let current = anthropic_identity();
        let target = AuthBindingRef {
            realm: RealmId::parse("tenant_b").unwrap(),
            binding: BindingId::parse("openai_default").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        };
        let request = reconfigure_request(
            Some("gpt-5.5"),
            None,
            Some(TurnMetadataOverride::Set(target.clone())),
        );

        let resolved =
            resolve_reconfigure_target_llm_identity(&model_registry(), &current, &request)
                .expect("explicit auth binding on catalog-owned switch");

        assert_eq!(resolved.provider, Provider::OpenAI);
        assert_eq!(resolved.auth_binding, Some(target));
    }
}
