//! Persisted-session recovery helpers.
//!
//! Populated by W1-D (`RecoveredCreateRequest`,
//! `RecoveryRuntimeBindingMode`) and W2-B (provider-override parsing,
//! `RecoveryContext` orchestrator for `load_persisted_session` and
//! `recovered_create_request*`).
//!
//! `recovery_overrides_from_turn` (depends on the RPC-private
//! `TurnOverrides`) and `recovery_external_tools` (depends on the RPC
//! callback dispatcher) remain in `meerkat-rpc`. Runtime rollback cleanup is
//! surface-agnostic and owned by `LiveOrchestrator` via
//! [`super::runtime_state::ArchiveRuntimeCleanup`].

use meerkat_core::service::CreateSessionRequest;

/// Re-inject process-local resources after canonical durable recovery lowering.
/// Durable policy and build state are deliberately absent: changes to those
/// must enter `SurfaceSessionRecoveryOverrides`, not this resource seam.
pub fn inject_recovery_resources(
    build: &mut meerkat_core::service::SessionBuildOptions,
    resources: &crate::AgentBuildConfig,
) {
    let resources = resources.to_session_build_options();
    build.llm_client_override = resources
        .llm_client_override
        .or(build.llm_client_override.take());
    build.agent_llm_client_decorator = resources
        .agent_llm_client_decorator
        .or(build.agent_llm_client_decorator.take());
    build.external_tools = resources.external_tools.or(build.external_tools.take());
    build.mcp_servers = resources.mcp_servers;
    build.custom_models = resources.custom_models;
    build.model_fallback = resources.model_fallback;
    build.image_generation_provider = resources.image_generation_provider;
    build.auto_compact_threshold_override = resources.auto_compact_threshold_override;
    build.compaction_curator_override = resources.compaction_curator_override;
    build.session_comms_runtime_override = resources.session_comms_runtime_override;
    build.blob_store_override = resources.blob_store_override;
    build.checkpointer = resources.checkpointer.or(build.checkpointer.take());
    build.schedule_tools = resources.schedule_tools;
    build.workgraph_tools = resources.workgraph_tools;
    build.workgraph_namespace_grant = resources.workgraph_namespace_grant;
    build.mob_tools = resources.mob_tools;
    build.tool_dispatch_admission = resources.tool_dispatch_admission;
    build.tool_consequence_policy_registry = resources.tool_consequence_policy_registry;
    build.host_prompt_sections = resources.host_prompt_sections;
}

/// Result of `recovered_create_request*`: a [`CreateSessionRequest`]
/// reconstructed from a persisted session, plus a flag indicating
/// whether the runtime binding already existed before recovery (so callers
/// roll back only a binding created by this recovery attempt on
/// pre-run-apply failure via `cleanup_recovered_runtime_if_new`).
#[derive(Debug)]
pub struct RecoveredCreateRequest {
    /// Reconstructed create request that surfaces feed back into the
    /// session service.
    pub request: CreateSessionRequest,
    /// Whether this session already had a runtime binding before recovery.
    /// `false` means this recovery attempt created the binding and owns
    /// rollback if later preparation fails.
    pub runtime_was_registered: bool,
}

/// How recovery should bind the persisted session into the runtime.
///
/// - `Authoritative` — register the recovered session with full machine
///   authority (default; used when the operator explicitly resumes).
/// - `LocalResources` — bind only local-resource projections, leaving
///   canonical authority to a peer (used when the recovered session is
///   observed during a peer-driven flow rather than a user-initiated
///   resume).
#[derive(Clone, Copy, Debug)]
pub enum RecoveryRuntimeBindingMode {
    /// Full authority: this process owns the session machine.
    Authoritative,
    /// Local-resource binding only; another node owns the machine.
    LocalResources,
}

/// Render the canonical "unknown provider" error message used by the
/// recovery flow when a turn override carries an unparseable provider
/// id.
#[must_use]
pub fn unknown_provider_message(provider: &str) -> String {
    format!("unknown provider '{provider}' (expected anthropic, openai, gemini, or self_hosted)")
}

/// Strict provider-id parser used by the recovery flow.
///
/// Returns the canonical [`unknown_provider_message`] on failure so
/// surfaces can map onto their own wire error without re-stringifying
/// the input.
pub fn parse_provider_override(provider: &str) -> Result<meerkat_core::Provider, String> {
    meerkat_core::Provider::parse_strict(provider).ok_or_else(|| unknown_provider_message(provider))
}

/// `RecoveryContext` orchestrator (gated on `session-store`).
///
/// `RecoveryContext` is only meaningful when the `PersistentSessionService`
/// is compiled in. The pure data types
/// ([`RecoveredCreateRequest`], [`RecoveryRuntimeBindingMode`]) and the
/// override parser ([`parse_provider_override`]) remain available
/// regardless of feature gates so other surfaces (CLI subcommands,
/// schema codegen) can reference them without pulling in the persistent
/// service.
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod context {
    use std::sync::Arc;

    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;
    use meerkat_core::{
        AgentLlmClientDecorator, AgentToolDispatcher, ConfigRuntime, RuntimeBuildMode, Session,
        SurfaceSessionRecoveryContext, build_recovered_session, connection::RealmId,
    };
    use meerkat_runtime::MeerkatMachine;

    use super::{RecoveredCreateRequest, RecoveryRuntimeBindingMode};
    use crate::factory::encode_llm_client_override_for_service;
    use crate::service_factory::FactoryAgentBuilder;
    use crate::session_runtime::errors::RecoveryError;
    use meerkat_session::{PersistentSessionService, SessionAgentBuilder};

    /// Surface-agnostic wiring needed by the recovery helpers.
    ///
    /// Holds the set of references `RecoveryContext::load_persisted_session`
    /// and `RecoveryContext::recovered_create_request*` need; surfaces
    /// build one per call (the values are short-lived borrows from
    /// `SessionRuntime`).
    pub struct RecoveryContext<'a, B: SessionAgentBuilder + 'static = FactoryAgentBuilder> {
        /// Persistent session service (loaded session lookup, archive
        /// authority).
        pub service: &'a Arc<PersistentSessionService<B>>,
        /// Runtime adapter used to register/unregister session bindings.
        pub runtime_adapter: &'a Arc<MeerkatMachine>,
        /// Realm id stamped onto rebuilt session build options.
        pub realm_id: Option<&'a RealmId>,
        /// Instance id stamped onto rebuilt session build options.
        pub instance_id: Option<&'a str>,
        /// Backend tag stamped onto rebuilt session build options.
        pub backend: Option<&'a str>,
        /// Default LLM client override (testing or single-client surfaces).
        pub default_llm_client: Option<Arc<dyn meerkat_client::LlmClient>>,
        /// Optional decorator wrapped around any session LLM client.
        pub agent_llm_client_decorator: Option<AgentLlmClientDecorator>,
        /// Optional external tool dispatcher injected into the recovered
        /// build (e.g. RPC callback dispatcher).
        pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
        /// Optional config runtime; queried for the current generation.
        pub config_runtime: Option<Arc<ConfigRuntime>>,
    }

    impl<B: SessionAgentBuilder + 'static> RecoveryContext<'_, B> {
        /// Load the persisted authoritative snapshot for `session_id`,
        /// honouring the durable archive flag (an archived session
        /// resolves to `None`).
        pub async fn load_persisted_session(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<Session>, SessionError> {
            let Some(session) = self.service.load_authoritative_session(session_id).await? else {
                return Ok(None);
            };
            if self
                .service
                .session_archived_by_authority(session_id, &session)
                .await?
            {
                return Ok(None);
            }
            Ok(Some(session))
        }

        /// Reconstruct a [`meerkat_core::service::CreateSessionRequest`] from a persisted
        /// session using authoritative runtime binding (default for
        /// operator-driven resume).
        pub async fn recovered_create_request(
            &self,
            session_id: &SessionId,
            session: Session,
            overrides: meerkat_core::SurfaceSessionRecoveryOverrides,
        ) -> Result<RecoveredCreateRequest, RecoveryError> {
            self.recovered_create_request_with_runtime_binding_mode(
                session_id,
                session,
                overrides,
                RecoveryRuntimeBindingMode::Authoritative,
            )
            .await
        }

        /// Reconstruct a [`meerkat_core::service::CreateSessionRequest`] from a persisted
        /// session, selecting the runtime binding mode (authoritative
        /// vs local-resources-only).
        pub async fn recovered_create_request_with_runtime_binding_mode(
            &self,
            session_id: &SessionId,
            session: Session,
            overrides: meerkat_core::SurfaceSessionRecoveryOverrides,
            binding_mode: RecoveryRuntimeBindingMode,
        ) -> Result<RecoveredCreateRequest, RecoveryError> {
            let current_generation = self.recovery_config_generation().await;
            let runtime_was_registered = self.runtime_adapter.contains_session(session_id).await;
            let bindings = match binding_mode {
                RecoveryRuntimeBindingMode::Authoritative => {
                    self.runtime_adapter
                        .prepare_bindings(session_id.clone())
                        .await
                }
                RecoveryRuntimeBindingMode::LocalResources => {
                    self.runtime_adapter
                        .prepare_local_session_bindings(session_id.clone())
                        .await
                }
            }
            .map_err(|e| RecoveryError::BindingPreparation {
                session_id: session_id.clone(),
                message: e.to_string(),
            })?;
            let request = match self.lower_recovered_create_request(
                session,
                overrides,
                bindings,
                current_generation,
            ) {
                Ok(request) => request,
                Err(error) => {
                    if !runtime_was_registered
                        && let Err(cleanup_error) =
                            self.runtime_adapter.unregister_session(session_id).await
                    {
                        return Err(RecoveryError::BindingPreparation {
                            session_id: session_id.clone(),
                            message: format!(
                                "{error}; additionally failed to unregister newly recovered runtime binding: {cleanup_error}"
                            ),
                        });
                    }
                    return Err(error);
                }
            };
            Ok(RecoveredCreateRequest {
                request,
                runtime_was_registered,
            })
        }

        /// Lower durable recovery using an already-owned materialization lease.
        /// Registration and cancellation compensation remain with its caller;
        /// this method never prepares or unregisters a second binding.
        pub async fn recovered_create_request_with_bindings(
            &self,
            session: Session,
            overrides: meerkat_core::SurfaceSessionRecoveryOverrides,
            bindings: meerkat_core::SessionRuntimeBindings,
        ) -> Result<meerkat_core::service::CreateSessionRequest, RecoveryError> {
            let current_generation = self.recovery_config_generation().await;
            self.lower_recovered_create_request(session, overrides, bindings, current_generation)
        }

        async fn recovery_config_generation(&self) -> Option<u64> {
            match self.config_runtime.as_ref() {
                Some(runtime) => runtime.get().await.ok().map(|snapshot| snapshot.generation),
                None => None,
            }
        }

        fn lower_recovered_create_request(
            &self,
            session: Session,
            overrides: meerkat_core::SurfaceSessionRecoveryOverrides,
            bindings: meerkat_core::SessionRuntimeBindings,
            current_generation: Option<u64>,
        ) -> Result<meerkat_core::service::CreateSessionRequest, RecoveryError> {
            let recovered = build_recovered_session(
                session,
                &overrides,
                SurfaceSessionRecoveryContext {
                    llm_client_override: self
                        .default_llm_client
                        .as_ref()
                        .map(|client| encode_llm_client_override_for_service(Arc::clone(client))),
                    agent_llm_client_decorator: self.agent_llm_client_decorator.clone(),
                    external_tools: self.external_tools.clone(),
                    checkpointer: None,
                    runtime_build_mode: RuntimeBuildMode::SessionOwned(bindings),
                    realm_id: self.realm_id.cloned(),
                    instance_id: self.instance_id.map(ToString::to_string),
                    backend: self.backend.map(ToString::to_string),
                    config_generation: current_generation,
                },
            )
            .map_err(RecoveryError::Recovery)?;
            Ok(recovered.into_deferred_create_request())
        }
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use context::RecoveryContext;
