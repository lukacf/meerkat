//! Facade composition of the optional decision service.
//!
//! The facade selects the route from the effective realm config, resolves the
//! Jev credential through the shared credential resolver, and wires the
//! `decide` tool. The decision crate itself never elects a route, reads the
//! environment, or persists a token; unsupported configurations fail closed
//! here with the crate's typed unavailability reasons.

use std::sync::Arc;

use meerkat_core::{
    AgentLlmClient, AgentToolDispatcher, Config, DecisionBackendSelection, DecisionLimitsConfig,
    SessionLlmIdentity,
};
use meerkat_decision::{
    BackendKind, DecisionBackend, DecisionService, DecisionUnavailableReason, LlmRouteBackend,
    RouteBinding, wire_decision_tool,
};
use meerkat_llm_core::LlmClientAdapter;

use crate::factory::{AgentFactory, BuildAgentError};

/// Build the `decide` dispatcher for one agent from the effective config.
///
/// On the `llm` backend the tool takes the event-isolated fork of the
/// session's *current* client from each dispatch context, so the route follows
/// hot-swaps and fallbacks instead of a build-time copy.
pub(crate) fn wire_decision_tools(
    config: &Config,
) -> Result<Arc<dyn AgentToolDispatcher>, BuildAgentError> {
    let service = build_decision_service_with_binding(config, RouteBinding::AdmittedSession)?;
    Ok(wire_decision_tool(Arc::new(service)))
}

/// Build the shared [`DecisionService`] for the configured backend over an
/// already-admitted LLM route.
///
/// Hosts that already hold an admitted client pass it here. A host with no
/// admitted route uses [`build_host_decision_service`], which resolves the
/// explicit `[decision.host_route]` through the factory. The Jev backend
/// ignores the route.
pub fn build_decision_service(
    config: &Config,
    route_client: Option<Arc<dyn AgentLlmClient>>,
) -> Result<DecisionService, BuildAgentError> {
    let binding = match (config.decision.backend, route_client) {
        (DecisionBackendSelection::Jev, _) => None,
        (DecisionBackendSelection::Llm, Some(client)) => Some(RouteBinding::Fixed(client)),
        (DecisionBackendSelection::Llm, None) => {
            return Err(unavailable(
                DecisionUnavailableReason::HostRouteNotConfigured,
            ));
        }
    };
    build_decision_service_with_binding(config, binding.unwrap_or(RouteBinding::AdmittedSession))
}

fn build_decision_service_with_binding(
    config: &Config,
    binding: RouteBinding,
) -> Result<DecisionService, BuildAgentError> {
    config
        .decision
        .validate()
        .map_err(|error| BuildAgentError::Config(error.to_string()))?;
    let limits: DecisionLimitsConfig = config.decision.limits;
    let backend: Arc<dyn DecisionBackend> = match config.decision.backend {
        DecisionBackendSelection::Llm => Arc::new(match binding {
            RouteBinding::Fixed(client) => LlmRouteBackend::fixed(client, limits.max_output_tokens),
            RouteBinding::AdmittedSession => {
                LlmRouteBackend::admitted_session(limits.max_output_tokens)
            }
        }),
        DecisionBackendSelection::Jev => build_jev_backend(config)?,
    };
    Ok(DecisionService::new(backend, limits))
}

/// Build the shared [`DecisionService`] for a host invocation with no admitted
/// session (SDK gateways, feature policies).
///
/// On the `llm` backend the host must declare `[decision.host_route]`; the
/// factory resolves that exact provider/model/auth binding through the same
/// registry and credential authority every agent build uses, so no leaf
/// elects a default. The Jev backend needs no LLM route.
pub async fn build_host_decision_service(
    factory: &AgentFactory,
    config: &Config,
) -> Result<DecisionService, BuildAgentError> {
    config
        .decision
        .validate()
        .map_err(|error| BuildAgentError::Config(error.to_string()))?;
    let route_client: Option<Arc<dyn AgentLlmClient>> = match config.decision.backend {
        DecisionBackendSelection::Jev => None,
        DecisionBackendSelection::Llm => {
            let route =
                config.decision.host_route.as_ref().ok_or_else(|| {
                    unavailable(DecisionUnavailableReason::HostRouteNotConfigured)
                })?;
            let identity = SessionLlmIdentity {
                model: route.model.clone(),
                provider: route.provider,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: route.auth_binding.clone(),
            };
            let raw_client = factory
                .build_llm_client_for_identity(config, &identity)
                .await
                .map_err(|error| {
                    BuildAgentError::Config(format!("decision host route: {error}"))
                })?;
            Some(Arc::new(
                LlmClientAdapter::try_for_provider_identity(
                    raw_client,
                    route.model.clone(),
                    route.provider,
                )
                .map_err(|error| BuildAgentError::Config(error.to_string()))?,
            ))
        }
    };
    build_decision_service(config, route_client)
}

fn unavailable(reason: DecisionUnavailableReason) -> BuildAgentError {
    BuildAgentError::Config(format!("decision service unavailable: {reason}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn build_jev_backend(config: &Config) -> Result<Arc<dyn DecisionBackend>, BuildAgentError> {
    let jev = config.decision.jev.as_ref().ok_or_else(|| {
        unavailable(DecisionUnavailableReason::BackendNotConfigured {
            backend: BackendKind::Jev,
        })
    })?;
    // The permit is the only door into the adapter; it exists only when the
    // host granted disclosure of admitted inputs to this destination.
    let permit = meerkat_decision::JevDisclosurePermit::from_config(jev).map_err(unavailable)?;
    let credential = jev_credential::RealmJevCredentialSource::from_spec(&jev.credential)?;
    let backend = meerkat_decision::JevBackend::new(jev, permit, Arc::new(credential))
        .map_err(|error| BuildAgentError::Config(error.to_string()))?;
    Ok(Arc::new(backend))
}

#[cfg(target_arch = "wasm32")]
fn build_jev_backend(_config: &Config) -> Result<Arc<dyn DecisionBackend>, BuildAgentError> {
    Err(unavailable(DecisionUnavailableReason::BackendNotCompiled {
        backend: BackendKind::Jev,
    }))
}

#[cfg(not(target_arch = "wasm32"))]
mod jev_credential {
    use async_trait::async_trait;
    use meerkat_core::CredentialSourceSpec;
    use meerkat_decision::{
        BackendKind, DecisionUnavailableReason, JevBearerSecret, JevCredentialError,
        JevCredentialSource,
    };
    use meerkat_providers::runtime::{ResolverEnvironment, resolve_env_secret};

    use super::{BuildAgentError, unavailable};

    /// Jev credential borrowed from the realm's typed credential source.
    ///
    /// Only sources whose resolution needs no provider-binding lease are
    /// supported for a non-LLM backend; the rest fail closed at build time
    /// with a typed reason rather than pretending Jev is an LLM binding.
    pub(super) struct RealmJevCredentialSource {
        spec: CredentialSourceSpec,
        env: ResolverEnvironment,
    }

    impl RealmJevCredentialSource {
        pub(super) fn from_spec(spec: &CredentialSourceSpec) -> Result<Self, BuildAgentError> {
            match spec {
                CredentialSourceSpec::InlineSecret { .. } | CredentialSourceSpec::Env { .. } => {
                    Ok(Self {
                        spec: spec.clone(),
                        env: ResolverEnvironment::with_process_env(),
                    })
                }
                other => Err(unavailable(
                    DecisionUnavailableReason::CredentialSourceUnsupported {
                        backend: BackendKind::Jev,
                        kind: other.kind_label().to_string(),
                    },
                )),
            }
        }
    }

    #[async_trait]
    impl JevCredentialSource for RealmJevCredentialSource {
        async fn bearer_secret(&self) -> Result<JevBearerSecret, JevCredentialError> {
            match &self.spec {
                CredentialSourceSpec::InlineSecret { secret } => {
                    Ok(JevBearerSecret::new(secret.clone()))
                }
                CredentialSourceSpec::Env { env, fallback } => {
                    resolve_env_secret(env, fallback, &self.env.env_lookup)
                        .map(JevBearerSecret::new)
                        .map_err(|error| {
                            JevCredentialError::Missing(format!(
                                "credential source env `{env}`: {error}"
                            ))
                        })
                }
                other => Err(JevCredentialError::ResolutionFailed(format!(
                    "credential source kind `{}` is not supported for Jev",
                    other.kind_label()
                ))),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{CredentialSourceSpec, JevBackendConfig};

    fn jev_config(credential: CredentialSourceSpec, allow_disclosure: bool) -> JevBackendConfig {
        JevBackendConfig {
            endpoint: meerkat_decision::DEFAULT_JEV_ENDPOINT.into(),
            model: meerkat_decision::DEFAULT_JEV_MODEL.into(),
            credential,
            allow_disclosure,
        }
    }

    #[test]
    fn llm_backend_requires_a_route_client() {
        let config = Config::default();
        let error = build_decision_service(&config, None).unwrap_err();
        assert!(error.to_string().contains("host_route"), "{error}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn host_service_on_llm_backend_requires_an_explicit_host_route() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let error = build_host_decision_service(&factory, &config)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("host_route"), "{error}");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn host_service_on_jev_backend_needs_no_llm_route() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        config.decision.backend = DecisionBackendSelection::Jev;
        config.decision.jev = Some(jev_config(
            CredentialSourceSpec::InlineSecret {
                secret: "inline".into(),
            },
            true,
        ));
        let service = build_host_decision_service(&factory, &config)
            .await
            .unwrap();
        assert_eq!(service.backend().kind(), BackendKind::Jev);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn jev_requires_its_table_disclosure_permission_and_a_supported_credential_source() {
        let mut config = Config::default();
        config.decision.backend = DecisionBackendSelection::Jev;
        let error = build_decision_service(&config, None).unwrap_err();
        assert!(error.to_string().contains("[decision.jev]"));

        config.decision.jev = Some(jev_config(
            CredentialSourceSpec::Env {
                env: "JEV_API_KEY".into(),
                fallback: Vec::new(),
            },
            false,
        ));
        let error = build_decision_service(&config, None).unwrap_err();
        assert!(
            error.to_string().contains("disclosure"),
            "unexpected: {error}"
        );

        let jev = config.decision.jev.as_mut().unwrap();
        jev.allow_disclosure = true;
        jev.credential = CredentialSourceSpec::ManagedStore;
        let error = build_decision_service(&config, None).unwrap_err();
        assert!(
            error.to_string().contains("managed_store"),
            "unexpected: {error}"
        );

        let jev = config.decision.jev.as_mut().unwrap();
        jev.credential = CredentialSourceSpec::InlineSecret {
            secret: "inline".into(),
        };
        let service = build_decision_service(&config, None).unwrap();
        assert_eq!(service.backend().kind(), BackendKind::Jev);
    }
}
