//! Typed stage trace for one console voice readiness check.
//!
//! A readiness check crosses Meta-owned stages (the external-live arbiter,
//! target authorization) and the shared Meerkat probe, which calls back into
//! Meta's binding authority for the durable-source check and binding
//! authorization. When the server budget expires, the route drops the check
//! and logs the active stage with the elapsed time of every stage, so a slow
//! answer names its cause instead of a bare "budget exceeded".
//!
//! The trace travels as a task-local so the binding-authority callbacks,
//! which Meerkat invokes without any Meta context, can advance it. Outside
//! a readiness scope (the open path) every stage call is a no-op.

use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One stage of a console voice readiness check, in execution order.
/// Every stage past the arbiter belongs to the live host (`openai-live`).
#[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum VoiceReadinessStage {
    /// Reading the voice-path holder from the shared live-owner arbiter.
    Arbiter,
    /// Principal and ABAC checks, identity lookup, and the live grant.
    Target,
    /// Meerkat's readiness probe before it reaches a Meta callback.
    Probe,
    /// Meta's body-free, actor-free durable-source observation, called by
    /// the probe.
    DurableSource,
    /// Probe work between the durable source and binding authorization:
    /// the current config and the host identity's binding selection.
    BindingSelection,
    /// Meta's binding-use authorization, called by the probe.
    BindingAuthorization,
    /// Probe work after binding authorization: the typed credential
    /// readiness check (no refresh, no preface, no pending channel).
    Credential,
}

impl VoiceReadinessStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Arbiter => "arbiter",
            Self::Target => "target",
            Self::Probe => "probe",
            Self::DurableSource => "durable_source",
            Self::BindingSelection => "binding_selection",
            Self::BindingAuthorization => "binding_authorization",
            Self::Credential => "credential",
        }
    }
}

impl fmt::Display for VoiceReadinessStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
struct Span {
    stage: VoiceReadinessStage,
    started: Instant,
    finished: Option<Instant>,
}

/// Stage spans for one readiness check. Entering a stage closes the open one.
#[derive(Debug)]
pub(crate) struct VoiceReadinessTrace {
    started: Instant,
    spans: Mutex<Vec<Span>>,
}

impl Default for VoiceReadinessTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceReadinessTrace {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            spans: Mutex::new(Vec::new()),
        }
    }

    fn spans(&self) -> std::sync::MutexGuard<'_, Vec<Span>> {
        self.spans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn enter(&self, stage: VoiceReadinessStage) {
        let now = Instant::now();
        let mut spans = self.spans();
        if let Some(open) = spans.last_mut().filter(|span| span.finished.is_none()) {
            if open.stage == stage {
                return;
            }
            open.finished = Some(now);
        }
        spans.push(Span {
            stage,
            started: now,
            finished: None,
        });
    }

    /// Close the open stage: the check returned.
    pub(crate) fn finish(&self) {
        let now = Instant::now();
        if let Some(open) = self
            .spans()
            .last_mut()
            .filter(|span| span.finished.is_none())
        {
            open.finished = Some(now);
        }
    }

    pub(crate) fn snapshot(&self) -> VoiceReadinessTraceSnapshot {
        let now = Instant::now();
        let spans = self.spans();
        let mut stages: Vec<StageElapsed> = Vec::new();
        for span in spans.iter() {
            let elapsed = span.finished.unwrap_or(now).duration_since(span.started);
            match stages.iter_mut().find(|entry| entry.stage == span.stage) {
                Some(entry) => entry.elapsed += elapsed,
                None => stages.push(StageElapsed {
                    stage: span.stage,
                    elapsed,
                }),
            }
        }
        VoiceReadinessTraceSnapshot {
            active: spans
                .last()
                .filter(|span| span.finished.is_none())
                .map(|span| span.stage),
            elapsed: now.duration_since(self.started),
            stages,
        }
    }
}

/// Accumulated time in one stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StageElapsed {
    pub stage: VoiceReadinessStage,
    pub elapsed: Duration,
}

/// What a readiness check had done when it was observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VoiceReadinessTraceSnapshot {
    /// The stage still running, `None` once the check returned.
    pub active: Option<VoiceReadinessStage>,
    pub elapsed: Duration,
    /// Stages in first-entry order with their accumulated time.
    pub stages: Vec<StageElapsed>,
}

impl VoiceReadinessTraceSnapshot {
    #[cfg(test)]
    pub(crate) fn elapsed_of(&self, stage: VoiceReadinessStage) -> Option<Duration> {
        self.stages
            .iter()
            .find(|entry| entry.stage == stage)
            .map(|entry| entry.elapsed)
    }
}

/// Log form: `target=3ms durable_source=4990ms(active)`.
impl fmt::Display for VoiceReadinessTraceSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, entry) in self.stages.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{}={}ms", entry.stage, entry.elapsed.as_millis())?;
            if index + 1 == self.stages.len() && self.active == Some(entry.stage) {
                f.write_str("(active)")?;
            }
        }
        Ok(())
    }
}

tokio::task_local! {
    static CURRENT: Arc<VoiceReadinessTrace>;
}

/// Run `future` with `trace` as the current readiness trace. The scope is
/// re-established on every poll, whichever task polls the future.
pub(crate) fn scope<F: Future>(
    trace: Arc<VoiceReadinessTrace>,
    future: F,
) -> tokio::task::futures::TaskLocalFuture<Arc<VoiceReadinessTrace>, F> {
    CURRENT.scope(trace, future)
}

/// Advance the current readiness trace, if any.
pub(crate) fn enter(stage: VoiceReadinessStage) {
    let _ = CURRENT.try_with(|trace| trace.enter(stage));
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn entering_a_stage_closes_the_open_one_and_repeats_accumulate() {
        let trace = VoiceReadinessTrace::new();
        trace.enter(VoiceReadinessStage::Target);
        std::thread::sleep(Duration::from_millis(5));
        trace.enter(VoiceReadinessStage::Probe);
        trace.enter(VoiceReadinessStage::Probe);
        trace.enter(VoiceReadinessStage::DurableSource);
        std::thread::sleep(Duration::from_millis(5));
        let running = trace.snapshot();
        assert_eq!(running.active, Some(VoiceReadinessStage::DurableSource));
        assert_eq!(
            running
                .stages
                .iter()
                .map(|entry| entry.stage)
                .collect::<Vec<_>>(),
            vec![
                VoiceReadinessStage::Target,
                VoiceReadinessStage::Probe,
                VoiceReadinessStage::DurableSource,
            ],
            "a repeated enter of the open stage is one span"
        );
        assert!(
            running
                .elapsed_of(VoiceReadinessStage::Target)
                .expect("target")
                >= Duration::from_millis(5)
        );
        assert!(running.to_string().ends_with("(active)"), "{running}");
        assert!(running.to_string().starts_with("target="), "{running}");
        trace.finish();
        let done = trace.snapshot();
        assert_eq!(done.active, None);
        assert!(!done.to_string().contains("(active)"), "{done}");
    }

    #[tokio::test]
    async fn stage_calls_reach_the_scoped_trace_only() {
        enter(VoiceReadinessStage::Target);
        let trace = Arc::new(VoiceReadinessTrace::new());
        scope(Arc::clone(&trace), async {
            enter(VoiceReadinessStage::Target);
            tokio::task::yield_now().await;
            enter(VoiceReadinessStage::DurableSource);
        })
        .await;
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.active, Some(VoiceReadinessStage::DurableSource));
        assert_eq!(snapshot.stages.len(), 2);
    }
}
