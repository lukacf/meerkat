//! Surface-agnostic LLM hot-swap support.
//!
//! Hosts the [`SessionRuntimeLlmReconfigureHost`] struct + its
//! [`SessionLlmReconfigureHost`] implementation, plus the `idle` hot-swap
//! flow ([`hot_swap_llm_client_on_idle_session`]) that surfaces drive when
//! the runtime adapter reports an idle session. The cross-surface
//! `meerkat-rpc::SessionRuntime::hot_swap_llm_client` thin wrapper stays
//! in `meerkat-rpc` because it adapts the RPC `TurnOverrides` struct onto
//! [`SessionLlmReconfigureRequest`] and translates the `RuntimeDriverError`
//! into `RpcError`; this module is the surface-agnostic core it
//! delegates to.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]

use std::sync::Arc;

use crate::LlmClient;
use meerkat_core::error::AgentError;
use meerkat_core::handles::AuthLeaseHandle;
use meerkat_core::service::{SessionError, SessionService};
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
use meerkat_session::PersistentSessionService;

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
    profile: &meerkat_models::profile::ModelProfile,
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

/// Surface-agnostic implementation of [`SessionLlmReconfigureHost`].
///
/// Surfaces construct one of these per-call (RPC, REST, MCP, …) so the
/// hot-swap helpers can hydrate the live session, resolve target
/// identities, build adapters, and apply the swap without depending on
/// any RPC-specific wire shape.
pub struct SessionRuntimeLlmReconfigureHost {
    /// Persistent session service.
    pub service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    /// Staged session registry; consulted when the live session is
    /// missing but a staged identity is available.
    pub staged_sessions: Arc<StagedSessionRegistry>,
    /// Agent factory used to build LLM clients/adapters.
    pub factory: AgentFactory,
    /// Auth lease handle threaded into freshly-built clients.
    pub auth_lease: Arc<dyn AuthLeaseHandle>,
    /// Override LLM client (test injection slot).
    pub default_llm_client: Arc<std::sync::RwLock<Option<Arc<dyn LlmClient>>>>,
    /// Default decorator applied to every freshly-built client.
    pub agent_llm_client_decorator: Arc<std::sync::RwLock<Option<AgentLlmClientDecorator>>>,
    /// Optional config runtime for resolving the model registry.
    pub config_runtime: Arc<std::sync::RwLock<Option<Arc<ConfigRuntime>>>>,
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
        let config_runtime = self
            .config_runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let config = if let Some(runtime) = config_runtime {
            runtime
                .get()
                .await
                .map(|snapshot| snapshot.config)
                .map_err(|e| RuntimeDriverError::Internal(format!("Failed to load config: {e}")))?
        } else {
            Config::default()
        };

        config.model_registry().map_err(|e| {
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
                .build_llm_client_for_identity_with_auth_lease(
                    &config,
                    identity,
                    Some(Arc::clone(&self.auth_lease)),
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
            .build_llm_adapter(raw_client, identity.model.clone())
            .await;
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
        if let Some(runtime) = config_runtime {
            runtime
                .get()
                .await
                .map(|snapshot| snapshot.config)
                .map_err(|e| {
                    RuntimeDriverError::Internal(format!("Failed to load config for hot-swap: {e}"))
                })
        } else {
            Ok(Config::default())
        }
    }

    async fn build_request_policy_for_llm_identity(
        &self,
        session_id: &SessionId,
        identity: &SessionLlmIdentity,
    ) -> Result<meerkat_core::SessionLlmRequestPolicy, RuntimeDriverError> {
        let config = self.load_config_for_hot_swap().await?;
        self.factory
            .request_policy_for_llm_identity(&config, identity)
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
        if request.provider.is_some() && request.model.is_none() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider override requires model on an existing session".to_string(),
            });
        }
        if request.clear_provider_params && request.provider_params.is_some() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "clear_provider_params cannot be combined with provider_params".to_string(),
            });
        }
        if request.clear_auth_binding && request.auth_binding.is_some() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "clear_auth_binding cannot be combined with auth_binding".to_string(),
            });
        }

        let registry = self.model_registry().await?;
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| current.model.clone());
        let provider = if let Some(provider_name) = request.provider.as_ref() {
            parse_provider_override(provider_name)
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
        } else {
            current.provider
        };
        if (request.model.is_some() || request.provider.is_some())
            && let Some(reason) =
                registered_model_provider_mismatch_reason(&registry, provider, &model)
        {
            return Err(RuntimeDriverError::ValidationFailed { reason });
        }
        let provider_params = if request.clear_provider_params {
            None
        } else {
            request
                .provider_params
                .clone()
                .or_else(|| current.provider_params.clone())
        };
        let self_hosted_server_id = if provider == meerkat_core::Provider::SelfHosted {
            if request.model.is_none() {
                current.self_hosted_server_id.clone().or_else(|| {
                    registry
                        .entry_for_provider(meerkat_core::Provider::SelfHosted, &model)
                        .and_then(|entry| entry.self_hosted.as_ref())
                        .map(|server| server.server_id.clone())
                })
            } else {
                match registry.entry_for_provider(meerkat_core::Provider::SelfHosted, &model) {
                    Some(entry) => entry
                        .self_hosted
                        .as_ref()
                        .map(|server| server.server_id.clone()),
                    None => {
                        return Err(RuntimeDriverError::ValidationFailed {
                            reason: format!(
                                "self-hosted provider requires a registered model alias; '{model}' is not configured"
                            ),
                        });
                    }
                }
            }
        } else {
            None
        };

        let auth_binding = if request.clear_auth_binding {
            None
        } else {
            request
                .auth_binding
                .clone()
                .or_else(|| current.auth_binding.clone())
        };

        Ok(SessionLlmIdentity {
            model,
            provider,
            self_hosted_server_id,
            provider_params,
            auth_binding,
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SessionLlmReconfigureHost for SessionRuntimeLlmReconfigureHost {
    async fn hydrate_session_llm_state(
        &self,
        session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
        let current_identity = match self.service.live_session_llm_identity(session_id).await {
            Ok(identity) => identity,
            Err(err) => {
                if let Some(hydrated) = self.hydrate_staged_session_llm_state(session_id).await? {
                    return Ok(hydrated);
                }
                return Err(session_error_to_runtime_driver(err));
            }
        };
        let session = match self.service.export_live_session(session_id).await {
            Ok(session) => session,
            Err(err) => {
                if let Some(hydrated) = self.hydrate_staged_session_llm_state(session_id).await? {
                    return Ok(hydrated);
                }
                return Err(session_error_to_runtime_driver(err));
            }
        };
        let current_visibility_state = session
            .try_tool_visibility_state()
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "invalid canonical tool visibility state: {err}"
                ))
            })?
            .unwrap_or_default();
        let base_tool_names = self
            .service
            .tool_scope_snapshot(session_id)
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
    ) -> Result<(), RuntimeDriverError> {
        let adapter = self.build_adapter_for_llm_identity(identity).await?;
        let request_policy = self
            .build_request_policy_for_llm_identity(session_id, identity)
            .await?;
        self.service
            .apply_runtime_session_llm_identity(
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
            .set_session_tool_visibility_state(session_id, visibility_state)
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn persist_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .persist_live_session_now(session_id)
            .await
            .map(|_| ())
            .map_err(session_error_to_runtime_driver)
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .discard_live_session(session_id)
            .await
            .map_err(session_error_to_runtime_driver)
    }
}

/// Pure: derive the new visibility state for a hot-swap given the
/// current state, target capability surface, and the base tool names.
/// Used by surfaces driving the idle hot-swap flow.
#[must_use]
pub fn derive_reconfigured_visibility_state(
    current: &SessionToolVisibilityState,
    target_capability_surface: &SessionLlmCapabilitySurface,
    base_tool_names: &std::collections::BTreeSet<String>,
) -> SessionToolVisibilityState {
    let current_view_image_visible =
        committed_visibility_allows(base_tool_names, current, meerkat_core::VIEW_IMAGE_TOOL_NAME);

    let mut next = current.clone();
    next.capability_base_filter = meerkat_core::capability_base_filter_for_image_tool_results(
        target_capability_surface.image_tool_results,
    );

    let next_view_image_visible =
        committed_visibility_allows(base_tool_names, &next, meerkat_core::VIEW_IMAGE_TOOL_NAME);
    if current_view_image_visible != next_view_image_visible {
        next.active_revision = current.active_revision.max(current.staged_revision) + 1;
    }

    next
}

fn committed_visibility_allows(
    base_tool_names: &std::collections::BTreeSet<String>,
    visibility_state: &SessionToolVisibilityState,
    tool_name: &str,
) -> bool {
    if !base_tool_names.contains(tool_name) {
        return false;
    }

    meerkat_core::ToolScope::compose(&[
        visibility_state.capability_base_filter.clone(),
        visibility_state.inherited_base_filter.clone(),
        visibility_state.active_filter.clone(),
    ])
    .allows(tool_name)
}

/// Roll back the live session to its prior identity + visibility state
/// after a failed idle hot-swap. Discards the live session if rollback
/// itself fails so the next request reattaches from durable state.
pub async fn rollback_idle_hot_swap_failure(
    host: &dyn SessionLlmReconfigureHost,
    session_id: &SessionId,
    previous_identity: &SessionLlmIdentity,
    previous_visibility_state: &SessionToolVisibilityState,
    original_error: RuntimeDriverError,
) -> Result<(), RuntimeDriverError> {
    let rollback_result = async {
        host.apply_live_session_llm_identity(session_id, previous_identity)
            .await?;
        host.apply_live_session_tool_visibility_state(
            session_id,
            Some(previous_visibility_state.clone()),
        )
        .await?;
        Ok::<(), RuntimeDriverError>(())
    }
    .await;

    match rollback_result {
        Ok(()) => Err(original_error),
        Err(rollback_error) => {
            let _ = host.discard_live_session(session_id).await;
            Err(RuntimeDriverError::Internal(format!(
                "failed to rollback idle live llm reconfiguration after error ({original_error}): {rollback_error}"
            )))
        }
    }
}

/// Idle-session hot-swap entry point. Drives hydrate → resolve →
/// derive-visibility → apply-identity → apply-visibility → persist,
/// rolling back to the previous identity on failure.
pub async fn hot_swap_llm_client_on_idle_session(
    host: &dyn SessionLlmReconfigureHost,
    session_id: &SessionId,
    request: &SessionLlmReconfigureRequest,
) -> Result<(), RuntimeDriverError> {
    let hydrated = host.hydrate_session_llm_state(session_id).await?;
    let resolved = host
        .resolve_target_session_llm_identity(request, &hydrated.current_identity)
        .await?;
    let next_visibility_state = derive_reconfigured_visibility_state(
        &hydrated.current_visibility_state,
        &resolved.target_capability_surface,
        &hydrated.base_tool_names,
    );

    host.apply_live_session_llm_identity(session_id, &resolved.target_identity)
        .await?;
    if let Err(error) = host
        .apply_live_session_tool_visibility_state(session_id, Some(next_visibility_state))
        .await
    {
        return rollback_idle_hot_swap_failure(
            host,
            session_id,
            &hydrated.current_identity,
            &hydrated.current_visibility_state,
            error,
        )
        .await;
    }
    if let Err(error) = host.persist_live_session(session_id).await {
        return rollback_idle_hot_swap_failure(
            host,
            session_id,
            &hydrated.current_identity,
            &hydrated.current_visibility_state,
            error,
        )
        .await;
    }

    Ok(())
}
