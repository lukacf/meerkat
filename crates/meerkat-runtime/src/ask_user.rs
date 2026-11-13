//! Questions an agent asks a person: a pending question set is a durable
//! decision of kind `ask_user.questions`.
//!
//! This is the shape of the kind only: its tokens, the held body, the hold,
//! the answer carrier, the result, the notice seam and the executor.
//! Nothing is held or read yet, and the executor refuses every execution.
//! The rows in `tests.rs` state what the kind does once it is built.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::RunId;
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::consent::ConsentEligibilityAnswer;
use meerkat_core::approval::decision::{
    AttestedDecision, DecisionContractError, DecisionId, DecisionKind, DecisionPrincipalKey,
    DecisionReference, StoredDecision,
};
use meerkat_core::ask_user::{AskUserAnswers, AskUserQuestionSet, AskUserResult};
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

/// The approver id every pending question set is held for.
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
    /// The request cannot be held.
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

/// The held body of a pending question set, `ask_user.questions.v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserBody {
    /// [`ASK_USER_QUESTIONS_BODY`].
    pub body: String,
    /// The host's opaque key of the asking agent.
    pub asker: String,
    /// The questions.
    pub questions: AskUserQuestionSet,
}

impl AskUserBody {
    /// The body `decision` holds. Not built: no decision is read.
    ///
    /// # Errors
    /// Always, for now.
    pub fn of(decision: &StoredDecision) -> Result<Self, AskUserError> {
        Err(AskUserError::UnreadableRequest(format!(
            "decision {}: question sets are not read yet",
            decision.decision_id
        )))
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

/// Hold the decision of one `ask_user` call. Not built: nothing is held.
///
/// # Errors
/// Always, for now.
pub fn hold_ask_user(
    decisions: &DurableDecisionService,
    now_ms: u64,
    hold: AskUserHold,
) -> Result<StoredDecision, AskUserError> {
    let _ = (decisions, now_ms, hold);
    Err(AskUserError::Invalid(
        "question sets are not held yet".to_owned(),
    ))
}

/// `decision` carrying `answers`. Not built: the decision as given.
///
/// # Errors
/// None yet.
pub fn attest_ask_user_answers(
    decision: AttestedDecision,
    answers: &AskUserAnswers,
) -> Result<AttestedDecision, AskUserError> {
    let _ = answers;
    Ok(decision)
}

/// The answers the person who approved `decision` gave. Not built: none.
///
/// # Errors
/// None yet.
pub fn ask_user_answers_of(
    decision: &StoredDecision,
    body: &AskUserBody,
) -> Result<Option<AskUserAnswers>, AskUserError> {
    let _ = (decision, body);
    Ok(None)
}

/// What the asking agent reads of `decision` now. Not built: `asked`.
///
/// # Errors
/// None yet.
pub fn ask_user_result_of(decision: &StoredDecision) -> Result<AskUserResult, AskUserError> {
    Ok(AskUserResult::asked(decision.decision_id.as_str(), 0))
}

/// The notice an ended question set owes the agent that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskUserNotice {
    /// The decision that held the set.
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
    /// Deliver `notice` to the session of its asker.
    async fn deliver(&self, notice: &AskUserNotice) -> Option<ApprovalConsentDeliveryState>;
}

/// The executor of `ask_user.questions` decisions. Not built: it refuses
/// every execution and delivers no notice.
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
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl DecisionExecutor for AskUserExecutor {
    fn kind(&self) -> &DecisionKind {
        &self.kind
    }

    async fn execute(
        &self,
        decision: &StoredDecision,
        permit: &DecisionExecutionPermit,
    ) -> DecisionExecution {
        let _ = (decision, permit, &self.notices);
        match DecisionReference::new("not_built") {
            Ok(reference) => DecisionExecution::Refused(reference),
            Err(_) => DecisionExecution::Released,
        }
    }

    async fn committed(
        &self,
        decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable> {
        let _ = decision;
        Ok(false)
    }

    async fn deliver_notice(
        &self,
        decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState> {
        let _ = decision;
        None
    }

    async fn sweep(&self) -> Vec<(DecisionId, DecisionPassOutcome)> {
        Vec::new()
    }
}

/// The host's currentness answer for the person who answered a question
/// set. Not built: unsettled.
#[derive(Debug, Clone, Copy, Default)]
pub struct AskUserApproverCurrent;

impl DecisionApproverCurrentness for AskUserApproverCurrent {
    fn approver_answer(&self, _decision: &StoredDecision) -> ConsentEligibilityAnswer {
        ConsentEligibilityAnswer::Unsettled
    }
}
