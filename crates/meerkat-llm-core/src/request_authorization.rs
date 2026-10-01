//! Mechanical forwarding of the factory-selected model and credential target.
//!
//! This projection contains no secret, policy, identity mapping or mutable work
//! context. Provider adapters add the actual endpoint, wire model and enabled
//! hosted capabilities only when they own the final request.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::authorization::{ModelAuthorizationFacts, ModelAuthorizationUse};
use meerkat_core::{AuthCredentialIdentity, Message, ServerToolKind, SessionLlmIdentity};

use crate::provider_runtime::binding::ResolvedTextTarget;
use crate::{LlmClient, LlmError, LlmReplayProjection, LlmRequest, LlmStream, PreparedLlmRequest};

#[derive(Clone)]
pub struct ResolvedModelAuthorizationTarget {
    identity: Arc<SessionLlmIdentity>,
    backend_profile_id: Arc<str>,
    backend_kind: Arc<str>,
    credential: AuthCredentialIdentity,
}

impl ResolvedModelAuthorizationTarget {
    pub(crate) fn from_target(target: &ResolvedTextTarget) -> Self {
        Self {
            identity: Arc::new(target.identity().clone()),
            backend_profile_id: Arc::from(target.connection().backend_profile.id.as_str()),
            backend_kind: Arc::from(target.connection().backend.as_str()),
            credential: target.connection().credential_identity.clone(),
        }
    }

    fn controller_model_selection(&self) -> meerkat_core::ControllerModelSelection {
        meerkat_core::ControllerModelSelection::new(
            (*self.identity).clone(),
            self.credential.clone(),
            self.backend_profile_id.to_string(),
            self.backend_kind.to_string(),
        )
    }

    pub(crate) fn facts(
        &self,
        endpoint: &str,
        wire_model: &str,
        hosted_capabilities: Vec<ServerToolKind>,
        usage: ModelAuthorizationUse,
    ) -> ModelAuthorizationFacts {
        ModelAuthorizationFacts {
            identity: Arc::clone(&self.identity),
            wire_model: Arc::from(wire_model),
            hosted_capabilities: hosted_capabilities.into(),
            backend_profile_id: Some(Arc::clone(&self.backend_profile_id)),
            backend_kind: Arc::clone(&self.backend_kind),
            endpoint: Arc::from(endpoint),
            credential: Some(self.credential.clone()),
            usage,
            live_channel: None,
        }
    }
}

impl std::fmt::Debug for ResolvedModelAuthorizationTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ResolvedModelAuthorizationTarget([REDACTED])")
    }
}

pub(crate) struct SelectedTargetClient {
    inner: Arc<dyn LlmClient>,
    target: Arc<ResolvedModelAuthorizationTarget>,
}

impl SelectedTargetClient {
    pub(crate) fn new(inner: Arc<dyn LlmClient>, target: ResolvedModelAuthorizationTarget) -> Self {
        Self {
            inner,
            target: Arc::new(target),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for SelectedTargetClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(self.target.controller_model_selection())
    }

    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<crate::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != self.target.identity.model {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        self.inner.plain_model_route(logical_model)
    }

    fn project_replay_request(
        &self,
        messages: &[Message],
    ) -> Result<LlmReplayProjection, LlmError> {
        self.inner.project_replay_request(messages)
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn request_pressure(
        &self,
        request: &LlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, LlmError> {
        self.inner.request_pressure(request)
    }

    fn prepared_request_pressure(
        &self,
        request: &PreparedLlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, LlmError> {
        self.inner.prepared_request_pressure(request)
    }

    fn authored_cache_breakpoints(
        &self,
        request: &LlmRequest,
        messages: &[Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, LlmError> {
        self.inner.authored_cache_breakpoints(request, messages)
    }

    fn prepared_cache_breakpoints(
        &self,
        request: &PreparedLlmRequest,
        messages: &[Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, LlmError> {
        self.inner.prepared_cache_breakpoints(request, messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }

    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        if request.authorization().is_none() {
            return self.inner.stream_prepared(request);
        }
        Box::pin(async_stream::try_stream! {
            if request.request().model != self.target.identity.model {
                Err(LlmError::operation_refused(
                    meerkat_core::authorization::OperationRefusalKind::MalformedFacts,
                ))?;
            }
            // The request body is Arc-owned: retaining the target does not
            // duplicate the transcript or expose mutable request data.
            let bound = request.clone().with_authorization_target(Arc::clone(&self.target));
            let mut stream = self.inner.stream_prepared(&bound);
            while let Some(event) = stream.next().await {
                yield event?;
            }
        })
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }

    fn compile_schema(
        &self,
        schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        self.inner.compile_schema(schema)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use meerkat_core::authorization::{
        OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
        PreparedOperationAuthorization, WorkAuthorization, WorkAuthorizationContext,
    };

    struct RefusingPolicy;
    impl WorkAuthorization for RefusingPolicy {
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<
            Arc<dyn PreparedOperationAuthorization>,
            meerkat_core::OperationAuthorizationError,
        > {
            Err(OperationRefused::new(OperationRefusalKind::Denied).into())
        }
    }
    struct LegacyClient(AtomicUsize);
    #[async_trait]
    impl LlmClient for LegacyClient {
        fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(futures::stream::empty())
        }
        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }
        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }
    fn prepared(authorized: bool) -> PreparedLlmRequest {
        let request = PreparedLlmRequest::from_projection(
            LlmRequest::new("fixture", Vec::new()),
            LlmReplayProjection::new(Vec::new()),
        );
        if !authorized {
            return request;
        }
        request.with_authorization(Some(meerkat_core::LlmRequestAuthorization::new(
            WorkAuthorizationContext::new(
                Arc::new(RefusingPolicy),
                meerkat_core::exact_operation::OperationExecutionScope::Domain,
            ),
            meerkat_core::OperationId::new(),
            ModelAuthorizationUse::Inference,
        )))
    }

    #[tokio::test]
    async fn unsupported_client_refuses_companion_without_invoking_raw_send() {
        let client = LegacyClient(AtomicUsize::new(0));
        let request = prepared(true);
        let events = client.stream_prepared(&request).collect::<Vec<_>>().await;
        assert!(matches!(
            events.as_slice(),
            [Err(LlmError::OperationRefused { .. })]
        ));
        assert_eq!(client.0.load(Ordering::SeqCst), 0);
        let trusted = prepared(false);
        assert!(
            client
                .stream_prepared(&trusted)
                .collect::<Vec<_>>()
                .await
                .is_empty()
        );
        assert_eq!(client.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn plain_route_default_is_unsupported_without_invoking_legacy_client() {
        let client = LegacyClient(AtomicUsize::new(0));
        assert!(client.plain_model_route("fixture").is_err());
        assert_eq!(client.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn lowering_retains_companion_and_cannot_invent_selected_target() {
        let request = prepared(true);
        let mut changed = request.request().clone();
        changed.model = "different".to_owned();
        let lowered = request.with_lowered_request(changed);
        assert!(lowered.authorization().is_some());
        assert!(matches!(
            lowered.prepare_model_authorization(
                "https://example.invalid/messages",
                "different",
                Vec::new()
            ),
            Err(LlmError::OperationRefused { .. })
        ));
    }

    #[test]
    fn refusal_roundtrip_is_nonretryable_redacted_and_preserves_agent_class() {
        let error = LlmError::operation_refused(OperationRefusalKind::Denied);
        assert!(!error.is_retryable());
        assert_eq!(
            error.to_string(),
            "operation unavailable under current authorization"
        );
        assert!(!format!("{error:?}").contains("Denied"));
        let wire = serde_json::to_vec(&error).unwrap();
        let decoded: LlmError = serde_json::from_slice(&wire).unwrap();
        assert!(
            matches!(decoded.into_agent_error("fixture"), meerkat_core::AgentError::OperationRefused { refusal }
            if refusal.kind() == OperationRefusalKind::Denied)
        );
    }
}
