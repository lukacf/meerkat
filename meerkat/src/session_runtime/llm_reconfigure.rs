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

/// Surface-agnostic implementation of [`SessionLlmReconfigureHost`].
///
/// Surfaces construct one of these per-call (RPC, REST, MCP, …) so the
/// generated runtime-adapter reconfigure path can hydrate the live session, resolve target
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
                    Some(self.auth_lease.clone()),
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
        let web_search = match self.service.export_live_session(session_id).await {
            Ok(session) => session
                .session_metadata()
                .map(|metadata| metadata.tooling.web_search)
                .unwrap_or(meerkat_core::ToolCategoryOverride::Inherit),
            Err(_) => meerkat_core::ToolCategoryOverride::Inherit,
        };
        self.factory
            .request_policy_for_llm_identity(&config, identity, web_search)
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
        resolve_reconfigure_target_llm_identity(&registry, current, request)
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
        Ok(Box::new(
            self.service
                .acquire_runtime_turn_finalization_guard(session_id)
                .await,
        ))
    }

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
            .apply_runtime_session_llm_identity_under_runtime_turn_boundary(
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
            .apply_runtime_session_tool_visibility_state_under_runtime_turn_boundary(
                session_id,
                visibility_state,
            )
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn persist_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .persist_live_session_now_under_runtime_turn_boundary(session_id)
            .await
            .map(|_| ())
            .map_err(session_error_to_runtime_driver)
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .discard_live_session_under_runtime_turn_boundary(session_id)
            .await
            .map_err(session_error_to_runtime_driver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{AuthBindingRef, BindingId, BindingOrigin, Provider, RealmId};

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
            provider_params: None,
            auth_binding,
        }
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
