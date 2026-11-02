//! Host-only observation of operation refusal dispositions.
//!
//! A refused or unavailable governed operation reaches the model and the run
//! only as a uniform, private-fact-free notice. The Rust host that composed
//! the agent and owns its work authorization may install an
//! [`OperationRefusalObserver`] to see which typed refusal Core met, on which
//! surface, and what Core did with it. The observation never enters the agent
//! event stream, a wire type, a config type or any model context: only the
//! host that installed the observer receives it.

use std::sync::Arc;

use super::{OperationRefusalKind, OperationRefused, OwnerChangeWait, RefusalDispatch};
use crate::lifecycle::RunId;
use crate::ops::OperationId;
use crate::types::SessionId;

/// Receives each operation refusal disposition Core decides, for the host
/// that installed it (`AgentBuilder::with_operation_refusal_observer`, or the
/// facade's agent factory).
///
/// It sees the agent's own model requests, tool calls (a decided consent's
/// retained attempt included, once the attempt has resolved its candidate)
/// and compaction summaries, and the owner-change waits of a tool's delegate
/// operations that use [`crate::ToolDispatchContext::owner_change_churn`].
/// Exact-control refusals (MCP attachments, tool applications, context
/// controls), a consent attempt refused before it resolved its candidate, and
/// review outcomes, which are not operation refusals, are not observed.
///
/// The contract:
/// - An observation is a diagnostic emitted when Core decides a disposition,
///   before any effect of it is durable. It is not exactly-once across a
///   crash, and an operation reported as waiting or rebuilding may still
///   fail afterwards.
/// - `observe` is called synchronously on the task that decided it, and it
///   must not block: a host hands the observation off to its own channel.
///   Core holds no lock shared with another operation while it calls it; a
///   tool call's wait is reported while that dispatch attempt holds its own
///   waiting state, which nothing else takes.
/// - A panicking observer is a host bug, and the panic propagates.
pub trait OperationRefusalObserver: Send + Sync {
    fn observe(&self, observation: &OperationRefusalObservation);
}

/// Where the refused or unavailable operation sat.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationRefusalSurface {
    /// The agent loop's model request.
    ModelRequest,
    /// One tool call, by its provider tool-use id: the call's own operation,
    /// or a delegate operation of it (such as the `web_search` helper's model
    /// request), which the observation's operation id tells apart.
    ToolCall { tool_use_id: String },
    /// A compaction summary request.
    Compaction,
}

/// What the operation's owners answered.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationRefusalOutcome {
    /// Refused with its closed kind, and whether the guarded tool code ran,
    /// read from the same refusal value a model payload renders.
    Refused {
        kind: OperationRefusalKind,
        dispatch: RefusalDispatch,
    },
    /// The owners could not decide: no verdict.
    AuthorityUnavailable,
}

impl From<OperationRefused> for OperationRefusalOutcome {
    fn from(refusal: OperationRefused) -> Self {
        Self::Refused {
            kind: refusal.kind(),
            dispatch: refusal.dispatch(),
        }
    }
}

impl OperationRefusalOutcome {
    /// The outcome a tool error carries: an operation refusal (also beneath
    /// settlement failures), read from the value its model payload renders,
    /// or owners that could not decide. `None` for every other error.
    pub(crate) fn of_tool_error(error: &crate::ToolError) -> Option<Self> {
        match error.primary_error() {
            crate::ToolError::AuthorizationRefused { refusal } => Some((*refusal).into()),
            crate::ToolError::OperationAuthorizationUnavailable => Some(Self::AuthorityUnavailable),
            _ => None,
        }
    }

    /// The outcome an agent error carries, as for a tool error.
    pub(crate) fn of_agent_error(error: &crate::AgentError) -> Option<Self> {
        match error.operation_refusal() {
            Some(refusal) => Some(refusal.into()),
            None => error
                .operation_authorization_unavailable()
                .then_some(Self::AuthorityUnavailable),
        }
    }
}

/// What Core did with the refusal.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationRefusalDisposition {
    /// Waited for an owner change in progress on the work's own publication
    /// (bounded): for `waited_ms`, and whether the change settled so the
    /// operation may rebuild.
    WaitedForOwnerChange { waited_ms: u64, settled: bool },
    /// Rebuilt the model request once from current owners.
    Rebuilt,
    /// Sent the controller feedback request instead.
    ControllerFeedback,
    /// Returned the refusal to the model as the tool call's result.
    ToolResult,
    /// Skipped the compaction and kept the history.
    CompactionSkipped,
    /// Failed the run.
    RunFailed,
}

/// One operation refusal disposition Core decided. Only Core builds one.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRefusalObservation {
    session_id: SessionId,
    run_id: Option<RunId>,
    operation_id: Option<OperationId>,
    surface: OperationRefusalSurface,
    outcome: OperationRefusalOutcome,
    disposition: OperationRefusalDisposition,
}

impl OperationRefusalObservation {
    /// The session whose agent decided it.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The run it belongs to, when the operation was prepared under one.
    pub fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    /// The refused operation's id, as its authorization audit records it;
    /// `None` for an operation outside governed work, and for the waits of
    /// a governed helper admission built from a dispatch
    /// (`GovernedHelperAdmissionFence::create_idempotent_after_owner_change`
    /// in the `meerkat` facade), which it reports on its call's surface
    /// without the dispatch's operation id.
    pub fn operation_id(&self) -> Option<&OperationId> {
        self.operation_id.as_ref()
    }

    pub fn surface(&self) -> &OperationRefusalSurface {
        &self.surface
    }

    pub fn outcome(&self) -> OperationRefusalOutcome {
        self.outcome
    }

    pub fn disposition(&self) -> OperationRefusalDisposition {
        self.disposition
    }
}

/// An installed observer with the session it reports for.
#[derive(Clone)]
pub(crate) struct RefusalObserverBinding {
    observer: Arc<dyn OperationRefusalObserver>,
    session_id: SessionId,
}

impl std::fmt::Debug for RefusalObserverBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RefusalObserverBinding")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl RefusalObserverBinding {
    pub(crate) fn new(observer: Arc<dyn OperationRefusalObserver>, session_id: SessionId) -> Self {
        Self {
            observer,
            session_id,
        }
    }

    /// Whether both report to the same observer for the same session.
    pub(crate) fn same_binding(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.observer, &other.observer) && self.session_id == other.session_id
    }

    pub(crate) fn observe(
        &self,
        run_id: Option<RunId>,
        operation_id: Option<OperationId>,
        surface: OperationRefusalSurface,
        outcome: OperationRefusalOutcome,
        disposition: OperationRefusalDisposition,
    ) {
        self.observer.observe(&OperationRefusalObservation {
            session_id: self.session_id.clone(),
            run_id,
            operation_id,
            surface,
            outcome,
            disposition,
        });
    }

    /// Where one operation's owner-change waits on `surface` are reported.
    pub(crate) fn wait_report(
        &self,
        run_id: Option<RunId>,
        surface: OperationRefusalSurface,
    ) -> WaitReport {
        WaitReport {
            binding: self.clone(),
            run_id,
            surface,
        }
    }
}

/// Where one operation's owner-change waits are reported: an
/// [`super::OwnerChangeChurn`] carrying one reports each wait it makes as a
/// [`OperationRefusalDisposition::WaitedForOwnerChange`] of a
/// `ReprepareRequired` refusal, which makes no claim about tool code.
#[derive(Debug, Clone)]
pub(crate) struct WaitReport {
    binding: RefusalObserverBinding,
    run_id: Option<RunId>,
    surface: OperationRefusalSurface,
}

impl WaitReport {
    pub(crate) fn report(&self, wait: OwnerChangeWait, operation_id: Option<&OperationId>) {
        self.binding.observe(
            self.run_id.clone(),
            operation_id.cloned(),
            self.surface.clone(),
            OperationRefused::new(OperationRefusalKind::ReprepareRequired).into(),
            OperationRefusalDisposition::WaitedForOwnerChange {
                waited_ms: u64::try_from(wait.waited().as_millis()).unwrap_or(u64::MAX),
                settled: wait.settled(),
            },
        );
    }
}
