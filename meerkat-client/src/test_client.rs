use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream};
use async_trait::async_trait;

/// Simple test client that emits a deterministic response.
pub struct TestClient {
    events: Vec<LlmEvent>,
}

impl TestClient {
    pub fn new(events: Vec<LlmEvent>) -> Self {
        Self { events }
    }
}

impl Default for TestClient {
    fn default() -> Self {
        Self::new(vec![
            LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            },
        ])
    }
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait
)]
impl LlmClient for TestClient {
    fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
        let events = self.events.clone();
        crate::streaming::ensure_terminal_done(Box::pin(futures::stream::iter(
            events.into_iter().map(Ok),
        )))
    }

    fn provider(&self) -> &'static str {
        "test"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}
