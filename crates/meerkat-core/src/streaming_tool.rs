//! Typed liveness primitives for synchronous streaming tool execution.

use crate::ToolError;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use serde::{Deserialize, Deserializer, Serialize};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// One accepted, non-terminal liveness update from a streaming tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolProgressFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completed_units: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total_units: Option<u64>,
}

impl ToolProgressFrame {
    pub fn new(
        message: Option<String>,
        completed_units: Option<u64>,
        total_units: Option<u64>,
    ) -> Result<Self, ToolProgressFrameError> {
        let message = message.and_then(|message| {
            let trimmed = message.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        if completed_units.is_some() != total_units.is_some() {
            return Err(ToolProgressFrameError::IncompleteUnits);
        }
        if matches!(total_units, Some(0)) {
            return Err(ToolProgressFrameError::ZeroTotalUnits);
        }
        if let (Some(completed), Some(total)) = (completed_units, total_units)
            && completed > total
        {
            return Err(ToolProgressFrameError::CompletedExceedsTotal);
        }
        if message.is_none() && completed_units.is_none() {
            return Err(ToolProgressFrameError::Empty);
        }
        Ok(Self {
            message,
            completed_units,
            total_units,
        })
    }

    pub fn message(message: impl Into<String>) -> Result<Self, ToolProgressFrameError> {
        Self::new(Some(message.into()), None, None)
    }

    pub fn units(
        completed_units: u64,
        total_units: u64,
        message: Option<String>,
    ) -> Result<Self, ToolProgressFrameError> {
        Self::new(message, Some(completed_units), Some(total_units))
    }

    pub fn message_text(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub const fn completed_units(&self) -> Option<u64> {
        self.completed_units
    }

    pub const fn total_units(&self) -> Option<u64> {
        self.total_units
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolProgressFrameWire {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    completed_units: Option<u64>,
    #[serde(default)]
    total_units: Option<u64>,
}

impl<'de> Deserialize<'de> for ToolProgressFrame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ToolProgressFrameWire::deserialize(deserializer)?;
        Self::new(wire.message, wire.completed_units, wire.total_units)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ToolProgressFrameError {
    #[error("progress frame must contain a message or a complete units update")]
    Empty,
    #[error("completed_units and total_units must be supplied together")]
    IncompleteUnits,
    #[error("progress total_units must be greater than zero")]
    ZeroTotalUnits,
    #[error("progress completed_units must not exceed total_units")]
    CompletedExceedsTotal,
}

struct ToolProgressSinkInner {
    sender: tokio::sync::mpsc::Sender<ToolProgressFrame>,
}

/// Bounded producer used by a streaming implementation to report liveness.
#[derive(Clone)]
pub struct ToolProgressSink {
    inner: Arc<ToolProgressSinkInner>,
}

impl std::fmt::Debug for ToolProgressSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolProgressSink")
            .field("capacity", &self.inner.sender.capacity())
            .finish_non_exhaustive()
    }
}

impl PartialEq for ToolProgressSink {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for ToolProgressSink {}

impl ToolProgressSink {
    /// Report a frame without allowing diagnostics to block execution.
    ///
    /// Only `Ok(())` means the canonical supervisor accepted the frame. A
    /// malformed, closed, or backpressured report never refreshes liveness.
    pub fn try_report(&self, frame: ToolProgressFrame) -> Result<(), ToolProgressReportError> {
        self.inner
            .sender
            .try_send(frame)
            .map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                    ToolProgressReportError::Backpressured
                }
                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                    ToolProgressReportError::Closed
                }
            })
    }

    /// Parse a transport frame at ingress, then report only a valid typed frame.
    pub fn try_report_json(&self, value: serde_json::Value) -> Result<(), ToolProgressReportError> {
        let frame = serde_json::from_value(value)
            .map_err(|error| ToolProgressReportError::Malformed(error.to_string()))?;
        self.try_report(frame)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolProgressReportError {
    #[error("malformed streaming progress frame: {0}")]
    Malformed(String),
    #[error("streaming progress sink is backpressured")]
    Backpressured,
    #[error("streaming progress sink is closed")]
    Closed,
}

struct ToolCancellationInner {
    cancelled: AtomicBool,
    changed: tokio::sync::watch::Sender<bool>,
}

/// Cooperative cancellation signal for one streaming tool dispatch.
#[derive(Clone)]
pub struct ToolCancellationToken {
    inner: Arc<ToolCancellationInner>,
}

impl ToolCancellationToken {
    fn new() -> Self {
        let (changed, _receiver) = tokio::sync::watch::channel(false);
        Self {
            inner: Arc::new(ToolCancellationInner {
                cancelled: AtomicBool::new(false),
                changed,
            }),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut receiver = self.inner.changed.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }

    fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            self.inner.changed.send_replace(true);
        }
    }
}

impl std::fmt::Debug for ToolCancellationToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolCancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl PartialEq for ToolCancellationToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for ToolCancellationToken {}

/// Typed streaming-only portion of [`crate::ToolDispatchContext`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStreamingDispatchContext {
    progress: ToolProgressSink,
    cancellation: ToolCancellationToken,
}

impl ToolStreamingDispatchContext {
    pub const fn progress(&self) -> &ToolProgressSink {
        &self.progress
    }

    pub const fn cancellation(&self) -> &ToolCancellationToken {
        &self.cancellation
    }
}

struct CancelOnDrop {
    token: ToolCancellationToken,
    armed: bool,
}

impl CancelOnDrop {
    fn new(token: ToolCancellationToken) -> Self {
        Self { token, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}

pub(crate) async fn supervise_streaming_tool<T, F, MakeFuture>(
    tool_name: &str,
    inactivity_timeout: Duration,
    absolute_timeout: Duration,
    make_future: MakeFuture,
) -> Result<T, ToolError>
where
    F: Future<Output = Result<T, ToolError>>,
    MakeFuture: FnOnce(ToolStreamingDispatchContext) -> F,
{
    supervise_streaming_tool_with_capacity(
        tool_name,
        inactivity_timeout,
        absolute_timeout,
        32,
        make_future,
    )
    .await
}

async fn supervise_streaming_tool_with_capacity<T, F, MakeFuture>(
    tool_name: &str,
    inactivity_timeout: Duration,
    absolute_timeout: Duration,
    capacity: usize,
    make_future: MakeFuture,
) -> Result<T, ToolError>
where
    F: Future<Output = Result<T, ToolError>>,
    MakeFuture: FnOnce(ToolStreamingDispatchContext) -> F,
{
    let (sender, mut receiver) = tokio::sync::mpsc::channel(capacity.max(1));
    let cancellation = ToolCancellationToken::new();
    let context = ToolStreamingDispatchContext {
        progress: ToolProgressSink {
            inner: Arc::new(ToolProgressSinkInner { sender }),
        },
        cancellation: cancellation.clone(),
    };
    let mut cancel_on_drop = CancelOnDrop::new(cancellation);
    let execution = make_future(context);
    tokio::pin!(execution);
    let inactivity = tokio::time::sleep(inactivity_timeout);
    tokio::pin!(inactivity);
    let absolute = tokio::time::sleep(absolute_timeout);
    tokio::pin!(absolute);
    let mut progress_open = true;

    loop {
        tokio::select! {
            biased;
            result = &mut execution => {
                cancel_on_drop.disarm();
                return result;
            }
            () = &mut absolute => {
                return Err(ToolError::timeout(
                    tool_name,
                    duration_millis(absolute_timeout),
                ));
            }
            frame = receiver.recv(), if progress_open => {
                match frame {
                    Some(_accepted) => {
                        inactivity
                            .as_mut()
                            .set(tokio::time::sleep(inactivity_timeout));
                    }
                    None => progress_open = false,
                }
            }
            () = &mut inactivity => {
                return Err(ToolError::inactivity_timeout(
                    tool_name,
                    duration_millis(inactivity_timeout),
                ));
            }
        }
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone,
    clippy::unwrap_used
)]
mod tests {
    use super::{
        ToolProgressFrame, ToolProgressReportError, supervise_streaming_tool_with_capacity,
    };
    use crate::ToolError;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn message(text: &str) -> ToolProgressFrame {
        ToolProgressFrame::message(text).expect("valid progress frame")
    }

    #[tokio::test(start_paused = true)]
    async fn silent_stream_times_out_and_cancels_exact_context() {
        let captured = Arc::new(Mutex::new(None));
        let captured_for_dispatch = Arc::clone(&captured);
        let task = tokio::spawn(supervise_streaming_tool_with_capacity(
            "scan",
            Duration::from_secs(5),
            Duration::from_secs(30),
            4,
            move |context| {
                *captured_for_dispatch.lock().unwrap() = Some(context.cancellation().clone());
                std::future::pending::<Result<(), ToolError>>()
            },
        ));

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        let error = task
            .await
            .expect("supervisor task joins")
            .expect_err("silent stream must time out");

        assert!(matches!(
            error,
            ToolError::InactivityTimeout {
                inactivity_ms: 5_000,
                ..
            }
        ));
        assert!(
            captured
                .lock()
                .unwrap()
                .as_ref()
                .expect("cancellation captured")
                .is_cancelled()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn accepted_progress_resets_inactivity_but_not_absolute_ceiling() {
        let (context_tx, context_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(supervise_streaming_tool_with_capacity(
            "scan",
            Duration::from_secs(5),
            Duration::from_secs(12),
            4,
            move |context| {
                context_tx.send(context.clone()).unwrap();
                std::future::pending::<Result<(), ToolError>>()
            },
        ));
        let context = context_rx.await.expect("streaming context");

        tokio::time::advance(Duration::from_secs(4)).await;
        context
            .progress()
            .try_report(message("first accepted frame"))
            .expect("progress accepted");
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(!task.is_finished(), "accepted progress resets inactivity");

        context
            .progress()
            .try_report(message("second accepted frame"))
            .expect("progress accepted");
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        assert!(!task.is_finished(), "absolute deadline has not elapsed");

        tokio::time::advance(Duration::from_secs(1)).await;
        let error = task
            .await
            .expect("supervisor task joins")
            .expect_err("absolute ceiling must win despite progress");
        assert!(matches!(
            error,
            ToolError::Timeout {
                timeout_ms: 12_000,
                ..
            }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn malformed_progress_never_reaches_or_resets_the_watchdog() {
        let (context_tx, context_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(supervise_streaming_tool_with_capacity(
            "scan",
            Duration::from_secs(5),
            Duration::from_secs(30),
            1,
            move |context| {
                context_tx.send(context.clone()).unwrap();
                std::future::pending::<Result<(), ToolError>>()
            },
        ));
        let context = context_rx.await.expect("streaming context");

        tokio::time::advance(Duration::from_secs(4)).await;
        assert!(matches!(
            context
                .progress()
                .try_report_json(serde_json::json!({"message": "  "})),
            Err(ToolProgressReportError::Malformed(_))
        ));
        tokio::time::advance(Duration::from_secs(1)).await;

        assert!(matches!(
            task.await
                .expect("supervisor task joins")
                .expect_err("malformed progress must not reset inactivity"),
            ToolError::InactivityTimeout { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn backpressured_progress_is_rejected_and_cannot_extend_liveness() {
        let observed_rejection = Arc::new(AtomicBool::new(false));
        let observed_rejection_for_dispatch = Arc::clone(&observed_rejection);
        let task = tokio::spawn(supervise_streaming_tool_with_capacity(
            "scan",
            Duration::from_secs(5),
            Duration::from_secs(30),
            1,
            move |context| {
                context
                    .progress()
                    .try_report(message("accepted"))
                    .expect("first frame fits");
                observed_rejection_for_dispatch.store(
                    matches!(
                        context.progress().try_report(message("rejected")),
                        Err(ToolProgressReportError::Backpressured)
                    ),
                    Ordering::SeqCst,
                );
                std::future::pending::<Result<(), ToolError>>()
            },
        ));

        tokio::task::yield_now().await;
        assert!(observed_rejection.load(Ordering::SeqCst));
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(matches!(
            task.await
                .expect("supervisor task joins")
                .expect_err("rejected frame must not add another liveness reset"),
            ToolError::InactivityTimeout { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_completion_wins_an_exact_inactivity_deadline_race() {
        let result = supervise_streaming_tool_with_capacity(
            "scan",
            Duration::from_secs(5),
            Duration::from_secs(30),
            1,
            |_context| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<_, ToolError>("complete")
            },
        );
        tokio::pin!(result);

        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(result.await.expect("completion wins tie"), "complete");
    }

    #[tokio::test(start_paused = true)]
    async fn continuous_progress_cannot_starve_the_absolute_ceiling() {
        let (context_tx, context_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(supervise_streaming_tool_with_capacity(
            "scan",
            Duration::from_secs(2),
            Duration::from_secs(5),
            1,
            move |context| {
                context_tx.send(context.clone()).unwrap();
                std::future::pending::<Result<(), ToolError>>()
            },
        ));
        let context = context_rx.await.expect("streaming context");
        let producer = tokio::spawn(async move {
            loop {
                match context.progress().try_report(message("flood")) {
                    Ok(()) | Err(ToolProgressReportError::Backpressured) => {
                        tokio::task::yield_now().await;
                    }
                    Err(ToolProgressReportError::Closed) => break,
                    Err(ToolProgressReportError::Malformed(error)) => {
                        panic!("typed frame became malformed: {error}")
                    }
                }
            }
        });

        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(matches!(
            task.await
                .expect("supervisor task joins")
                .expect_err("absolute ceiling must win under continuous progress"),
            ToolError::Timeout {
                timeout_ms: 5_000,
                ..
            }
        ));
        producer.await.expect("producer observes closed sink");
    }

    #[tokio::test(start_paused = true)]
    async fn outer_timeout_drop_still_cancels_the_streaming_context() {
        let captured = Arc::new(Mutex::new(None));
        let captured_for_dispatch = Arc::clone(&captured);
        let supervised = supervise_streaming_tool_with_capacity(
            "scan",
            Duration::from_secs(20),
            Duration::from_secs(30),
            1,
            move |context| {
                *captured_for_dispatch.lock().unwrap() = Some(context.cancellation().clone());
                std::future::pending::<Result<(), ToolError>>()
            },
        );
        let outer = tokio::spawn(tokio::time::timeout(Duration::from_secs(3), supervised));

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        outer
            .await
            .expect("outer task joins")
            .expect_err("outer deadline expires");
        assert!(
            captured
                .lock()
                .unwrap()
                .as_ref()
                .expect("cancellation captured")
                .is_cancelled(),
            "dropping the supervised future must signal cancellation"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_drops_an_implementation_that_ignores_cancellation() {
        struct DropProbe(Arc<AtomicBool>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let dropped_for_dispatch = Arc::clone(&dropped);
        let task = tokio::spawn(supervise_streaming_tool_with_capacity(
            "scan",
            Duration::from_secs(5),
            Duration::from_secs(30),
            1,
            move |_context| {
                let probe = DropProbe(dropped_for_dispatch);
                async move {
                    let _probe = probe;
                    std::future::pending::<Result<(), ToolError>>().await
                }
            },
        ));

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(task.await.expect("supervisor task joins").is_err());
        assert!(
            dropped.load(Ordering::SeqCst),
            "dropping the supervised future is the final cleanup boundary"
        );
    }
}
