//! Persisted-session recovery helpers.
//!
//! Populated by W1-D (`RecoveredCreateRequest`,
//! `RecoveryRuntimeBindingMode`) and W2-B (provider-override parsing,
//! `RecoveryContext` orchestrator for `load_persisted_session` and
//! `recovered_create_request*`).
//!
//! `recovery_overrides_from_turn` (depends on the RPC-private
//! `TurnOverrides`), `recovery_external_tools` (depends on the RPC
//! callback dispatcher), and `cleanup_recovered_runtime_if_new` (depends
//! on the RPC-private `ArchiveRuntimeCleanup` — deferred to W3-A) stay
//! in `meerkat-rpc` until those upstream blockers clear.

use meerkat_core::service::CreateSessionRequest;

/// Result of `recovered_create_request*`: a [`CreateSessionRequest`]
/// reconstructed from a persisted session, plus a flag indicating
/// whether the recovery process registered a fresh runtime binding for
/// the session (so callers can roll back the registration on
/// pre-run-apply failure via `cleanup_recovered_runtime_if_new`).
#[derive(Debug)]
pub struct RecoveredCreateRequest {
    /// Reconstructed create request that surfaces feed back into the
    /// session service.
    pub request: CreateSessionRequest,
    /// Whether the recovery flow registered a *new* runtime binding for
    /// this session (i.e. it was not already live in the runtime).
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
    use meerkat_session::PersistentSessionService;

    /// Surface-agnostic wiring needed by the recovery helpers.
    ///
    /// Holds the set of references `RecoveryContext::load_persisted_session`
    /// and `RecoveryContext::recovered_create_request*` need; surfaces
    /// build one per call (the values are short-lived borrows from
    /// `SessionRuntime`).
    pub struct RecoveryContext<'a> {
        /// Persistent session service (loaded session lookup, archive
        /// authority).
        pub service: &'a Arc<PersistentSessionService<FactoryAgentBuilder>>,
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

    impl RecoveryContext<'_> {
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

        /// Reconstruct a [`CreateSessionRequest`] from a persisted
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

        /// Reconstruct a [`CreateSessionRequest`] from a persisted
        /// session, selecting the runtime binding mode (authoritative
        /// vs local-resources-only).
        pub async fn recovered_create_request_with_runtime_binding_mode(
            &self,
            session_id: &SessionId,
            session: Session,
            overrides: meerkat_core::SurfaceSessionRecoveryOverrides,
            binding_mode: RecoveryRuntimeBindingMode,
        ) -> Result<RecoveredCreateRequest, RecoveryError> {
            let current_generation = match self.config_runtime.as_ref() {
                Some(runtime) => runtime.get().await.ok().map(|snapshot| snapshot.generation),
                None => None,
            };
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
            let recovered = match build_recovered_session(
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
                    runtime_build_mode: Some(RuntimeBuildMode::SessionOwned(bindings)),
                    require_runtime_build_mode: true,
                    realm_id: self.realm_id.map(ToString::to_string),
                    instance_id: self.instance_id.map(ToString::to_string),
                    backend: self.backend.map(ToString::to_string),
                    config_generation: current_generation,
                },
            ) {
                Ok(recovered) => recovered,
                Err(error) => {
                    if !runtime_was_registered {
                        self.runtime_adapter.unregister_session(session_id).await;
                    }
                    return Err(RecoveryError::Recovery(error));
                }
            };
            Ok(RecoveredCreateRequest {
                request: recovered.into_deferred_create_request(),
                runtime_was_registered,
            })
        }
    }
}

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
pub use context::RecoveryContext;
