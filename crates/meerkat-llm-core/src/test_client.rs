use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream};
use async_trait::async_trait;

/// Simple test client that emits a deterministic response.
pub struct TestClient {
    events: Vec<LlmEvent>,
    provider: meerkat_core::Provider,
    synthesize_usage: bool,
}

impl TestClient {
    pub fn new(events: Vec<LlmEvent>) -> Self {
        Self {
            events,
            provider: meerkat_core::Provider::Other,
            synthesize_usage: false,
        }
    }

    /// Build the deterministic client bound to a canonical provider identity.
    pub fn for_provider(provider: meerkat_core::Provider) -> Self {
        Self {
            events: Self::default_events(),
            provider,
            synthesize_usage: true,
        }
    }

    fn default_events() -> Vec<LlmEvent> {
        vec![
            LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
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
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for TestClient {
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
