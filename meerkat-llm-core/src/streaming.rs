use crate::error::LlmError;
use crate::types::{LlmDoneOutcome, LlmEvent, LlmStream};
use futures::StreamExt;
use meerkat_core::execution_scope::{
    ScopedEffectOutcome, ScopedModelEffectCustody, ScopedModelRequest,
};
use std::sync::{Arc, Mutex};

enum ModelFeedbackCustody {
    AwaitingResponse,
    Streaming(ScopedModelEffectCustody),
    Settled,
}

/// Ephemeral ownership transfer from the physical send to its terminal event
/// observer. Durable effect state remains with the injected runtime host.
#[derive(Clone)]
pub struct ScopedModelStreamFeedback {
    custody: Option<Arc<Mutex<ModelFeedbackCustody>>>,
}

struct ModelStreamObservation(ScopedModelStreamFeedback);

impl Drop for ModelStreamObservation {
    fn drop(&mut self) {
        let Some(slot) = &self.0.custody else {
            return;
        };
        let state = {
            let mut state = match slot.lock() {
                Ok(state) => state,
                Err(error) => {
                    tracing::error!(%error, "scoped model feedback lock poisoned during cancellation");
                    error.into_inner()
                }
            };
            std::mem::replace(&mut *state, ModelFeedbackCustody::Settled)
        };
        drop(state);
    }
}

impl ScopedModelStreamFeedback {
    pub fn new(scope: Option<&Arc<ScopedModelRequest>>) -> Self {
        Self {
            custody: scope.map(|_| Arc::new(Mutex::new(ModelFeedbackCustody::AwaitingResponse))),
        }
    }

    pub fn install(&self, custody: Option<ScopedModelEffectCustody>) -> Result<(), LlmError> {
        match (&self.custody, custody) {
            (None, None) => Ok(()),
            (Some(slot), Some(custody)) => {
                let mut state = slot.lock().map_err(|error| LlmError::Unknown {
                    message: format!("scoped model feedback lock poisoned: {error}"),
                })?;
                if !matches!(*state, ModelFeedbackCustody::AwaitingResponse) {
                    return Err(LlmError::InvalidRequest {
                        message: "scoped model stream received duplicate physical custody".into(),
                    });
                }
                *state = ModelFeedbackCustody::Streaming(custody);
                Ok(())
            }
            _ => Err(LlmError::InvalidRequest {
                message: "model stream and physical custody disagree about execution scope".into(),
            }),
        }
    }

    fn observe_usage(&self, usage: &meerkat_core::TurnUsage) -> Result<(), LlmError> {
        let Some(slot) = &self.custody else {
            return Ok(());
        };
        let mut state = slot.lock().map_err(|error| LlmError::Unknown {
            message: format!("scoped model feedback lock poisoned: {error}"),
        })?;
        let ModelFeedbackCustody::Streaming(custody) = &mut *state else {
            return Err(LlmError::InvalidRequest {
                message: "model usage event has no unsettled physical claim".into(),
            });
        };
        custody.observe_usage(usage);
        Ok(())
    }

    async fn settle(&self, outcome: ScopedEffectOutcome) -> Result<(), LlmError> {
        let Some(slot) = &self.custody else {
            return Ok(());
        };
        let state = {
            let mut state = slot.lock().map_err(|error| LlmError::Unknown {
                message: format!("scoped model feedback lock poisoned: {error}"),
            })?;
            std::mem::replace(&mut *state, ModelFeedbackCustody::Settled)
        };
        match state {
            ModelFeedbackCustody::Streaming(custody)
                if outcome == ScopedEffectOutcome::Succeeded =>
            {
                custody
                    .settle_completed()
                    .await
                    .map_err(crate::http::scoped_model_error)
            }
            ModelFeedbackCustody::Streaming(custody) => custody
                .settle(outcome)
                .await
                .map_err(crate::http::scoped_model_error),
            ModelFeedbackCustody::AwaitingResponse if outcome == ScopedEffectOutcome::Unknown => {
                Ok(())
            }
            ModelFeedbackCustody::AwaitingResponse | ModelFeedbackCustody::Settled => {
                Err(LlmError::InvalidRequest {
                    message: "model terminal event has no unsettled physical claim".into(),
                })
            }
        }
    }

    pub fn wrap<'a>(&self, mut stream: LlmStream<'a>) -> LlmStream<'a> {
        if self.custody.is_none() {
            return stream;
        }
        let observation = ModelStreamObservation(self.clone());
        Box::pin(async_stream::try_stream! {
            while let Some(item) = stream.next().await {
                match item {
                    Ok(event @ LlmEvent::Done { .. }) => {
                        let outcome = if matches!(&event, LlmEvent::Done {
                            outcome: LlmDoneOutcome::Success { .. },
                        }) {
                            ScopedEffectOutcome::Succeeded
                        } else {
                            ScopedEffectOutcome::Unknown
                        };
                        observation.0.settle(outcome).await?;
                        yield event;
                        return;
                    }
                    Ok(event) => {
                        if let LlmEvent::UsageUpdate { usage } = &event {
                            observation.0.observe_usage(usage)?;
                        }
                        yield event;
                    }
                    Err(error) => {
                        observation.0.settle(ScopedEffectOutcome::Unknown).await?;
                        Err(error)?;
                    }
                }
            }
            observation.0.settle(ScopedEffectOutcome::Unknown).await?;
        })
    }
}

pub fn ensure_terminal_done(mut stream: LlmStream<'_>) -> LlmStream<'_> {
    Box::pin(async_stream::stream! {
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    if matches!(event, LlmEvent::Done { .. }) {
                        yield Ok(event);
                        return;
                    }
                    yield Ok(event);
                }
                Err(error) => {
                    yield Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Error { error },
                    });
                    return;
                }
            }
        }

        yield Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Error {
                error: LlmError::IncompleteResponse {
                    message: "Stream ended without Done event".to_string(),
                },
            },
        });
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::StopReason;

    #[tokio::test]
    async fn test_ensure_terminal_done_converts_error_to_done() {
        let inner = Box::pin(futures::stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: "hi".to_string(),
                meta: None,
            }),
            Err(LlmError::ConnectionReset),
        ]));

        let mut stream = ensure_terminal_done(inner);
        let mut saw_done = false;

        while let Some(item) = stream.next().await {
            let event = item.expect("wrapper should not yield Err");
            match event {
                LlmEvent::TextDelta { .. } => {}
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error { error },
                } => {
                    assert!(matches!(error, LlmError::ConnectionReset));
                    saw_done = true;
                }
                other => panic!("Unexpected event: {other:?}"),
            }
        }

        assert!(saw_done, "Expected terminal Done event");
    }

    #[tokio::test]
    async fn test_ensure_terminal_done_appends_incomplete_done() {
        let inner = Box::pin(futures::stream::iter([Ok(LlmEvent::TextDelta {
            delta: "hi".to_string(),
            meta: None,
        })]));

        let mut stream = ensure_terminal_done(inner);
        let mut done_count = 0;

        while let Some(item) = stream.next().await {
            let event = item.expect("wrapper should not yield Err");
            if let LlmEvent::Done { outcome } = event {
                done_count += 1;
                match outcome {
                    LlmDoneOutcome::Error { error } => {
                        assert!(matches!(error, LlmError::IncompleteResponse { .. }));
                    }
                    LlmDoneOutcome::Success { .. } => panic!("Expected error outcome"),
                }
            }
        }

        assert_eq!(done_count, 1, "Expected exactly one Done event");
    }

    #[tokio::test]
    async fn test_ensure_terminal_done_stops_after_success_done() {
        let inner = Box::pin(futures::stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: "hi".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            }),
            Ok(LlmEvent::TextDelta {
                delta: "later".to_string(),
                meta: None,
            }),
        ]));

        let mut stream = ensure_terminal_done(inner);
        let mut done_count = 0;
        let mut saw_later_delta = false;

        while let Some(item) = stream.next().await {
            let event = item.expect("wrapper should not yield Err");
            match event {
                LlmEvent::TextDelta { delta, .. } => {
                    if delta == "later" {
                        saw_later_delta = true;
                    }
                }
                LlmEvent::Done { outcome } => {
                    done_count += 1;
                    assert!(matches!(outcome, LlmDoneOutcome::Success { .. }));
                }
                _ => {}
            }
        }

        assert_eq!(done_count, 1, "Expected exactly one Done event");
        assert!(
            !saw_later_delta,
            "Expected stream to stop yielding events after Done"
        );
    }
}
