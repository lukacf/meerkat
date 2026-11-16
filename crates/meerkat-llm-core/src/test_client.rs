use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream};
use async_trait::async_trait;

/// Simple test client that emits a deterministic response.
pub struct TestClient {
    events: Vec<LlmEvent>,
    provider: meerkat_core::Provider,
    synthesize_usage: bool,
    reasoning_route: Option<meerkat_core::ReasoningRouteObservation>,
}

impl TestClient {
    pub fn new(events: Vec<LlmEvent>) -> Self {
        Self {
            events,
            provider: meerkat_core::Provider::Other,
            synthesize_usage: false,
            reasoning_route: None,
        }
    }

    /// Build the deterministic client bound to a canonical provider identity.
    pub fn for_provider(provider: meerkat_core::Provider) -> Self {
        Self {
            events: Self::default_events(),
            provider,
            synthesize_usage: true,
            reasoning_route: None,
        }
    }

    /// Report `route` as the route this client serves
    /// ([`LlmClient::reasoning_route`]), so a test can script the route a
    /// request's reasoning effort is resolved against. A test client
    /// reports no route unless one is set here.
    #[must_use]
    pub fn with_reasoning_route(mut self, route: meerkat_core::ReasoningRouteObservation) -> Self {
        self.reasoning_route = Some(route);
        self
    }

    fn default_events() -> Vec<LlmEvent> {
        vec![
            LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            },
        ]
    }
}

impl Default for TestClient {
    fn default() -> Self {
        Self {
            events: Self::default_events(),
            provider: meerkat_core::Provider::Other,
            synthesize_usage: true,
            reasoning_route: None,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for TestClient {
    fn reasoning_route(&self) -> Option<meerkat_core::ReasoningRouteObservation> {
        self.reasoning_route.clone()
    }

    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let mut events = self.events.clone();
        if self.synthesize_usage
            && !events
                .iter()
                .any(|event| matches!(event, LlmEvent::UsageUpdate { .. }))
            && let Some(done_index) = events.iter().position(|event| {
                matches!(
                    event,
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success { .. }
                    }
                )
            })
        {
            events.insert(
                done_index,
                LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        self.provider,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                },
            );
        }
        crate::streaming::ensure_terminal_done(Box::pin(futures::stream::iter(
            events.into_iter().map(Ok),
        )))
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.provider
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use meerkat_core::model_profile::capabilities::EffortLevel;
    use meerkat_core::provider_matrix::SelfHostedBackendKind;
    use meerkat_core::{
        AccountLevelStatement, AgentLlmClient, ModelApi, Provider, ReasoningBackend,
        ReasoningRouteObservation,
    };

    use super::TestClient;
    use crate::adapter::LlmClientAdapter;
    use crate::types::LlmClient;

    /// A test client reports no route until a test sets one, then reports
    /// exactly that one, also through the adapter the agent loop holds.
    #[test]
    fn every_adapter_reports_its_route() {
        assert_eq!(TestClient::default().reasoning_route(), None);
        assert_eq!(TestClient::new(Vec::new()).reasoning_route(), None);
        assert_eq!(
            TestClient::for_provider(Provider::OpenAI).reasoning_route(),
            None
        );

        let route = ReasoningRouteObservation {
            backend: ReasoningBackend::SelfHosted(SelfHostedBackendKind::SelfHosted),
            api: ModelApi::ChatCompletions,
            account: AccountLevelStatement::NotApplicable,
            operator_levels: Some(vec![EffortLevel::Low, EffortLevel::High]),
        };
        let client =
            TestClient::for_provider(Provider::SelfHosted).with_reasoning_route(route.clone());
        assert_eq!(client.reasoning_route(), Some(route.clone()));

        let adapter = LlmClientAdapter::new(Arc::new(client), "test-model".to_string());
        assert_eq!(AgentLlmClient::reasoning_route(&adapter), Some(route));
    }
}
