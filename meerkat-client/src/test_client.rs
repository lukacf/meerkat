use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
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
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            },
        ])
    }
}

#[async_trait]
impl LlmClient for TestClient {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
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
