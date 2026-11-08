//! Approval-carried tool grants: a grant request holds a durable decision of
//! kind `tool_grant.issue`, and the decision's executor issues the grant once
//! a person approved it.
//!
//! - [`hold_tool_grant_request`] holds the decision for one request: its
//!   body `tool_grant.issue.v1` ([`ToolGrantIssueBody`]: subject, target,
//!   duration, run scope, reason and who asked), its subject key
//!   `{subject key}/{target}/{request id}` (an agent's request id is its run
//!   and tool call, so a replayed call answers the same decision; a host's is
//!   fresh for each call), the approver id of the host's
//!   [`ToolGrantRequestPolicy`], the requester the host names, and the
//!   policy's deadline. It issues nothing.
//! - [`ToolGrantIssueExecutor`] executes an approved decision: it issues the
//!   grant under the id derived from the decision
//!   ([`ToolGrantId::approved`]), issuer
//!   [`ToolGrantIssuer::Approval`](meerkat_core::tool_grant::ToolGrantIssuer::Approval)
//!   with the decision's id, for `[now, now + duration)`. A replayed execution
//!   meets the grant it issued and settles executed. It refuses a decision no
//!   person decided (`not_a_human_decision`), a request whose subject ended
//!   (`subject_ended`: a member identity whose mob is gone or was replaced,
//!   or whose member left the roster) or that the host cannot confirm now
//!   (`subject_unknown`, fail closed), a run-scoped request whose run is
//!   over (`run_ended`), and a body this version cannot read
//!   (`unreadable_request`). The host's [`ToolGrantSubjectObserver`] answers
//!   what the subject is now.
//! - A request's end owes its requester a notice, which the executor hands
//!   to the host's [`ToolGrantRequestNotices`]: the subject's session (for a
//!   member identity, the session its member holds when the notice is
//!   delivered) reads that its request was approved or declined.
//! - [`tool_grant_requests_of`] reads a subject's requests, which a list
//!   projects: open and approved ones as `requested`, and the rejected,
//!   expired, withdrawn and refused ones due from a bound on as `declined`.
//! - An agent asks through its session's grant seam, which holds the request
//!   with what its host takes requests with ([`ToolGrantRequestHost`]).
//!
//! The decision's machine owns a request's lifecycle, and the grant's own
//! machine owns the grant from its issuance: no grant machine state stands
//! for a request.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::RunId;
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::decision::{
    ApproverId, ConsentProvenance, DECISION_PAGE_MAX, DecisionContractError, DecisionHoldRequest,
    DecisionId, DecisionKind, DecisionListFilter, DecisionOperationId, DecisionPhase,
    DecisionPrincipalKey, DecisionReference, DecisionRenderer, DecisionRendering,
    DecisionRequestBody, DecisionSubjectKey, DecisionTier, Sha256Digest, StoredDecision,
};
use meerkat_core::tool_grant::{ToolGrantId, ToolGrantSubject, ToolGrantTarget};
use serde::{Deserialize, Serialize};

use super::service::{ToolGrantService, ToolGrantServiceError};
use crate::decision::{
    DecisionError, DecisionExecution, DecisionExecutionPermit, DecisionExecutor,
    DecisionExecutorRecordUnavailable, DurableDecisionService,
};

/// The decision kind of a tool grant request.
pub const TOOL_GRANT_ISSUE_KIND: &str = "tool_grant.issue";

/// The version of a tool grant request's held body.
pub const TOOL_GRANT_ISSUE_BODY: &str = "tool_grant.issue.v1";

/// The renderer of a tool grant request's rendering.
pub const TOOL_GRANT_ISSUE_RENDERER: &str = "tool_grant.render-v1";

/// The decision kind of a tool grant request, [`TOOL_GRANT_ISSUE_KIND`].
///
/// # Errors
/// Never in practice: the kind is a valid token.
pub fn tool_grant_issue_kind() -> Result<DecisionKind, DecisionContractError> {
    DecisionKind::new(TOOL_GRANT_ISSUE_KIND)
}

/// Who asked for a grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolGrantRequestOrigin {
    /// A host asked (`tool_grants/request`); `request_id` names this ask.
    Host { request_id: String },
    /// The subject's agent asked from one tool call of one of its runs.
    Agent { run_id: RunId, call_id: String },
}

impl ToolGrantRequestOrigin {
    /// The request id a decision's subject key ends with.
    fn request_key(&self) -> String {
        match self {
            Self::Host { request_id } => format!("host:{request_id}"),
            Self::Agent { run_id, call_id } => format!("run:{run_id}:call:{call_id}"),
        }
    }
}

/// The held body of a tool grant request, `tool_grant.issue.v1`. A body
/// field this version does not know makes the body unreadable here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolGrantIssueBody {
    /// [`TOOL_GRANT_ISSUE_BODY`].
    pub body: String,
    pub subject: ToolGrantSubject,
    pub target: ToolGrantTarget,
    /// How long the grant lasts from its issuance, in milliseconds.
    pub duration_ms: u64,
    /// The run of the subject session the grant is scoped to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_scope: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub origin: ToolGrantRequestOrigin,
}

impl ToolGrantIssueBody {
    /// The body `decision` holds, when it is a tool grant request this
    /// version reads.
    ///
    /// # Errors
    /// [`ToolGrantRequestError::UnreadableRequest`] for another kind, another
    /// body version, or a body that does not parse.
    pub fn of(decision: &StoredDecision) -> Result<Self, ToolGrantRequestError> {
        if decision.descriptor.kind.as_str() != TOOL_GRANT_ISSUE_KIND {
            return Err(ToolGrantRequestError::UnreadableRequest(format!(
                "decision {} is of kind {}",
                decision.decision_id, decision.descriptor.kind
            )));
        }
        let body: Self =
            serde_json::from_str(decision.descriptor.request.get()).map_err(|error| {
                ToolGrantRequestError::UnreadableRequest(format!(
                    "decision {}: {error}",
                    decision.decision_id
                ))
            })?;
        if body.body != TOOL_GRANT_ISSUE_BODY {
            return Err(ToolGrantRequestError::UnreadableRequest(format!(
                "decision {} holds the body {}",
                decision.decision_id, body.body
            )));
        }
        Ok(body)
    }
}

/// The host's policy for the grant requests it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGrantRequestPolicy {
    /// The approver id every request's decision is held for: the host maps
    /// it to the people who may approve.
    pub approver_id: String,
    /// How long a request waits for its decision.
    pub request_ttl: Duration,
}

impl ToolGrantRequestPolicy {
    /// The default approver id: `operator`.
    pub const DEFAULT_APPROVER_ID: &'static str = "operator";
    /// The default time a request waits for its decision: one hour.
    pub const DEFAULT_REQUEST_TTL: Duration = Duration::from_hours(1);

    /// The approver id a request for `subject` is held for.
    #[must_use]
    pub fn approver_for(&self, subject: &ToolGrantSubject) -> &str {
        let _ = subject;
        &self.approver_id
    }
}

impl Default for ToolGrantRequestPolicy {
    fn default() -> Self {
        Self {
            approver_id: Self::DEFAULT_APPROVER_ID.to_owned(),
            request_ttl: Self::DEFAULT_REQUEST_TTL,
        }
    }
}

/// What a host takes its agents' grant requests with: the decision owner
/// that holds them, its request policy, and the principal an agent's request
/// names as its requester. A host sets it on its runtime
/// ([`crate::MeerkatMachine::set_tool_grant_request_host`]) while it runs the
/// executor of the decisions they hold.
#[derive(Debug, Clone)]
pub struct ToolGrantRequestHost {
    /// The decision owner the requests are held in.
    pub decisions: DurableDecisionService,
    /// Who approves them and how long they wait.
    pub policy: ToolGrantRequestPolicy,
    /// The principal an agent's request names as its requester.
    pub requester: DecisionPrincipalKey,
}

/// One grant request, as a host or an agent asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGrantIssueRequest {
    pub subject: ToolGrantSubject,
    pub target: ToolGrantTarget,
    /// How long the grant lasts from its issuance.
    pub duration: Duration,
    /// The run of the subject session the grant is scoped to, if any.
    pub run_scope: Option<RunId>,
    pub reason: Option<String>,
    pub origin: ToolGrantRequestOrigin,
    /// Who asks: the host's principal for a host's request, the agent's for
    /// an agent's.
    pub requester: DecisionPrincipalKey,
}

/// A grant request was refused, or its decision could not be held.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolGrantRequestError {
    /// The target names no tool.
    #[error("a tool grant request names no tool")]
    EmptyTarget,
    /// The duration is empty or longer than the host's maximum lifetime.
    #[error(
        "a tool grant request lasts {duration_ms} ms, which must be at least 1 and at most {max_lifetime_ms}"
    )]
    InvalidDuration {
        duration_ms: u64,
        max_lifetime_ms: u64,
    },
    /// The request cannot be held: a key, the body, the rendering or the
    /// host's approver id is not one a decision takes.
    #[error("the tool grant request cannot be held: {0}")]
    Invalid(String),
    /// The decision's owner refused or failed the hold.
    #[error(transparent)]
    Decision(#[from] DecisionError),
    /// The held body is not a tool grant request this version reads.
    #[error("unreadable tool grant request: {0}")]
    UnreadableRequest(String),
    /// The host clock could not be read.
    #[error("the host clock could not be read: {0}")]
    Clock(String),
}

impl From<DecisionContractError> for ToolGrantRequestError {
    fn from(error: DecisionContractError) -> Self {
        Self::Invalid(error.to_string())
    }
}

/// The key of a target in a request's subject key.
fn target_key(target: &ToolGrantTarget) -> String {
    match target {
        ToolGrantTarget::Tool { name } => format!("tool:{name}"),
        ToolGrantTarget::Family { family, .. } => format!("family:{family}"),
    }
}

/// The text a person reads for a request.
fn rendering_text(body: &ToolGrantIssueBody) -> String {
    let subject = match &body.subject {
        ToolGrantSubject::Session { session_id } => format!("the session {session_id}"),
        ToolGrantSubject::Identity {
            mob_id, identity, ..
        } => format!("the member {identity} of the mob {mob_id}"),
    };
    let target = match &body.target {
        ToolGrantTarget::Tool { name } => format!("the tool {name}"),
        ToolGrantTarget::Family { family, names } => {
            let names = names
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("the tool family {family} ({names})")
        }
    };
    let window = render_duration(body.duration_ms);
    let scope = match &body.run_scope {
        Some(run_id) => format!(", and only in its run {run_id}"),
        None => String::new(),
    };
    let asked_by = match &body.origin {
        ToolGrantRequestOrigin::Host { .. } => "a host",
        ToolGrantRequestOrigin::Agent { .. } => "the session's agent",
    };
    let mut text = format!(
        "Grant {subject} {target} for {window} from the approval{scope}. Asked by {asked_by}."
    );
    if let Some(reason) = body.reason.as_deref().filter(|reason| !reason.is_empty()) {
        text.push_str("\nReason: ");
        text.push_str(reason);
    }
    text
}

/// A duration in milliseconds as a person reads it.
fn render_duration(duration_ms: u64) -> String {
    const SECOND: u64 = 1_000;
    const MINUTE: u64 = 60 * SECOND;
    const HOUR: u64 = 60 * MINUTE;
    if duration_ms.is_multiple_of(HOUR) {
        format!("{} h", duration_ms / HOUR)
    } else if duration_ms.is_multiple_of(MINUTE) {
        format!("{} min", duration_ms / MINUTE)
    } else if duration_ms.is_multiple_of(SECOND) {
        format!("{} s", duration_ms / SECOND)
    } else {
        format!("{duration_ms} ms")
    }
}

/// Hold the decision of one grant request: see the module documentation.
/// A replay of the same agent request (its run and tool call) answers the
/// decision it held, as it stands.
///
/// # Errors
/// [`ToolGrantRequestError::EmptyTarget`],
/// [`ToolGrantRequestError::InvalidDuration`] (against `max_lifetime_ms`),
/// [`ToolGrantRequestError::Invalid`], and the decision owner's refusals and
/// failures ([`ToolGrantRequestError::Decision`]).
pub fn hold_tool_grant_request(
    decisions: &DurableDecisionService,
    policy: &ToolGrantRequestPolicy,
    max_lifetime_ms: u64,
    now_ms: u64,
    request: ToolGrantIssueRequest,
) -> Result<StoredDecision, ToolGrantRequestError> {
    let (_, hold) = tool_grant_hold(policy, max_lifetime_ms, now_ms, request)?;
    Ok(decisions.hold(hold, true)?)
}

/// What an agent's ask was answered with: the request's decision and held
/// body, and whether the call joined a request already open for its target
/// instead of holding (or replaying) its own.
#[derive(Debug, Clone)]
pub(crate) struct AgentRequestAnswer {
    pub(crate) decision: StoredDecision,
    pub(crate) body: ToolGrantIssueBody,
    pub(crate) joined: bool,
}

/// Hold an agent's grant request as [`hold_tool_grant_request`] holds one,
/// keeping one request per target. A replayed call (its run and tool call)
/// answers the request it held. A call for a target its subject already has
/// an open or approved window request for joins that request: it is answered
/// with it while the request is still open or approved as it reads now (one
/// past its deadline reads expired before the host records the expiry, and is
/// not joined), and holds a new request after it ended. Only otherwise is a
/// decision held. So an agent that asks again and again leaves one request
/// per target with its approver. A joined request keeps its own duration,
/// which can differ from the one the call asked.
///
/// # Errors
/// As [`hold_tool_grant_request`], and the decision owner's read failures.
pub(crate) fn hold_agent_tool_grant_request(
    decisions: &DurableDecisionService,
    policy: &ToolGrantRequestPolicy,
    max_lifetime_ms: u64,
    now_ms: u64,
    request: ToolGrantIssueRequest,
) -> Result<AgentRequestAnswer, ToolGrantRequestError> {
    let subject = request.subject.clone();
    let target = request.target.clone();
    let (decision_id, hold) = tool_grant_hold(policy, max_lifetime_ms, now_ms, request)?;
    let held = |hold| -> Result<AgentRequestAnswer, ToolGrantRequestError> {
        let decision = decisions.hold(hold, true)?;
        let body = ToolGrantIssueBody::of(&decision)?;
        Ok(AgentRequestAnswer {
            decision,
            body,
            joined: false,
        })
    };
    match decisions.get(&decision_id) {
        // A replay holds again, which answers the decision as it stands.
        Ok(_) => return held(hold),
        Err(DecisionError::NotFound(_)) => {}
        Err(error) => return Err(error.into()),
    }
    let open = tool_grant_requests_of(decisions, &subject, None)?
        .into_iter()
        .find(|(decision, body)| {
            matches!(
                decision.state.phase,
                DecisionPhase::Open | DecisionPhase::Approved
            ) && body.target == target
                && body.run_scope.is_none()
        });
    if let Some((decision, body)) = open {
        return Ok(AgentRequestAnswer {
            decision,
            body,
            joined: true,
        });
    }
    held(hold)
}

/// What holding `request` holds, once its target and duration are checked:
/// the decision's id and its hold.
fn tool_grant_hold(
    policy: &ToolGrantRequestPolicy,
    max_lifetime_ms: u64,
    now_ms: u64,
    request: ToolGrantIssueRequest,
) -> Result<(DecisionId, DecisionHoldRequest), ToolGrantRequestError> {
    if request.target.names().next().is_none()
        || request.target.names().any(|name| name.as_str().is_empty())
    {
        return Err(ToolGrantRequestError::EmptyTarget);
    }
    let duration_ms = u64::try_from(request.duration.as_millis()).unwrap_or(u64::MAX);
    if duration_ms == 0 || duration_ms > max_lifetime_ms {
        return Err(ToolGrantRequestError::InvalidDuration {
            duration_ms,
            max_lifetime_ms,
        });
    }
    let kind = tool_grant_issue_kind()?;
    let subject_key = DecisionSubjectKey::new(format!(
        "{}/{}/{}",
        request.subject.key(),
        target_key(&request.target),
        request.origin.request_key()
    ))?;
    let decision_id = DecisionId::derive(&kind, &subject_key);
    let grant_id = ToolGrantId::approved(&decision_id);
    let approver_id = ApproverId::new(policy.approver_for(&request.subject))?;
    let body = ToolGrantIssueBody {
        body: TOOL_GRANT_ISSUE_BODY.to_owned(),
        subject: request.subject,
        target: request.target,
        duration_ms,
        run_scope: request.run_scope,
        reason: request.reason,
        origin: request.origin,
    };
    let text = serde_json::to_string(&body)
        .map_err(|error| ToolGrantRequestError::Invalid(error.to_string()))?;
    let raw = serde_json::value::RawValue::from_string(text.clone())
        .map_err(|error| ToolGrantRequestError::Invalid(error.to_string()))?;
    let ttl_ms = u64::try_from(policy.request_ttl.as_millis()).unwrap_or(u64::MAX);
    let hold = DecisionHoldRequest::new(
        kind,
        subject_key,
        DecisionOperationId::new(format!("tool_grant:{grant_id}"))?,
        Sha256Digest::of(text.as_bytes()),
        DecisionRequestBody::new(raw)?,
        request.requester,
        approver_id,
        now_ms.saturating_add(ttl_ms),
        DecisionRendering::new(rendering_text(&body))?,
        DecisionRenderer::new(TOOL_GRANT_ISSUE_RENDERER)?,
    );
    Ok((decision_id, hold))
}

/// The tool grant requests held for `subject`, in decision id order per
/// phase: the open and approved ones and, with `ended_since_ms`, the
/// rejected, expired, withdrawn and refused ones whose deadline is at or
/// after it. An executed request is its grant, which the grant store lists.
/// The decision store files decisions by kind, phase and deadline, not by
/// subject: this pages the kind's decisions of each phase (an ended phase
/// from `ended_since_ms` on, through the store's phase and deadline index)
/// and keeps the subject's.
///
/// # Errors
/// The decision owner's read failures.
pub fn tool_grant_requests_of(
    decisions: &DurableDecisionService,
    subject: &ToolGrantSubject,
    ended_since_ms: Option<u64>,
) -> Result<Vec<(StoredDecision, ToolGrantIssueBody)>, DecisionError> {
    let kind = tool_grant_issue_kind()?;
    let of_phase = |phase| {
        DecisionListFilter::default()
            .with_kind(kind.clone())
            .with_phase(phase)
    };
    let mut filters = vec![
        of_phase(DecisionPhase::Open),
        of_phase(DecisionPhase::Approved),
    ];
    if let Some(since) = ended_since_ms {
        filters.extend(
            [
                DecisionPhase::Rejected,
                DecisionPhase::Expired,
                DecisionPhase::Withdrawn,
                DecisionPhase::ExecutionRefused,
            ]
            .into_iter()
            .map(|phase| of_phase(phase).with_deadline_from(since)),
        );
    }
    let mut requests = Vec::new();
    for filter in filters {
        let mut after: Option<String> = None;
        loop {
            let listing = decisions.list(&filter, after.as_deref(), DECISION_PAGE_MAX)?;
            for decision in listing.decisions {
                if let Ok(body) = ToolGrantIssueBody::of(&decision)
                    && body.subject == *subject
                {
                    requests.push((decision, body));
                }
            }
            match listing.next_after {
                Some(next) if after.as_deref() != Some(next.as_str()) => after = Some(next),
                _ => break,
            }
        }
    }
    Ok(requests)
}

/// How a request ended, as its notice says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolGrantRequestOutcome {
    /// Approved, and the grant was issued.
    Granted { grant_id: ToolGrantId },
    /// A person rejected it.
    Rejected,
    /// Nobody decided it before its deadline, or it was approved too late.
    Expired,
    /// Its requester or the host withdrew it.
    Withdrawn,
    /// Approved, and the grant service refused to issue it: the reference
    /// names why (`run_ended`, `not_a_human_decision`, ...).
    Refused { reason: String },
}

impl ToolGrantRequestOutcome {
    /// The outcome's token in a notice: `granted`, `rejected`, `expired`,
    /// `withdrawn` or `refused`.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Granted { .. } => "granted",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
            Self::Withdrawn => "withdrawn",
            Self::Refused { .. } => "refused",
        }
    }
}

/// The notice an ended request owes its subject's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGrantRequestNotice {
    pub decision_id: DecisionId,
    pub subject: ToolGrantSubject,
    pub target: ToolGrantTarget,
    pub outcome: ToolGrantRequestOutcome,
}

impl ToolGrantRequestNotice {
    /// The notice `decision` owes, when it ended.
    #[must_use]
    pub fn of(decision: &StoredDecision, body: &ToolGrantIssueBody) -> Option<Self> {
        let outcome = match decision.state.phase {
            DecisionPhase::Executed => ToolGrantRequestOutcome::Granted {
                grant_id: ToolGrantId::approved(&decision.decision_id),
            },
            DecisionPhase::Rejected => ToolGrantRequestOutcome::Rejected,
            DecisionPhase::Expired => ToolGrantRequestOutcome::Expired,
            DecisionPhase::Withdrawn => ToolGrantRequestOutcome::Withdrawn,
            DecisionPhase::ExecutionRefused => ToolGrantRequestOutcome::Refused {
                reason: decision
                    .execution_refusal
                    .as_ref()
                    .map_or_else(|| "refused".to_owned(), ToString::to_string),
            },
            DecisionPhase::Open | DecisionPhase::Approved => return None,
        };
        Some(Self {
            decision_id: decision.decision_id.clone(),
            subject: body.subject.clone(),
            target: body.target.clone(),
            outcome,
        })
    }
}

/// Where a host delivers the notices of ended grant requests.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ToolGrantRequestNotices: Send + Sync {
    /// Deliver `notice` to its subject's session: the receipt (admitted, or
    /// failed terminally), or `None` when it cannot be delivered now (the
    /// session cannot take the input now), so a later pass asks again. A
    /// delivery whose receipt was not recorded is asked again too: deliver
    /// idempotently per decision.
    async fn deliver(
        &self,
        notice: &ToolGrantRequestNotice,
    ) -> Option<ApprovalConsentDeliveryState>;
}

/// What the host that serves a grant subject sees of it now.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolGrantSubjectPresence {
    /// The subject can hold a grant, and runs `current_run` here now (`None`
    /// when it runs none here).
    Held { current_run: Option<RunId> },
    /// The subject ended: a member identity whose mob is gone or was
    /// replaced, or whose member left the roster. Its grants ended with it,
    /// and none is issued to it after.
    Ended,
    /// The host cannot tell now (it cannot read its record of the subject):
    /// fail closed, an approval of a request to it is refused and never
    /// issues a grant.
    Unknown,
}

/// What a host sees of the grant subjects it serves.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ToolGrantSubjectObserver: Send + Sync {
    /// What `subject` is now. A session is held, running its current run
    /// on this host. A member identity is held while its mob runs the
    /// incarnation the subject names and its roster holds the identity,
    /// running the current run of the session its member holds on this
    /// host; otherwise it ended.
    async fn presence(&self, subject: &ToolGrantSubject) -> ToolGrantSubjectPresence;
}

/// The executor of `tool_grant.issue` decisions: see the module
/// documentation.
pub struct ToolGrantIssueExecutor {
    kind: DecisionKind,
    grants: Arc<ToolGrantService>,
    subjects: Arc<dyn ToolGrantSubjectObserver>,
    notices: Arc<dyn ToolGrantRequestNotices>,
}

impl std::fmt::Debug for ToolGrantIssueExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolGrantIssueExecutor")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl ToolGrantIssueExecutor {
    /// The executor that issues approved requests through `grants`, asks
    /// `subjects` whether a request's subject still holds grants and a
    /// run-scoped request's run still runs, and hands notices to `notices`.
    ///
    /// # Errors
    /// Never in practice: the decision kind is a valid token.
    pub fn new(
        grants: Arc<ToolGrantService>,
        subjects: Arc<dyn ToolGrantSubjectObserver>,
        notices: Arc<dyn ToolGrantRequestNotices>,
    ) -> Result<Self, DecisionContractError> {
        Ok(Self {
            kind: tool_grant_issue_kind()?,
            grants,
            subjects,
            notices,
        })
    }

    fn refused(reason: &str) -> DecisionExecution {
        match DecisionReference::new(reason) {
            Ok(reference) => DecisionExecution::Refused(reference),
            // Every reason this executor names is a valid reference.
            Err(_) => DecisionExecution::Released,
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl DecisionExecutor for ToolGrantIssueExecutor {
    fn kind(&self) -> &DecisionKind {
        &self.kind
    }

    async fn execute(
        &self,
        decision: &StoredDecision,
        permit: &DecisionExecutionPermit,
    ) -> DecisionExecution {
        let descriptor = &decision.descriptor;
        if !permit.binds(&descriptor.operation_id, &descriptor.request_digest) {
            return DecisionExecution::Released;
        }
        // Decided by people only: a reviewer's allow never issues a grant.
        if !matches!(
            permit.consent(),
            ConsentProvenance::Decided {
                tier: DecisionTier::Human,
                ..
            }
        ) {
            return Self::refused("not_a_human_decision");
        }
        let body = match ToolGrantIssueBody::of(decision) {
            Ok(body) => body,
            Err(_) => return Self::refused("unreadable_request"),
        };
        let current_run = match self.subjects.presence(&body.subject).await {
            ToolGrantSubjectPresence::Held { current_run } => current_run,
            // An ended identity's grants ended with it, and none is issued
            // after: a later member of its name never holds one asked for
            // this one.
            ToolGrantSubjectPresence::Ended => return Self::refused("subject_ended"),
            // Fail closed: an approval never issues a grant to a subject the
            // host cannot confirm.
            ToolGrantSubjectPresence::Unknown => return Self::refused("subject_unknown"),
        };
        if let Some(run_id) = &body.run_scope
            && current_run.as_ref() != Some(run_id)
        {
            return Self::refused("run_ended");
        }
        // The window starts at the clock reading the commit is admitted at.
        let Ok(now_ms) = self.grants.now_ms() else {
            return DecisionExecution::Released;
        };
        if !permit.admits_commit_at(now_ms) {
            return DecisionExecution::Released;
        }
        match self
            .grants
            .issue_approved_at(&decision.decision_id, &body, now_ms)
            .await
        {
            Ok(_) => DecisionExecution::Committed,
            Err(ToolGrantServiceError::InvalidWindow { .. }) => Self::refused("invalid_window"),
            Err(ToolGrantServiceError::EmptyTarget) => Self::refused("empty_target"),
            Err(ToolGrantServiceError::Conflict { .. }) => Self::refused("grant_id_taken"),
            // Past the permit's deadline, or a store or clock failure:
            // nothing was committed.
            Err(_) => DecisionExecution::Released,
        }
    }

    async fn committed(
        &self,
        decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable> {
        self.grants
            .issued_for(&decision.decision_id)
            .await
            .map(|row| row.is_some())
            .map_err(|error| DecisionExecutorRecordUnavailable(error.to_string()))
    }

    async fn deliver_notice(
        &self,
        decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState> {
        let body = match ToolGrantIssueBody::of(decision) {
            Ok(body) => body,
            Err(error) => {
                return Some(ApprovalConsentDeliveryState::Failed {
                    reason: error.to_string(),
                });
            }
        };
        let notice = ToolGrantRequestNotice::of(decision, &body)?;
        self.notices.deliver(&notice).await
    }
}
