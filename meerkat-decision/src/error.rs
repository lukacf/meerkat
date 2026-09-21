//! Typed decision failures.
//!
//! Three classes stay distinct and are never collapsed into one another:
//! a request the service refuses to send, an operational backend failure,
//! and a backend answer that fails the fixed interpretation contract. A valid
//! judgment the caller dislikes is none of these.

use serde::{Deserialize, Serialize};

use crate::contracts::{
    BackendKind, InvalidIdentifier, InvalidStateShape, QuestionId, QuestionKind,
};

/// The request was refused before any backend work.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RequestValidationError {
    #[error("request carries no questions")]
    NoQuestions,
    #[error("request carries {count} questions; the limit is {max}")]
    TooManyQuestions { count: usize, max: usize },
    #[error("question id `{id}` appears more than once")]
    DuplicateQuestionId { id: QuestionId },
    #[error("question `{question}` has empty instructions")]
    EmptyInstructions { question: QuestionId },
    #[error("question `{question}` carries structured text that is not an object or array")]
    InstructionShape { question: QuestionId },
    #[error("question `{question}` text is {bytes} bytes; the limit is {max}")]
    InstructionTooLarge {
        question: QuestionId,
        bytes: usize,
        max: usize,
    },
    #[error("choose-one question `{question}` supplies {count} options; at least 2 are required")]
    TooFewOptions { question: QuestionId, count: usize },
    #[error("choose-one question `{question}` supplies {count} options; the limit is {max}")]
    TooManyOptions {
        question: QuestionId,
        count: usize,
        max: usize,
    },
    #[error("choose-one question `{question}` repeats option `{option}`")]
    DuplicateOption {
        question: QuestionId,
        option: String,
    },
    #[error("choose-one question `{question}` uses reserved option id `{option}`")]
    ReservedOption {
        question: QuestionId,
        option: String,
    },
    #[error("grade question `{question}` supplies {count} levels; at least 2 are required")]
    TooFewLevels { question: QuestionId, count: usize },
    #[error("grade question `{question}` supplies {count} levels; the limit is {max}")]
    TooManyLevels {
        question: QuestionId,
        count: usize,
        max: usize,
    },
    #[error("state is {bytes} bytes; the limit is {max}")]
    StateTooLarge { bytes: usize, max: usize },
    #[error("task is {bytes} bytes; the limit is {max}")]
    TaskTooLarge { bytes: usize, max: usize },
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(InvalidIdentifier),
    #[error("invalid state: {0}")]
    InvalidState(InvalidStateShape),
}

impl From<InvalidIdentifier> for RequestValidationError {
    fn from(error: InvalidIdentifier) -> Self {
        Self::InvalidIdentifier(error)
    }
}

impl From<InvalidStateShape> for RequestValidationError {
    fn from(error: InvalidStateShape) -> Self {
        Self::InvalidState(error)
    }
}

/// The backend answered, but the answer fails the fixed interpretation
/// contract. Nothing is guessed or defaulted in its place.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum AnswerValidationError {
    #[error("no answer was returned for question `{question}`")]
    MissingAnswer { question: QuestionId },
    #[error("an answer was returned for unknown question `{id}`")]
    UnknownQuestion { id: String },
    #[error("question `{question}` expected a {expected:?} answer but received {actual:?}")]
    KindMismatch {
        question: QuestionId,
        expected: QuestionKind,
        actual: QuestionKind,
    },
    #[error("question `{question}` selected option `{option}`, which was not supplied")]
    OptionNotSupplied {
        question: QuestionId,
        option: String,
    },
    #[error("question `{question}` elected level {level} but only {levels} levels were supplied")]
    LevelOutOfRange {
        question: QuestionId,
        level: u32,
        levels: u32,
    },
    #[error("question `{question}` carries a non-finite or out-of-range `{field}` value")]
    InvalidNumeric { question: QuestionId, field: String },
    #[error("question `{question}` distribution names option `{option}`, which was not supplied")]
    DistributionOptionNotSupplied {
        question: QuestionId,
        option: String,
    },
}

/// Operational failure of the backend call. None of these is a judgment.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum BackendFailure {
    #[error("backend call exceeded the remaining deadline")]
    Timeout,
    #[error("backend transport failed: {message}")]
    Transport { message: String },
    #[error("backend rejected the credential")]
    Unauthorized,
    #[error("backend credential is unavailable: {message}")]
    CredentialUnavailable { message: String },
    #[error("backend rejected the request as invalid: {message}")]
    InvalidRequestRejected { message: String },
    #[error("backend rate limit exceeded")]
    RateLimited,
    #[error("backend is overloaded")]
    Overloaded,
    #[error("backend returned status {status}: {message}")]
    ServiceError { status: u16, message: String },
    #[error("backend response could not be decoded: {message}")]
    InvalidResponse { message: String },
    #[error("provider request failed: {message}")]
    Provider { message: String },
}

impl BackendFailure {
    /// Whether waiting and trying again within the same deadline may help.
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::RateLimited | Self::Overloaded)
    }
}

/// Why the service could not evaluate at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DecisionUnavailableReason {
    #[error("the decision capability is disabled by config.tools.decision_enabled")]
    Disabled,
    #[error("backend {backend:?} is selected but not configured")]
    BackendNotConfigured { backend: BackendKind },
    #[error("disclosure of decision inputs to backend {backend:?} is not permitted by the host")]
    DisclosureNotPermitted { backend: BackendKind },
    #[error("credential source kind `{kind}` is not supported for backend {backend:?}")]
    CredentialSourceUnsupported { backend: BackendKind, kind: String },
    #[error("backend {backend:?} is not compiled into this build")]
    BackendNotCompiled { backend: BackendKind },
}

/// Stable machine-readable code for a [`DecisionError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DecisionErrorCode {
    InvalidRequest,
    Unavailable,
    BackendFailure,
    InvalidAnswer,
    DeadlineExceeded,
    BudgetRefused,
}

impl DecisionErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Unavailable => "unavailable",
            Self::BackendFailure => "backend_failure",
            Self::InvalidAnswer => "invalid_answer",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::BudgetRefused => "budget_refused",
        }
    }
}

/// Every way an evaluation can fail.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DecisionError {
    #[error("invalid decision request: {0}")]
    InvalidRequest(RequestValidationError),
    #[error("decision service unavailable: {0}")]
    Unavailable(DecisionUnavailableReason),
    #[error("decision backend failed: {0}")]
    BackendFailure(BackendFailure),
    #[error("decision backend returned an invalid answer: {0}")]
    InvalidAnswer(AnswerValidationError),
    #[error("decision evaluation exceeded its {deadline_ms} ms deadline")]
    DeadlineExceeded { deadline_ms: u64 },
    #[error("decision refused by the caller's token budget: {used} of {limit} tokens used")]
    BudgetRefused { used: u64, limit: u64 },
}

impl DecisionError {
    pub const fn code(&self) -> DecisionErrorCode {
        match self {
            Self::InvalidRequest(_) => DecisionErrorCode::InvalidRequest,
            Self::Unavailable(_) => DecisionErrorCode::Unavailable,
            Self::BackendFailure(_) => DecisionErrorCode::BackendFailure,
            Self::InvalidAnswer(_) => DecisionErrorCode::InvalidAnswer,
            Self::DeadlineExceeded { .. } => DecisionErrorCode::DeadlineExceeded,
            Self::BudgetRefused { .. } => DecisionErrorCode::BudgetRefused,
        }
    }
}

impl From<RequestValidationError> for DecisionError {
    fn from(error: RequestValidationError) -> Self {
        Self::InvalidRequest(error)
    }
}

impl From<AnswerValidationError> for DecisionError {
    fn from(error: AnswerValidationError) -> Self {
        Self::InvalidAnswer(error)
    }
}

impl From<BackendFailure> for DecisionError {
    fn from(error: BackendFailure) -> Self {
        Self::BackendFailure(error)
    }
}

impl From<DecisionUnavailableReason> for DecisionError {
    fn from(reason: DecisionUnavailableReason) -> Self {
        Self::Unavailable(reason)
    }
}

impl From<meerkat_core::BudgetExceeded> for DecisionError {
    fn from(exceeded: meerkat_core::BudgetExceeded) -> Self {
        Self::BudgetRefused {
            used: exceeded.used,
            limit: exceeded.limit,
        }
    }
}
