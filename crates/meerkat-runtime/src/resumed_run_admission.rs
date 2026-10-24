//! Host admission of a resumed run.
//!
//! A durable session that stopped inside a run keeps that run open: on the
//! next open its runtime resumes the run from the session's run cursor
//! instead of running the run's inputs again. Before it does, the runtime
//! asks the admission bound to the session's registration, once per resume,
//! whether the run's work is still wanted. A host that tracks work outside
//! the session (a work item claim, for example) answers that a run whose work
//! moved on is superseded, and the runtime then settles the run the way a
//! stop would, without running it.
//!
//! The admission is optional. A registration without one resumes its run.

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;

use crate::identifiers::LogicalRuntimeId;

/// Attempts the runtime makes while an admission answers
/// [`ResumedRunAdmissionDecision::Unavailable`] or does not answer in time.
pub const RESUMED_RUN_ADMISSION_ATTEMPTS: u32 = 3;

/// How long one attempt may take. An attempt that takes longer counts as
/// [`ResumedRunAdmissionDecision::Unavailable`].
pub const RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(8);

/// The pause between two attempts. With the attempt timeout, the runtime
/// settles an admission that stays unavailable after about 30 seconds.
pub const RESUMED_RUN_ADMISSION_RETRY_DELAY: Duration = Duration::from_secs(3);

/// What the runtime asks before it resumes a run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResumedRunAdmissionRequest {
    /// The runtime that holds the run.
    pub runtime_id: LogicalRuntimeId,
    /// The session the run belongs to.
    pub session_id: SessionId,
    /// The run the runtime would resume, under its original id.
    pub run_id: RunId,
    /// Every input bound to the run as of its last committed round.
    pub bound_input_ids: Vec<InputId>,
    /// The turn tool overlay the resumed run executes under: the one its
    /// bound inputs carry, merged as their run merges it. `None` when they
    /// carry none.
    pub turn_tool_overlay: Option<meerkat_core::TurnToolOverlay>,
}

impl ResumedRunAdmissionRequest {
    #[must_use]
    pub fn new(
        runtime_id: LogicalRuntimeId,
        session_id: SessionId,
        run_id: RunId,
        bound_input_ids: Vec<InputId>,
    ) -> Self {
        Self {
            runtime_id,
            session_id,
            run_id,
            bound_input_ids,
            turn_tool_overlay: None,
        }
    }

    /// The same request with the overlay the resumed run executes under.
    #[must_use]
    pub fn with_turn_tool_overlay(
        mut self,
        overlay: Option<meerkat_core::TurnToolOverlay>,
    ) -> Self {
        self.turn_tool_overlay = overlay;
        self
    }
}

/// The admission's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResumedRunAdmissionDecision {
    /// Resume the run.
    Continue,
    /// The run's work moved on. The runtime settles the run at once, like a
    /// stop: its bound inputs are abandoned as refused resumes and the run
    /// ends failed. It is never resumed.
    Superseded {
        /// Why, for the run's terminal record.
        detail: String,
    },
    /// The admission cannot decide now, for example because the record it
    /// reads is unreachable. The runtime asks again, up to
    /// [`RESUMED_RUN_ADMISSION_ATTEMPTS`] times, and then settles the run as
    /// it settles a superseded one. An admission that cannot decide never
    /// lets the run resume.
    Unavailable {
        /// Why, for the run's terminal record when the attempts run out.
        detail: String,
    },
}

/// Host admission of a resumed run, bound to one session registration with
/// [`crate::MeerkatMachine::bind_resumed_run_admission`].
///
/// For each resume the runtime asks once per attempt, up to
/// [`RESUMED_RUN_ADMISSION_ATTEMPTS`] attempts, after the session's executor
/// is attached and before the run is authorized again, never while it holds
/// the session's queue. An attempt that runs past
/// [`RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT`] is cancelled (its future is
/// dropped) and counts as unavailable, so an implementation must be
/// cancellation-safe, and idempotent across the attempts for one resume. It
/// answers from its own records and maps its own failures to
/// [`ResumedRunAdmissionDecision::Unavailable`].
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait ResumedRunAdmission: Send + Sync {
    async fn admit_resumed_run(
        &self,
        request: &ResumedRunAdmissionRequest,
    ) -> ResumedRunAdmissionDecision;
}

/// The runtime's final verdict for one resume after it asked the admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResumedRunAdmissionVerdict {
    Continue,
    /// Settle the run as a refused resume, with this detail.
    Refused {
        detail: String,
    },
}

/// Ask `admission` about `request` within the bounded attempts.
///
/// No admission resumes the run. `Superseded` settles it at once. An
/// admission that stays unavailable, or does not answer in time, through
/// every attempt settles it too, with the last reason it gave.
pub(crate) async fn ask_resumed_run_admission(
    admission: Option<&Arc<dyn ResumedRunAdmission>>,
    request: &ResumedRunAdmissionRequest,
) -> ResumedRunAdmissionVerdict {
    let Some(admission) = admission else {
        return ResumedRunAdmissionVerdict::Continue;
    };
    let mut last_detail = String::new();
    for attempt in 1..=RESUMED_RUN_ADMISSION_ATTEMPTS {
        let answer = crate::tokio::time::timeout(
            RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT,
            admission.admit_resumed_run(request),
        )
        .await;
        match answer {
            Ok(ResumedRunAdmissionDecision::Continue) => {
                return ResumedRunAdmissionVerdict::Continue;
            }
            Ok(ResumedRunAdmissionDecision::Superseded { detail }) => {
                return ResumedRunAdmissionVerdict::Refused {
                    detail: format!("the resumed run was superseded: {detail}"),
                };
            }
            Ok(ResumedRunAdmissionDecision::Unavailable { detail }) => {
                last_detail = detail;
            }
            Err(_elapsed) => {
                last_detail = format!(
                    "the admission did not answer within {RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT:?}"
                );
            }
        }
        tracing::warn!(
            session_id = %request.session_id,
            run_id = %request.run_id,
            attempt,
            detail = %last_detail,
            "resumed run admission is unavailable"
        );
        if attempt < RESUMED_RUN_ADMISSION_ATTEMPTS {
            crate::tokio::time::sleep(RESUMED_RUN_ADMISSION_RETRY_DELAY).await;
        }
    }
    ResumedRunAdmissionVerdict::Refused {
        detail: format!(
            "the resumed run admission stayed unavailable after {RESUMED_RUN_ADMISSION_ATTEMPTS} attempts: {last_detail}"
        ),
    }
}

/// One registration's admission slot. Every registration owns a fresh, empty
/// slot; a replaced or reopened registration therefore starts without one.
#[derive(Clone, Default)]
pub(crate) struct ResumedRunAdmissionSlot(Arc<std::sync::OnceLock<Arc<dyn ResumedRunAdmission>>>);

impl std::fmt::Debug for ResumedRunAdmissionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumedRunAdmissionSlot")
            .field("bound", &self.0.get().is_some())
            .finish()
    }
}

impl ResumedRunAdmissionSlot {
    /// Bind `admission`. Binding the same admission again is accepted;
    /// another one is refused, because the slot is fixed for its
    /// registration's lifetime.
    pub(crate) fn bind(
        &self,
        admission: Arc<dyn ResumedRunAdmission>,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        let bound = self.0.get_or_init(|| Arc::clone(&admission));
        if Arc::ptr_eq(bound, &admission) {
            Ok(())
        } else {
            Err(crate::traits::RuntimeDriverError::ValidationFailed {
                reason:
                    "the session registration is already bound to another resumed run admission"
                        .to_string(),
            })
        }
    }

    pub(crate) fn current(&self) -> Option<Arc<dyn ResumedRunAdmission>> {
        self.0.get().cloned()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Scripted {
        answers: Mutex<Vec<ResumedRunAdmissionDecision>>,
        calls: Mutex<u32>,
    }

    impl Scripted {
        fn new(mut answers: Vec<ResumedRunAdmissionDecision>) -> Arc<Self> {
            answers.reverse();
            Arc::new(Self {
                answers: Mutex::new(answers),
                calls: Mutex::new(0),
            })
        }

        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl ResumedRunAdmission for Scripted {
        async fn admit_resumed_run(
            &self,
            _request: &ResumedRunAdmissionRequest,
        ) -> ResumedRunAdmissionDecision {
            *self.calls.lock().unwrap() += 1;
            self.answers
                .lock()
                .unwrap()
                .pop()
                .expect("the runtime asked more often than scripted")
        }
    }

    struct NeverAnswers;

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl ResumedRunAdmission for NeverAnswers {
        async fn admit_resumed_run(
            &self,
            _request: &ResumedRunAdmissionRequest,
        ) -> ResumedRunAdmissionDecision {
            std::future::pending().await
        }
    }

    fn request() -> ResumedRunAdmissionRequest {
        let session_id = SessionId::new();
        ResumedRunAdmissionRequest::new(
            LogicalRuntimeId::for_session(&session_id),
            session_id,
            RunId::new(),
            vec![InputId::new()],
        )
    }

    #[tokio::test]
    async fn no_admission_resumes_the_run() {
        assert_eq!(
            ask_resumed_run_admission(None, &request()).await,
            ResumedRunAdmissionVerdict::Continue
        );
    }

    #[tokio::test]
    async fn superseded_settles_at_once_without_asking_again() {
        let scripted = Scripted::new(vec![ResumedRunAdmissionDecision::Superseded {
            detail: "claim moved".into(),
        }]);
        let admission: Arc<dyn ResumedRunAdmission> = scripted.clone();
        let verdict = ask_resumed_run_admission(Some(&admission), &request()).await;
        assert!(
            matches!(&verdict, ResumedRunAdmissionVerdict::Refused { detail } if detail.contains("claim moved")),
            "{verdict:?}"
        );
        assert_eq!(scripted.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_is_asked_again_and_continue_on_a_later_attempt_resumes() {
        let scripted = Scripted::new(vec![
            ResumedRunAdmissionDecision::Unavailable {
                detail: "store unreachable".into(),
            },
            ResumedRunAdmissionDecision::Continue,
        ]);
        let admission: Arc<dyn ResumedRunAdmission> = scripted.clone();
        assert_eq!(
            ask_resumed_run_admission(Some(&admission), &request()).await,
            ResumedRunAdmissionVerdict::Continue
        );
        assert_eq!(scripted.calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_through_every_attempt_settles_with_the_last_reason() {
        let scripted = Scripted::new(vec![
            ResumedRunAdmissionDecision::Unavailable {
                detail: "first".into(),
            },
            ResumedRunAdmissionDecision::Unavailable {
                detail: "second".into(),
            },
            ResumedRunAdmissionDecision::Unavailable {
                detail: "third".into(),
            },
        ]);
        let admission: Arc<dyn ResumedRunAdmission> = scripted.clone();
        let verdict = ask_resumed_run_admission(Some(&admission), &request()).await;
        assert!(
            matches!(&verdict, ResumedRunAdmissionVerdict::Refused { detail } if detail.contains("third")),
            "{verdict:?}"
        );
        assert_eq!(scripted.calls(), RESUMED_RUN_ADMISSION_ATTEMPTS);
    }

    #[tokio::test(start_paused = true)]
    async fn an_admission_that_never_answers_is_settled_after_the_bounded_attempts() {
        let admission: Arc<dyn ResumedRunAdmission> = Arc::new(NeverAnswers);
        let started = crate::tokio::time::Instant::now();
        let verdict = ask_resumed_run_admission(Some(&admission), &request()).await;
        assert!(
            matches!(&verdict, ResumedRunAdmissionVerdict::Refused { detail } if detail.contains("did not answer")),
            "{verdict:?}"
        );
        let bound = RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT * RESUMED_RUN_ADMISSION_ATTEMPTS
            + RESUMED_RUN_ADMISSION_RETRY_DELAY * (RESUMED_RUN_ADMISSION_ATTEMPTS - 1);
        assert_eq!(started.elapsed(), bound);
    }

    #[test]
    fn a_slot_keeps_its_first_admission_and_refuses_another() {
        let slot = ResumedRunAdmissionSlot::default();
        assert!(slot.current().is_none());
        let first: Arc<dyn ResumedRunAdmission> = Arc::new(NeverAnswers);
        slot.bind(Arc::clone(&first)).expect("first bind");
        slot.bind(Arc::clone(&first))
            .expect("binding the same admission again is accepted");
        let other: Arc<dyn ResumedRunAdmission> = Arc::new(NeverAnswers);
        assert!(slot.bind(other).is_err());
        assert!(Arc::ptr_eq(&slot.current().expect("bound"), &first));
    }
}
