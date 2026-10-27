//! The observation barrier of a fenced entry: awaited after the decision is
//! prepared, any review admitted and the root entry staged, and before any
//! dispatcher code runs. A failed barrier refuses the call before its
//! dispatcher runs, a decision prepared again during the wait is staged and
//! made durable before the call enters with it, a dispatch waiting in its
//! barrier holds nothing another dispatch needs, and a context without a
//! barrier is dispatched as before.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::approval::review::{
    BoundOperationReview, OperationReviewer, ReviewCandidate, ReviewEntrySupport, ReviewVerdict,
    ReviewerFailure,
};
use crate::authorization::{
    OperationAuthorizationError, OperationObservation, OperationObservationBarrier,
    OperationObservationError, OperationRefusalKind, OperationRefused, OperationReviewTier,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
};
use crate::types::ToolResult;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// What happened, in order: the work owner's decisions, what its sink
/// staged, the barrier's waits, the reviewer and the dispatcher code.
type Events = Arc<Mutex<Vec<String>>>;

fn push(events: &Events, event: impl Into<String>) {
    events.lock().unwrap().push(event.into());
}

fn events_of(events: &Events) -> Vec<String> {
    events.lock().unwrap().clone()
}

/// A work owner whose decisions stay current while its owner sequence is
/// the one they were prepared at. It records each decision, and each
/// observation its sink stages, with that sequence.
struct Owner {
    sequence: Arc<AtomicU64>,
    tier: OperationReviewTier,
    refuse: bool,
    events: Events,
}

struct Decision {
    at: u64,
    sequence: Arc<AtomicU64>,
    tier: OperationReviewTier,
    events: Events,
}

impl WorkAuthorization for Owner {
    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        let at = self.sequence.load(Ordering::SeqCst);
        if self.refuse {
            push(&self.events, format!("refused@{at}"));
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        push(&self.events, format!("prepared@{at}"));
        Ok(Arc::new(Decision {
            at,
            sequence: Arc::clone(&self.sequence),
            tier: self.tier,
            events: Arc::clone(&self.events),
        }))
    }

    fn review_operation_context(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<crate::authorization::ReviewOperationContextResolution, OperationAuthorizationError>
    {
        Ok(crate::authorization::ReviewOperationContextResolution::NotApplicable)
    }
}

impl PreparedOperationAuthorization for Decision {
    fn review_tier(&self) -> OperationReviewTier {
        self.tier
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        if self.sequence.load(Ordering::SeqCst) == self.at {
            Ok(())
        } else {
            Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into())
        }
    }

    fn observe(
        &self,
        _binding: &PreparedAuthorizationBinding,
        observation: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        let at = self.at;
        let event = match observation {
            OperationObservation::ReviewAttemptStarted { .. } => format!("review_attempt@{at}"),
            OperationObservation::Entry(boundary) => format!("entry:{boundary:?}@{at}"),
            OperationObservation::Outcome(_) => format!("outcome@{at}"),
            OperationObservation::Refused(kind) => format!("refused:{kind:?}@{at}"),
            OperationObservation::AuthorizationUnavailable => format!("unavailable@{at}"),
        };
        push(&self.events, event);
        Ok(())
    }
}

/// A barrier that records each wait and what it made durable (everything
/// recorded before it), and fails, completes owner changes or parks as
/// scripted.
struct Barrier {
    events: Events,
    fail: bool,
    /// Owner changes still to complete, one during each wait.
    changes: AtomicUsize,
    sequence: Arc<AtomicU64>,
    /// When set, each wait signals `entered` and parks until `release`.
    park: Option<Park>,
}

struct Park {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl OperationObservationBarrier for Barrier {
    async fn before_entry(&self) -> Result<(), OperationObservationError> {
        push(&self.events, "barrier");
        if let Some(park) = &self.park {
            park.entered.notify_one();
            park.release.notified().await;
        }
        tokio::task::yield_now().await;
        if self
            .changes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            // An owner change completes while the barrier waits.
            self.sequence.fetch_add(1, Ordering::SeqCst);
        }
        if self.fail {
            return Err(OperationObservationError);
        }
        push(&self.events, "durable");
        Ok(())
    }
}

/// A model reviewer that allows every candidate and records its review.
struct Allow(Events);

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl OperationReviewer for Allow {
    async fn review(
        &self,
        _candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        push(&self.0, "review");
        Ok(ReviewVerdict::Allow)
    }
}

/// One fast tool whose body records that dispatcher code ran; a reviewed
/// one enters its reviewed leaf first.
struct Tool {
    label: &'static str,
    reviewed: bool,
    events: Events,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for Tool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::new(ToolDef::new(
            "tool",
            "fixture tool",
            serde_json::json!({"type": "object"}),
        ))])
    }

    fn review_entry_support(&self, _tool_name: &str) -> ReviewEntrySupport {
        if self.reviewed {
            ReviewEntrySupport::ConsumesAtEntry
        } else {
            ReviewEntrySupport::Unsupported
        }
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        push(&self.events, self.label);
        Ok(ToolResult::new(call.id.to_string(), "ran".to_string(), false).into())
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &crate::ResolvedToolExecutionPlan,
    ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
        if self.reviewed {
            context.enter_reviewed_effect(call, Some(plan))?;
        }
        self.dispatch(call).await
    }
}

struct Fixture {
    events: Events,
    sequence: Arc<AtomicU64>,
    work: crate::WorkAuthorizationContext,
}

fn fixture(tier: OperationReviewTier, refuse: bool) -> Fixture {
    let events = Events::default();
    let sequence = Arc::new(AtomicU64::new(0));
    let work = crate::WorkAuthorizationContext::new(
        Arc::new(Owner {
            sequence: Arc::clone(&sequence),
            tier,
            refuse,
            events: Arc::clone(&events),
        }),
        crate::OperationExecutionScope::Domain,
    );
    Fixture {
        events,
        sequence,
        work,
    }
}

impl Fixture {
    fn context(&self) -> ToolDispatchContext {
        ToolDispatchContext::default().with_work_authorization(Some(self.work.clone()))
    }

    fn barrier(&self, fail: bool, changes: usize, park: bool) -> Arc<Barrier> {
        Arc::new(Barrier {
            events: Arc::clone(&self.events),
            fail,
            changes: AtomicUsize::new(changes),
            sequence: Arc::clone(&self.sequence),
            park: park.then(|| Park {
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            }),
        })
    }

    fn with_barrier(&self, barrier: &Arc<Barrier>) -> ToolDispatchContext {
        self.context()
            .with_observation_barrier(Arc::clone(barrier) as Arc<dyn OperationObservationBarrier>)
    }

    fn tool(&self, label: &'static str, reviewed: bool) -> Arc<Tool> {
        Arc::new(Tool {
            label,
            reviewed,
            events: Arc::clone(&self.events),
        })
    }
}

/// One real fenced resolution and dispatch of the tool's call.
async fn fenced_call(
    tool: &Arc<Tool>,
    context: &ToolDispatchContext,
) -> Result<crate::ops::ToolDispatchOutcome, crate::ToolError> {
    let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw arguments");
    let call = ToolCallView {
        id: "call-1",
        name: "tool",
        args: &args,
    };
    let resolution = crate::ToolExecutionResolutionContext::new(
        crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
            crate::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_mins(1),
        )])
        .expect("deadline chain"),
    );
    let plan = resolve_tool_execution_plan_fenced(tool, call, context, &resolution)
        .expect("the tool's plan");
    dispatch_tool_execution_plan_fenced(tool, call, context, &plan).await
}

fn strings(events: &[&str]) -> Vec<String> {
    events.iter().map(|event| (*event).to_string()).collect()
}

/// The barrier is awaited after the decision is prepared and the root entry
/// staged, and its wait completes before any dispatcher code runs.
#[tokio::test]
async fn the_barrier_completes_after_the_root_entry_and_before_any_dispatcher_code() {
    let fixture = fixture(OperationReviewTier::R1, false);
    let barrier = fixture.barrier(false, 0, false);
    let result = fenced_call(
        &fixture.tool("dispatcher", false),
        &fixture.with_barrier(&barrier),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        events_of(&fixture.events),
        strings(&[
            "prepared@0",
            "entry:ToolDispatch@0",
            "barrier",
            "durable",
            "dispatcher",
            "outcome@0",
        ])
    );
}

/// A failed barrier refuses the call before any dispatcher code runs, with
/// `OperationObservationUnavailable`, and nothing is entered or observed
/// after it.
#[tokio::test]
async fn a_failed_barrier_refuses_the_call_before_any_dispatcher_code_runs() {
    let fixture = fixture(OperationReviewTier::R1, false);
    let barrier = fixture.barrier(true, 0, false);
    let result = fenced_call(
        &fixture.tool("dispatcher", false),
        &fixture.with_barrier(&barrier),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(crate::ToolError::OperationObservationUnavailable)
        ),
        "{result:?}"
    );
    assert_eq!(
        events_of(&fixture.events),
        strings(&["prepared@0", "entry:ToolDispatch@0", "barrier"])
    );
}

/// A decision the owner prepares again while the barrier waits is staged
/// with a new root entry and made durable by another wait before the call
/// enters with it: what the last wait made durable carries the decision the
/// call entered with.
#[tokio::test]
async fn a_decision_prepared_again_during_the_wait_is_made_durable_before_the_call_enters() {
    let fixture = fixture(OperationReviewTier::R1, false);
    let barrier = fixture.barrier(false, 1, false);
    let result = fenced_call(
        &fixture.tool("dispatcher", false),
        &fixture.with_barrier(&barrier),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        events_of(&fixture.events),
        strings(&[
            "prepared@0",
            "entry:ToolDispatch@0",
            "barrier",
            "durable",
            "prepared@1",
            "entry:ToolDispatch@1",
            "barrier",
            "durable",
            "dispatcher",
            "outcome@1",
        ])
    );
}

/// An owner that changes during every wait is refused after the bounded
/// rounds: the refusal of the last decision is staged, and no dispatcher
/// code runs.
#[tokio::test]
async fn an_owner_changing_during_every_wait_is_refused_after_bounded_rounds() {
    let fixture = fixture(OperationReviewTier::R1, false);
    let barrier = fixture.barrier(false, usize::MAX, false);
    let result = fenced_call(
        &fixture.tool("dispatcher", false),
        &fixture.with_barrier(&barrier),
    )
    .await;
    assert!(
        matches!(
            &result,
            Err(crate::ToolError::AuthorizationRefused { refusal })
                if refusal.kind() == OperationRefusalKind::ReprepareRequired
        ),
        "{result:?}"
    );
    let events = events_of(&fixture.events);
    assert_eq!(
        events.iter().filter(|event| *event == "barrier").count(),
        OBSERVATION_BARRIER_ROUNDS
    );
    assert_eq!(
        events.last().map(String::as_str),
        Some("refused:ReprepareRequired@3"),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|event| event == "dispatcher"),
        "{events:?}"
    );
}

/// The barrier follows the review admission: a reviewed operation is
/// reviewed, its root entry staged and the barrier awaited, and only then
/// does the dispatcher's reviewed leaf enter.
#[tokio::test]
async fn the_barrier_follows_the_review_admission() {
    let fixture = fixture(OperationReviewTier::R2, false);
    let barrier = fixture.barrier(false, 0, false);
    let context = fixture
        .with_barrier(&barrier)
        .with_operation_review(Some(Arc::new(BoundOperationReview::new(
            Arc::new(Allow(Arc::clone(&fixture.events))),
            crate::approval::ApprovalService::new(),
            std::time::Duration::from_secs(30),
        ))));
    let result = fenced_call(&fixture.tool("dispatcher", true), &context).await;
    assert!(result.is_ok(), "{result:?}");
    let events = events_of(&fixture.events);
    let at = |event: &str| {
        events
            .iter()
            .position(|seen| seen == event)
            .unwrap_or_else(|| panic!("{event} in {events:?}"))
    };
    assert!(at("review") < at("entry:ToolDispatch@0"), "{events:?}");
    assert!(at("entry:ToolDispatch@0") < at("barrier"), "{events:?}");
    assert!(at("durable") < at("entry:ToolEffect@0"), "{events:?}");
    assert!(at("entry:ToolEffect@0") < at("dispatcher"), "{events:?}");
    assert_eq!(
        events.iter().filter(|event| *event == "barrier").count(),
        1,
        "{events:?}"
    );
}

/// A refused decision never reaches the barrier: nothing is entered.
#[tokio::test]
async fn a_refused_decision_never_reaches_the_barrier() {
    let fixture = fixture(OperationReviewTier::R1, true);
    let barrier = fixture.barrier(false, 0, false);
    let result = fenced_call(
        &fixture.tool("dispatcher", false),
        &fixture.with_barrier(&barrier),
    )
    .await;
    assert!(
        matches!(
            &result,
            Err(crate::ToolError::AuthorizationRefused { refusal })
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "{result:?}"
    );
    assert_eq!(events_of(&fixture.events), strings(&["refused@0"]));
}

/// A context without a barrier is dispatched as before: the same decision,
/// entry, dispatcher code and outcome, with no wait. A context with a
/// barrier differs from one without only by it.
#[tokio::test]
async fn a_context_without_a_barrier_is_dispatched_as_before() {
    let fixture = fixture(OperationReviewTier::R1, false);
    let result = fenced_call(&fixture.tool("dispatcher", false), &fixture.context()).await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(
        events_of(&fixture.events),
        strings(&[
            "prepared@0",
            "entry:ToolDispatch@0",
            "dispatcher",
            "outcome@0"
        ])
    );

    let plain = fixture.context();
    let barrier = fixture.barrier(false, 0, false);
    assert!(plain == fixture.context(), "equal without a barrier");
    assert!(
        plain != fixture.with_barrier(&barrier),
        "a barrier is part of the context"
    );
    assert!(
        fixture.with_barrier(&barrier) == fixture.with_barrier(&barrier),
        "the same barrier"
    );
}

/// A dispatch waiting in its barrier holds nothing another dispatch needs:
/// while it waits, other dispatches of the same work, with no barrier and
/// with a barrier of their own, prepare, enter and run their dispatcher
/// code, and the waiting one completes once its barrier is released.
#[tokio::test]
async fn a_dispatch_waiting_in_its_barrier_blocks_no_other_dispatch_of_the_same_work() {
    let fixture = fixture(OperationReviewTier::R1, false);
    let slow = fixture.barrier(false, 0, true);
    let own = fixture.barrier(false, 0, false);
    let waiting_context = fixture.with_barrier(&slow);
    let unbarriered = fixture.context();
    let barriered = fixture.with_barrier(&own);
    let waiting_tool = fixture.tool("waiting", false);
    let unbarriered_tool = fixture.tool("unbarriered", false);
    let barriered_tool = fixture.tool("barriered", false);
    let park = slow.park.as_ref().expect("a parked barrier");

    let waiting = fenced_call(&waiting_tool, &waiting_context);
    let others = async {
        park.entered.notified().await;
        let unbarriered = fenced_call(&unbarriered_tool, &unbarriered).await;
        let barriered = fenced_call(&barriered_tool, &barriered).await;
        park.release.notify_one();
        (unbarriered, barriered)
    };
    let (waiting, (unbarriered, barriered)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(waiting, others)
        })
        .await
        .expect("no dispatch waited for another");
    assert!(waiting.is_ok(), "{waiting:?}");
    assert!(unbarriered.is_ok(), "{unbarriered:?}");
    assert!(barriered.is_ok(), "{barriered:?}");
    let events = events_of(&fixture.events);
    let at = |event: &str| {
        events
            .iter()
            .position(|seen| seen == event)
            .unwrap_or_else(|| panic!("{event} in {events:?}"))
    };
    assert!(at("unbarriered") < at("waiting"), "{events:?}");
    assert!(at("barriered") < at("waiting"), "{events:?}");
}
