//! Shared controls for nonterminal diagnostics beside physical results.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};

pub(crate) const PHYSICAL_TEXT: &str = "physical response, not diagnostic text";

#[derive(Clone, Copy, Debug)]
pub(crate) enum CompletionCase {
    Success,
    TerminalError,
    StreamError,
    Truncated,
    /// The stream ends with no terminal event and no error.
    Unterminated,
}

impl CompletionCase {
    pub(crate) const ALL: [Self; 5] = [
        Self::Success,
        Self::TerminalError,
        Self::StreamError,
        Self::Truncated,
        Self::Unterminated,
    ];
}

/// Every stop reason that ends a memory stage's completion before a complete
/// answer, apart from the output ceiling's own `Truncated`. Each stage's tests
/// refuse all of them.
pub(crate) const INCOMPLETE_STOPS: [meerkat_core::StopReason; 6] = [
    meerkat_core::StopReason::ContentFilter,
    meerkat_core::StopReason::ToolUse,
    meerkat_core::StopReason::Cancelled,
    meerkat_core::StopReason::Refusal,
    meerkat_core::StopReason::PauseTurn,
    meerkat_core::StopReason::ContextWindowExceeded,
];

/// A client whose completion streams `body` and then stops with
/// `stop_reason`.
pub(crate) fn stopping_client(
    body: &str,
    stop_reason: meerkat_core::StopReason,
) -> meerkat_client::TestClient {
    meerkat_client::TestClient::new(vec![
        LlmEvent::TextDelta {
            delta: body.to_string(),
            meta: None,
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::success(stop_reason),
        },
    ])
}

pub(crate) struct ObservationClient {
    events: Vec<Result<LlmEvent, LlmError>>,
    calls: AtomicUsize,
    operation_id: meerkat_core::OperationId,
}

impl ObservationClient {
    pub(crate) fn new(case: CompletionCase) -> Self {
        let operation_id = meerkat_core::OperationId::new();
        let mut events = vec![
            Ok(LlmEvent::TextDelta {
                delta: PHYSICAL_TEXT.into(),
                meta: None,
            }),
            Ok(LlmEvent::OperationObservationFailed {
                operation_id: operation_id.clone(),
                phase: meerkat_core::authorization::OperationObservationPhase::Outcome,
            }),
        ];
        events.extend(match case {
            CompletionCase::Success => Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            })),
            CompletionCase::TerminalError => Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error {
                    error: LlmError::ConnectionReset,
                },
            })),
            CompletionCase::StreamError => Some(Err(LlmError::ConnectionReset)),
            CompletionCase::Truncated => Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::MaxTokens),
            })),
            CompletionCase::Unterminated => None,
        });
        Self {
            events,
            calls: AtomicUsize::new(0),
            operation_id,
        }
    }

    pub(crate) fn assert_observed_once_without_retry(&self, log: &str) {
        assert_eq!(self.calls.load(Ordering::SeqCst), 1);
        assert_eq!(log.matches("LLM operation observation failed").count(), 1);
        assert!(log.contains(&self.operation_id.to_string()), "{log}");
        assert!(log.contains("phase=Outcome"), "{log}");
        assert!(!log.contains(PHYSICAL_TEXT), "response text leaked: {log}");
    }
}

#[async_trait]
impl LlmClient for ObservationClient {
    fn stream<'a>(&'a self, _request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(self.events.clone()))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

pub(crate) async fn capture_warnings<T>(future: impl Future<Output = T>) -> (T, String) {
    use tracing::instrument::WithSubscriber;

    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .without_time()
        .finish();
    // A second registered Dispatch keeps callsite interest from being cached
    // `never` by threads without a subscriber.
    let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let result = future.with_subscriber(subscriber).await;
    let log = String::from_utf8_lossy(
        &writer
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_owned();
    (result, log)
}

pub(crate) fn settlement_failure(
    is_error: bool,
) -> meerkat_core::ops::ToolDispatchSettlementFailure {
    meerkat_core::ops::ToolDispatchSettlementFailure {
        admission_source: meerkat_core::ops::ToolDispatchAdmissionSource::AuthorizationAudit,
        effect_kind: meerkat_core::LiveBridgeEffectKind::ToolDispatch,
        physical_outcome: if is_error {
            meerkat_core::LiveBridgeEffectOutcome::Failed
        } else {
            meerkat_core::LiveBridgeEffectOutcome::Committed
        },
        failure_kind: meerkat_core::ops::ToolDispatchTerminalErrorKind::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::capture_warnings;

    const PROBE: &str = "capture anchor probe";

    fn warn_probe() {
        tracing::warn!("{PROBE}");
    }

    /// While the capture's subscriber is registered, a thread without a
    /// subscriber reaches the probe's callsite first and rebuilds the
    /// interest cache. With the capture's subscriber as the only registered
    /// one, tracing asks that thread's empty default instead and caches the
    /// callsite as `never` for every thread, so the capture records nothing.
    /// A second registered Dispatch makes the rebuild ask the registered
    /// subscribers.
    #[tokio::test]
    async fn capture_records_a_callsite_another_thread_reached_first() {
        let ((), log) = capture_warnings(async {
            let probe = std::thread::spawn(|| {
                tracing::dispatcher::with_default(&tracing::Dispatch::none(), || {
                    warn_probe();
                    tracing::callsite::rebuild_interest_cache();
                });
            });
            assert!(probe.join().is_ok(), "probe thread panicked");
            warn_probe();
        })
        .await;
        assert_eq!(log.matches(PROBE).count(), 1, "{log}");
    }
}
