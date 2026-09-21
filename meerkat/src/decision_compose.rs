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
};
use meerkat_decision::{
    BackendKind, DecisionBackend, DecisionService, DecisionUnavailableReason, SessionLlmBackend,
    wire_decision_tool,
};

use crate::factory::BuildAgentError;

/// Build the `decide` dispatcher for one agent from the effective config.
///
/// `route_client` is the session's admitted LLM route on an event-isolated
/// adapter; it is required only when the configured backend is `session_llm`.
pub(crate) fn wire_decision_tools(
    config: &Config,
    route_client: Option<Arc<dyn AgentLlmClient>>,
) -> Result<Arc<dyn AgentToolDispatcher>, BuildAgentError> {
    let service = build_decision_service(config, route_client)?;
    Ok(wire_decision_tool(Arc::new(service)))
}

/// Build the shared [`DecisionService`] for the configured backend.
///
/// Hosts that call the service outside an agent (feature policies, SDK
/// gateways) compose through this same function so both paths share one
/// route selection and one credential seam.
pub fn build_decision_service(
    config: &Config,
    route_client: Option<Arc<dyn AgentLlmClient>>,
) -> Result<DecisionService, BuildAgentError> {
    config
        .decision
        .validate()
        .map_err(|error| BuildAgentError::Config(error.to_string()))?;
    let limits: DecisionLimitsConfig = config.decision.limits;
    let backend: Arc<dyn DecisionBackend> = match config.decision.backend {
        DecisionBackendSelection::SessionLlm => {
            let client = route_client.ok_or_else(|| {
                BuildAgentError::Config(
                    "decision backend `session_llm` requires the session's admitted LLM route, \
                     but no LLM client was built for this agent"
                        .to_string(),
                )
            })?;
            Arc::new(SessionLlmBackend::new(client, limits.max_output_tokens))
        }
        DecisionBackendSelection::Jev => build_jev_backend(config)?,
    };
    Ok(DecisionService::new(backend, limits))
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
    if !jev.allow_disclosure {
        return Err(unavailable(
            DecisionUnavailableReason::DisclosureNotPermitted {
                backend: BackendKind::Jev,
            },
        ));
    }
    let credential = jev_credential::RealmJevCredentialSource::from_spec(&jev.credential)?;
    let backend = meerkat_decision::JevBackend::new(jev, Arc::new(credential))
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

    #[test]
    fn session_llm_backend_requires_a_route_client() {
        let config = Config::default();
        let error = build_decision_service(&config, None).unwrap_err();
        assert!(error.to_string().contains("session_llm"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn jev_requires_its_table_disclosure_permission_and_a_supported_credential_source() {
        let mut config = Config::default();
        config.decision.backend = DecisionBackendSelection::Jev;
        let error = build_decision_service(&config, None).unwrap_err();
        assert!(error.to_string().contains("[decision.jev]"));

        config.decision.jev = Some(JevBackendConfig::default());
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
