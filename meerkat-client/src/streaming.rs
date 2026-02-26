use crate::error::LlmError;
use crate::types::{LlmDoneOutcome, LlmEvent, LlmStream};
use futures::StreamExt;

pub(crate) fn ensure_terminal_done(mut stream: LlmStream<'_>) -> LlmStream<'_> {
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
