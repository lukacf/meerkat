//! Questions an agent asks a person: a pending question set is a durable
//! decision of kind `ask_user.questions`, and its answers are the evidence
//! of that decision.
//!
//! - [`hold_ask_user`] holds the decision of one `ask_user` call: its body
//!   `ask_user.questions.v1` ([`AskUserBody`]: the asking agent and the
//!   typed question set), its subject key `{asker}/{run id}/{call id}` (so
//!   a replayed call answers the decision it held), the fixed approver id
//!   [`ASK_USER_APPROVER_ID`], the requester the host names, a plain-text
//!   rendering of the questions, and the deadline the call's timeout gives.
//! - A person answers by approving the decision with an answer document
//!   ([`attest_ask_user_answers`]), or dismisses the questions by rejecting
//!   it. The decision's machine binds the answers through the digest of the
//!   human evidence that carries them. [`ask_user_answers_of`] reads them
//!   back and validates them against the held set again: an answer that is
//!   not valid for the set is an error, never an answer.
//! - [`AskUserExecutor`] executes an answered decision: it commits nothing
//!   beyond the decision itself. It refuses a decision no person decided
//!   (`not_a_human_decision`), one without a valid answer document
//!   (`invalid_answer`) and a body this version cannot read
//!   (`unreadable_request`).
//! - Every end of a decision owes its asker one notice, which the executor
//!   hands to the host's [`AskUserNotices`] with the typed
//!   [`AskUserResult`] ([`ask_user_result_of`]): `answered`, `dismissed`
//!   (rejected), `timed_out` (expired), or `withdrawn` (withdrawn, or an
//!   execution refused).
//!
//! The decision's machine owns the whole lifecycle of a question set: this
//! module keeps no state of its own, and a restart finds every pending set
//! and every owed notice in the decision store.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::RunId;
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::consent::{
    ConsentEligibility, ConsentEligibilityAnswer, ConsentIneligible,
};
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, ConsentProvenance, DecisionContractError, DecisionEvidence,
    DecisionHoldRequest, DecisionId, DecisionKind, DecisionOperationId, DecisionPhase,
    DecisionPrincipalKey, DecisionReference, DecisionRenderer, DecisionRendering,
    DecisionRequestBody, DecisionSubjectKey, DecisionTier, Sha256Digest, StoredDecision,
};
use meerkat_core::ask_user::{
    ASK_USER_TIMEOUT_MAX_SECONDS, ASK_USER_TIMEOUT_MIN_SECONDS, AskUserAnswers, AskUserInvalid,
    AskUserOutcome, AskUserQuestionAnswer, AskUserQuestionSet, AskUserResult, ask_user_unanswered,
};
use serde::{Deserialize, Serialize};

use crate::decision::{
    DecisionApproverCurrentness, DecisionError, DecisionExecution, DecisionExecutionPermit,
    DecisionExecutor, DecisionExecutorRecordUnavailable, DecisionPassOutcome,
    DurableDecisionService,
};

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests;

/// The decision kind of a pending question set.
pub const ASK_USER_QUESTIONS_KIND: &str = "ask_user.questions";

/// The version of a pending question set's held body.
pub const ASK_USER_QUESTIONS_BODY: &str = "ask_user.questions.v1";

/// The renderer of a pending question set's rendering.
pub const ASK_USER_RENDERER: &str = "ask_user.render-v1";

/// The approver id every pending question set is held for. Who may answer
/// is the host's rule for its human surface; the id names no person.
pub const ASK_USER_APPROVER_ID: &str = "ask_user";

/// The decision kind of a pending question set, [`ASK_USER_QUESTIONS_KIND`].
///
/// # Errors
/// Never in practice: the kind is a valid token.
pub fn ask_user_questions_kind() -> Result<DecisionKind, DecisionContractError> {
    DecisionKind::new(ASK_USER_QUESTIONS_KIND)
}

/// A question set could not be held, or a held one could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AskUserError {
    /// The request cannot be held: its asker, a key, its timeout, the body
    /// or the rendering is not one a decision takes.
    #[error("the question set cannot be held: {0}")]
    Invalid(String),
    /// The decision's owner refused or failed the call.
    #[error(transparent)]
    Decision(#[from] DecisionError),
    /// The held body is not a question set this version reads.
    #[error("unreadable question set: {0}")]
    UnreadableRequest(String),
    /// The decision carries no answer document that is valid for its
    /// question set.
    #[error("invalid answer: {0}")]
    InvalidAnswer(String),
}

impl From<DecisionContractError> for AskUserError {
    fn from(error: DecisionContractError) -> Self {
        Self::Invalid(error.to_string())
    }
}

/// The held body of a pending question set, `ask_user.questions.v1`. A
/// body field this version does not know makes the body unreadable here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserBody {
    /// [`ASK_USER_QUESTIONS_BODY`].
    pub body: String,
    /// The host's opaque key of the asking agent: the host routes the
    /// request's notice by it, and decides who may see the request by it.
    pub asker: String,
    /// The questions.
    pub questions: AskUserQuestionSet,
}

impl AskUserBody {
    /// The body `decision` holds, when it is a question set this version
    /// reads.
    ///
    /// # Errors
    /// [`AskUserError::UnreadableRequest`] for another kind, another body
    /// version, or a body that does not parse (a set that breaks a bound of
    /// the tool does not).
    pub fn of(decision: &StoredDecision) -> Result<Self, AskUserError> {
        if decision.descriptor.kind.as_str() != ASK_USER_QUESTIONS_KIND {
            return Err(AskUserError::UnreadableRequest(format!(
                "decision {} is of kind {}",
                decision.decision_id, decision.descriptor.kind
            )));
        }
        let body: Self =
            serde_json::from_str(decision.descriptor.request.get()).map_err(|error| {
                AskUserError::UnreadableRequest(format!(
                    "decision {}: {error}",
                    decision.decision_id
                ))
            })?;
        if body.body != ASK_USER_QUESTIONS_BODY {
            return Err(AskUserError::UnreadableRequest(format!(
                "decision {} holds the body {}",
                decision.decision_id, body.body
            )));
        }
        Ok(body)
    }
}

/// One `ask_user` call a host holds for a person.
#[derive(Debug, Clone)]
pub struct AskUserHold {
    /// The host's opaque key of the asking agent.
    pub asker: String,
    /// The principal the decision names as its requester.
    pub requester: DecisionPrincipalKey,
    /// The questions.
    pub questions: AskUserQuestionSet,
    /// How long the request waits for its answers.
    pub timeout: Duration,
    /// The run the call belongs to.
    pub run_id: RunId,
    /// The provider's call id.
    pub call_id: String,
}

/// The text a person reads for a question set: its title, each question
/// numbered with its choices, the asker's recommendation marked, and a line
/// where a written answer is accepted.
fn rendering_text(questions: &AskUserQuestionSet) -> String {
    let mut text = String::new();
    if let Some(title) = questions.title().filter(|title| !title.trim().is_empty()) {
        text.push_str(title);
        text.push_str("\n\n");
    }
    for (index, question) in questions.questions().iter().enumerate() {
        // Writing to a `String` cannot fail.
        let _ = writeln!(text, "{}. {}", index + 1, question.prompt);
        for choice in &question.choices {
            let recommended = if question.recommended.as_deref() == Some(choice.id.as_str()) {
                " (recommended)"
            } else {
                ""
            };
            let _ = writeln!(text, "   - {}{recommended}", choice.label);
        }
        if question.allow_free_text {
            text.push_str("   A written answer is accepted.\n");
        }
    }
    text.truncate(text.trim_end().len());
    text
}

/// Hold the decision of one `ask_user` call: see the module documentation.
/// `now_ms` is the host clock's reading of the call; the deadline is that
/// reading plus the call's timeout. A replay of the call (its asker, run
/// and call id, with the same questions) answers the decision it held, as
/// it stands, whatever the clock reads now: the deadline sealed at the
/// first hold stays. The same call with other questions is refused as a
/// hold conflict by the decision's machine.
///
/// # Errors
/// [`AskUserError::Invalid`] for an empty asker, a timeout outside 60 to
/// 604800 seconds, or a key, body or rendering a decision does not take;
/// the decision owner's refusals and failures ([`AskUserError::Decision`]).
pub fn hold_ask_user(
    decisions: &DurableDecisionService,
    now_ms: u64,
    hold: AskUserHold,
) -> Result<StoredDecision, AskUserError> {
    if hold.asker.is_empty() {
        return Err(AskUserError::Invalid(
            "the asking agent has no key".to_owned(),
        ));
    }
    let timeout_ms = u64::try_from(hold.timeout.as_millis()).unwrap_or(u64::MAX);
    if timeout_ms < ASK_USER_TIMEOUT_MIN_SECONDS.saturating_mul(1_000)
        || timeout_ms > ASK_USER_TIMEOUT_MAX_SECONDS.saturating_mul(1_000)
    {
        return Err(AskUserError::Invalid(
            AskUserInvalid::TimeoutOutOfRange {
                seconds: hold.timeout.as_secs(),
            }
            .to_string(),
        ));
    }
    let kind = ask_user_questions_kind()?;
    let subject_key =
        DecisionSubjectKey::new(format!("{}/{}/{}", hold.asker, hold.run_id, hold.call_id))?;
    let decision_id = DecisionId::derive(&kind, &subject_key);
    let body = AskUserBody {
        body: ASK_USER_QUESTIONS_BODY.to_owned(),
        asker: hold.asker,
        questions: hold.questions,
    };
    let text =
        serde_json::to_string(&body).map_err(|error| AskUserError::Invalid(error.to_string()))?;
    let raw = serde_json::value::RawValue::from_string(text.clone())
        .map_err(|error| AskUserError::Invalid(error.to_string()))?;
    // A replay names the deadline its first hold sealed, so the machine
    // compares what the call asks, not when it was replayed.
    let deadline_at_ms = match decisions.get(&decision_id) {
        Ok(held) => held.descriptor.deadline_at_ms,
        Err(DecisionError::NotFound(_)) => now_ms.saturating_add(timeout_ms),
        Err(error) => return Err(error.into()),
    };
    let request = DecisionHoldRequest::new(
        kind,
        subject_key,
        DecisionOperationId::new(format!("ask_user:{decision_id}"))?,
        Sha256Digest::of(text.as_bytes()),
        DecisionRequestBody::new(raw)?,
        hold.requester,
        ApproverId::new(ASK_USER_APPROVER_ID)?,
        deadline_at_ms,
        DecisionRendering::new(rendering_text(&body.questions))?,
        DecisionRenderer::new(ASK_USER_RENDERER)?,
    );
    Ok(decisions.hold(request, true)?)
}

/// The answer document an approving decision carries.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerDocument {
    answers: Vec<AskUserQuestionAnswer>,
}

/// `decision`, an approving human decision, carrying `answers` as its
/// answer document. With [`ask_user_answers_of`] this is the only code that
/// writes or reads where a decision carries its answers.
///
/// # Errors
/// [`AskUserError::Invalid`] for a rejecting decision (a dismissal carries
/// no answers) and for a document larger than a decision's evidence takes.
pub fn attest_ask_user_answers(
    decision: AttestedDecision,
    answers: &AskUserAnswers,
) -> Result<AttestedDecision, AskUserError> {
    if !decision.approves() {
        return Err(AskUserError::Invalid(
            "a rejecting decision carries no answers".to_owned(),
        ));
    }
    let document = serde_json::to_string(&AnswerDocument {
        answers: answers.answers().to_vec(),
    })
    .map_err(|error| AskUserError::Invalid(error.to_string()))?;
    Ok(decision.with_reason(document)?)
}

/// The answers the person who approved `decision` gave to the question set
/// `body` holds: `None` when the decision is not approved by a person or
/// carries no answer document. The document is validated against the held
/// set again.
///
/// # Errors
/// [`AskUserError::InvalidAnswer`] when the decision carries something that
/// is not an answer document valid for the set: it never reads as an
/// answer.
pub fn ask_user_answers_of(
    decision: &StoredDecision,
    body: &AskUserBody,
) -> Result<Option<AskUserAnswers>, AskUserError> {
    let Some(outcome) = &decision.outcome else {
        return Ok(None);
    };
    let DecisionEvidence::Human {
        approve: true,
        reason,
        ..
    } = &outcome.evidence
    else {
        return Ok(None);
    };
    let Some(reason) = reason else {
        return Ok(None);
    };
    let invalid = |why: String| {
        AskUserError::InvalidAnswer(format!("decision {}: {why}", decision.decision_id))
    };
    let document: AnswerDocument =
        serde_json::from_str(reason).map_err(|error| invalid(error.to_string()))?;
    AskUserAnswers::for_set(&body.questions, document.answers)
        .map(Some)
        .map_err(|error| invalid(error.to_string()))
}

/// What the asking agent reads of `decision` now: `asked` while it is open
/// or approved and not yet executed, `answered` with the answer to every
/// question once it executed, `dismissed` when a person rejected it,
/// `timed_out` when its deadline passed, and `withdrawn` when it was
/// withdrawn or its execution was refused. An ended decision nobody
/// answered lists every question `unanswered`.
///
/// # Errors
/// [`AskUserError::UnreadableRequest`] for a decision that holds no
/// question set this version reads, and [`AskUserError::InvalidAnswer`] for
/// an executed decision without a valid answer document.
pub fn ask_user_result_of(decision: &StoredDecision) -> Result<AskUserResult, AskUserError> {
    let body = AskUserBody::of(decision)?;
    let request_id = decision.decision_id.as_str();
    let unanswered =
        |outcome| AskUserResult::ended(outcome, request_id, ask_user_unanswered(&body.questions));
    Ok(match decision.state.phase {
        DecisionPhase::Open | DecisionPhase::Approved => {
            AskUserResult::asked(request_id, decision.descriptor.deadline_at_ms)
        }
        DecisionPhase::Executed => {
            let Some(answers) = ask_user_answers_of(decision, &body)? else {
                return Err(AskUserError::InvalidAnswer(format!(
                    "decision {} executed without an answer document",
                    decision.decision_id
                )));
            };
            AskUserResult::ended(
                AskUserOutcome::Answered,
                request_id,
                answers.complete(&body.questions),
            )
        }
        DecisionPhase::Rejected => unanswered(AskUserOutcome::Dismissed),
        DecisionPhase::Expired => unanswered(AskUserOutcome::TimedOut),
        DecisionPhase::Withdrawn | DecisionPhase::ExecutionRefused => {
            unanswered(AskUserOutcome::Withdrawn)
        }
    })
}

/// The notice an ended question set owes the agent that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskUserNotice {
    /// The decision that held the set: the `request_id` the call answered.
    pub decision_id: DecisionId,
    /// The host's opaque key of the asking agent.
    pub asker: String,
    /// What the agent reads: an ended [`AskUserResult`].
    pub result: AskUserResult,
}

/// Where a host delivers the notices of ended question sets.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AskUserNotices: Send + Sync {
    /// Deliver `notice` to the session of its asker: the receipt (admitted,
    /// or failed terminally), or `None` when it cannot be delivered now, so
    /// a later pass asks again. A delivery whose receipt was not recorded
    /// is asked again too: deliver idempotently per decision.
    async fn deliver(&self, notice: &AskUserNotice) -> Option<ApprovalConsentDeliveryState>;
}

/// The executor of `ask_user.questions` decisions: see the module
/// documentation.
pub struct AskUserExecutor {
    kind: DecisionKind,
    notices: Arc<dyn AskUserNotices>,
}

impl std::fmt::Debug for AskUserExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskUserExecutor")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl AskUserExecutor {
    /// The executor that hands the notices of ended question sets to
    /// `notices`.
    ///
    /// # Errors
    /// Never in practice: the decision kind is a valid token.
    pub fn new(notices: Arc<dyn AskUserNotices>) -> Result<Self, DecisionContractError> {
        Ok(Self {
            kind: ask_user_questions_kind()?,
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
impl DecisionExecutor for AskUserExecutor {
    fn kind(&self) -> &DecisionKind {
        &self.kind
    }

    /// The answers are the decision's own evidence, which its machine
    /// already bound: an execution checks them and commits nothing else.
    async fn execute(
        &self,
        decision: &StoredDecision,
        permit: &DecisionExecutionPermit,
    ) -> DecisionExecution {
        let descriptor = &decision.descriptor;
        if !permit.binds(&descriptor.operation_id, &descriptor.request_digest) {
            return DecisionExecution::Released;
        }
        // Answered by people only: a model's allow is never an answer.
        if !matches!(
            permit.consent(),
            ConsentProvenance::Decided {
                tier: DecisionTier::Human,
                ..
            }
        ) {
            return Self::refused("not_a_human_decision");
        }
        let Ok(body) = AskUserBody::of(decision) else {
            return Self::refused("unreadable_request");
        };
        match ask_user_answers_of(decision, &body) {
            Ok(Some(_)) => DecisionExecution::Committed,
            Ok(None) | Err(_) => Self::refused("invalid_answer"),
        }
    }

    /// The commit of an answered decision is its valid answer document,
    /// which the decision row itself records: a permit left outstanding
    /// settles executed exactly when the row carries one, and is released
    /// otherwise, so the next execution refuses it.
    async fn committed(
        &self,
        decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable> {
        let Ok(body) = AskUserBody::of(decision) else {
            return Ok(false);
        };
        Ok(matches!(ask_user_answers_of(decision, &body), Ok(Some(_))))
    }

    async fn deliver_notice(
        &self,
        decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState> {
        let failed = |error: AskUserError| {
            Some(ApprovalConsentDeliveryState::Failed {
                reason: error.to_string(),
            })
        };
        let body = match AskUserBody::of(decision) {
            Ok(body) => body,
            Err(error) => return failed(error),
        };
        let result = match ask_user_result_of(decision) {
            Ok(result) => result,
            Err(error) => return failed(error),
        };
        // A decision that has not ended owes no notice.
        if result.outcome == AskUserOutcome::Asked {
            return None;
        }
        self.notices
            .deliver(&AskUserNotice {
                decision_id: decision.decision_id.clone(),
                asker: body.asker,
                result,
            })
            .await
    }

    /// An answered question set leaves nothing outside the decision store
    /// to undo. No outcome.
    async fn sweep(&self) -> Vec<(DecisionId, DecisionPassOutcome)> {
        Vec::new()
    }
}

/// The host's currentness answer for the person who answered a question
/// set: always current. An answer is data and carries no authority, so
/// there is nothing a later revocation of the person could take back; who
/// may answer is checked by the host when the answer is given.
#[derive(Debug, Clone, Copy, Default)]
pub struct AskUserApproverCurrent;

/// Evidence that never goes stale: see [`AskUserApproverCurrent`].
struct AlwaysCurrent;

impl ConsentEligibility for AlwaysCurrent {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

impl DecisionApproverCurrentness for AskUserApproverCurrent {
    fn approver_answer(&self, _decision: &StoredDecision) -> ConsentEligibilityAnswer {
        ConsentEligibilityAnswer::Eligible(Arc::new(AlwaysCurrent))
    }
}
