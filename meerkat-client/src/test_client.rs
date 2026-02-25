use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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
