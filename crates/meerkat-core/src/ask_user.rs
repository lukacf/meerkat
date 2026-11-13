//! The `ask_user` tool: an agent asks a person a set of multiple-choice
//! questions.
//!
//! This is the shape of the tool only: the vocabulary, its bounds as
//! constants, the host seam and the dispatcher. Nothing is validated, the
//! host is not asked, and every result is an empty `asked`. The rows in
//! `tests.rs` state what the tool does once it is built.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::agent::{AgentToolDispatcher, ToolDispatchContext};
use crate::error::ToolError;
use crate::ops::ToolDispatchOutcome;
use crate::types::{ToolCallView, ToolDef, ToolResult};
use crate::{RunId, ToolCatalogCapabilities};

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests;

/// The name of the tool an agent asks a person with.
pub const ASK_USER_TOOL: &str = "ask_user";

/// The label of the input that carries a request's [`AskUserResult`] to the
/// asking session once the request ended.
pub const ASK_USER_RESULT_INPUT: &str = "ask_user_result";

/// Fewest questions of one set.
pub const ASK_USER_QUESTIONS_MIN: usize = 1;
/// Most questions of one set.
pub const ASK_USER_QUESTIONS_MAX: usize = 10;
/// Fewest choices of one question.
pub const ASK_USER_CHOICES_MIN: usize = 2;
/// Most choices of one question.
pub const ASK_USER_CHOICES_MAX: usize = 8;
/// Largest question or choice id, in bytes of `a-z`, `0-9`, `_` and `-`.
pub const ASK_USER_ID_MAX_BYTES: usize = 32;
/// Largest question prompt, in bytes.
pub const ASK_USER_PROMPT_MAX_BYTES: usize = 1024;
/// Largest choice label, in bytes.
pub const ASK_USER_LABEL_MAX_BYTES: usize = 256;
/// Largest title of a set, in bytes.
pub const ASK_USER_TITLE_MAX_BYTES: usize = 200;
/// Largest written answer, in bytes after trimming.
pub const ASK_USER_FREE_TEXT_MAX_BYTES: usize = 280;
/// Shortest time a request waits for its answers, in seconds.
pub const ASK_USER_TIMEOUT_MIN_SECONDS: u64 = 60;
/// Longest time a request waits for its answers, in seconds (7 days).
pub const ASK_USER_TIMEOUT_MAX_SECONDS: u64 = 604_800;
/// The time a request waits for its answers when the call names none, in
/// seconds (1 hour).
pub const ASK_USER_TIMEOUT_DEFAULT_SECONDS: u64 = 3_600;

/// A question set, a timeout or a set of answers breaks one of the tool's
/// rules. Each variant is one rule, and names what broke it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AskUserInvalid {
    /// The set has too few or too many questions.
    #[error("a set has {count} questions")]
    QuestionCount { count: usize },
    /// A question has too few or too many choices.
    #[error("the question `{question_id}` has {count} choices")]
    ChoiceCount { question_id: String, count: usize },
    /// A question id is not 1 to 32 bytes of `a-z`, `0-9`, `_` and `-`.
    #[error("the question id `{id}` is not valid")]
    QuestionId { id: String },
    /// A choice id is not 1 to 32 bytes of `a-z`, `0-9`, `_` and `-`.
    #[error("the choice id `{id}` of the question `{question_id}` is not valid")]
    ChoiceId { question_id: String, id: String },
    /// Two questions of the set have one id.
    #[error("the question id `{id}` is used twice")]
    DuplicateQuestionId { id: String },
    /// Two choices of one question have one id.
    #[error("the choice id `{id}` is used twice in the question `{question_id}`")]
    DuplicateChoiceId { question_id: String, id: String },
    /// A prompt is empty.
    #[error("the question `{question_id}` has an empty prompt")]
    EmptyPrompt { question_id: String },
    /// A prompt is longer than its bound.
    #[error("the prompt of the question `{question_id}` is {bytes} bytes")]
    PromptTooLong { question_id: String, bytes: usize },
    /// A label is empty.
    #[error("the choice `{choice_id}` of the question `{question_id}` has an empty label")]
    EmptyLabel {
        question_id: String,
        choice_id: String,
    },
    /// A label is longer than its bound.
    #[error(
        "the label of the choice `{choice_id}` of the question `{question_id}` is {bytes} bytes"
    )]
    LabelTooLong {
        question_id: String,
        choice_id: String,
        bytes: usize,
    },
    /// The title is longer than its bound.
    #[error("the title is {bytes} bytes")]
    TitleTooLong { bytes: usize },
    /// `recommended` names no choice of its question.
    #[error("the question `{question_id}` recommends `{recommended}`")]
    UnknownRecommended {
        question_id: String,
        recommended: String,
    },
    /// The timeout is outside its range.
    #[error("`timeout_seconds` is {seconds}")]
    TimeoutOutOfRange { seconds: u64 },
    /// The answers answer no question.
    #[error("the answers answer no question")]
    NoAnswers,
    /// An answer names a question the set does not have.
    #[error("an answer names the question `{question_id}`")]
    UnknownQuestion { question_id: String },
    /// Two answers name one question.
    #[error("the question `{question_id}` is answered twice")]
    RepeatedQuestion { question_id: String },
    /// An answer says `unanswered`.
    #[error("the answer to the question `{question_id}` is `unanswered`")]
    UnansweredGiven { question_id: String },
    /// An answer names a choice its question does not have.
    #[error("the question `{question_id}` has no choice `{choice_id}`")]
    UnknownChoice {
        question_id: String,
        choice_id: String,
    },
    /// A written answer to a question that accepts none.
    #[error("the question `{question_id}` accepts no written answer")]
    FreeTextNotAllowed { question_id: String },
    /// A written answer is empty once trimmed.
    #[error("the written answer to the question `{question_id}` is empty")]
    EmptyFreeText { question_id: String },
    /// A written answer is longer than its bound.
    #[error("the written answer to the question `{question_id}` is {bytes} bytes")]
    FreeTextTooLong { question_id: String, bytes: usize },
}

/// The time a request waits for its answers. Not built: every value is
/// taken.
///
/// # Errors
/// None yet.
pub fn ask_user_timeout(timeout_seconds: Option<u64>) -> Result<Duration, AskUserInvalid> {
    Ok(Duration::from_secs(timeout_seconds.unwrap_or(0)))
}

/// One choice of a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserChoice {
    /// The choice's id, unique in its question.
    pub id: String,
    /// What the person reads for the choice.
    pub label: String,
}

/// One question of a set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserQuestion {
    /// The question's id, unique in its set.
    pub id: String,
    /// What the person is asked.
    pub prompt: String,
    /// The choices the person picks one of.
    pub choices: Vec<AskUserChoice>,
    /// Whether the person may write one answer instead of picking a choice.
    #[serde(default)]
    pub allow_free_text: bool,
    /// The id of the choice the agent would pick, if it has a view.
    #[serde(default)]
    pub recommended: Option<String>,
}

/// A set of questions one call asks a person. Not built: nothing is
/// validated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionSet {
    title: Option<String>,
    questions: Vec<AskUserQuestion>,
}

impl AskUserQuestionSet {
    /// The set of `questions` under `title`.
    ///
    /// # Errors
    /// None yet.
    pub fn new(
        title: Option<String>,
        questions: Vec<AskUserQuestion>,
    ) -> Result<Self, AskUserInvalid> {
        Ok(Self { title, questions })
    }

    /// The set's title, if it has one.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The questions, in the order they are asked.
    #[must_use]
    pub fn questions(&self) -> &[AskUserQuestion] {
        &self.questions
    }

    /// The question `question_id` names. Not built: none.
    #[must_use]
    pub fn question(&self, question_id: &str) -> Option<&AskUserQuestion> {
        let _ = question_id;
        None
    }
}

/// The answer to one question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskUserAnswer {
    /// The person picked the choice `choice_id`.
    Choice { choice_id: String },
    /// The person wrote `text` instead of picking a choice.
    FreeText { text: String },
    /// Nobody answered the question.
    Unanswered,
}

/// One question's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionAnswer {
    /// The question answered.
    pub question_id: String,
    /// Its answer.
    pub answer: AskUserAnswer,
}

/// The answers a person gave to one question set. Not built: nothing is
/// validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskUserAnswers {
    answers: Vec<AskUserQuestionAnswer>,
}

impl AskUserAnswers {
    /// The answers a person gave to `set`.
    ///
    /// # Errors
    /// None yet.
    pub fn for_set(
        set: &AskUserQuestionSet,
        answers: Vec<AskUserQuestionAnswer>,
    ) -> Result<Self, AskUserInvalid> {
        let _ = set;
        Ok(Self { answers })
    }

    /// The answers the person gave, in the order they gave them.
    #[must_use]
    pub fn answers(&self) -> &[AskUserQuestionAnswer] {
        &self.answers
    }

    /// The answer to every question of `set`. Not built: the answers as
    /// given.
    #[must_use]
    pub fn complete(&self, set: &AskUserQuestionSet) -> Vec<AskUserQuestionAnswer> {
        let _ = set;
        self.answers.clone()
    }
}

/// Every question of `set` as `unanswered`. Not built: none.
#[must_use]
pub fn ask_user_unanswered(set: &AskUserQuestionSet) -> Vec<AskUserQuestionAnswer> {
    let _ = set;
    Vec::new()
}

/// Where a request stands, as its result says. More outcomes may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AskUserOutcome {
    /// A person is being asked: the answers arrive later.
    Asked,
    /// A person answered.
    Answered,
    /// The person closed the questions without answering.
    Dismissed,
    /// Nobody answered before the request's deadline.
    TimedOut,
    /// The request was withdrawn before a person answered it.
    Withdrawn,
    /// The host has no surface that shows questions to a person.
    NoHumanSurface,
}

impl AskUserOutcome {
    /// The outcome's stable token (`asked`, `answered`, ...).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asked => "asked",
            Self::Answered => "answered",
            Self::Dismissed => "dismissed",
            Self::TimedOut => "timed_out",
            Self::Withdrawn => "withdrawn",
            Self::NoHumanSurface => "no_human_surface",
        }
    }
}

/// What an agent reads of one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserResult {
    /// Where the request stands.
    pub outcome: AskUserOutcome,
    /// The request's id.
    pub request_id: Option<String>,
    /// When a pending request ends unanswered, in RFC 3339 UTC.
    pub answer_by: Option<String>,
    /// The answer to every question of an ended request.
    pub answers: Vec<AskUserQuestionAnswer>,
    /// What the outcome means and what happens next.
    pub note: String,
}

impl AskUserResult {
    /// Not built: an `asked` result with empty fields.
    fn empty() -> Self {
        Self {
            outcome: AskUserOutcome::Asked,
            request_id: None,
            answer_by: None,
            answers: Vec::new(),
            note: String::new(),
        }
    }

    /// The result of a call whose request is pending. Not built.
    #[must_use]
    pub fn asked(request_id: impl Into<String>, answer_by_ms: u64) -> Self {
        let _ = (request_id.into(), answer_by_ms);
        Self::empty()
    }

    /// The result of a call on a host that cannot ask a person. Not built.
    #[must_use]
    pub fn no_human_surface() -> Self {
        Self::empty()
    }

    /// The result of a request that ended. Not built.
    #[must_use]
    pub fn ended(
        outcome: AskUserOutcome,
        request_id: impl Into<String>,
        answers: Vec<AskUserQuestionAnswer>,
    ) -> Self {
        let _ = (outcome, request_id.into(), answers);
        Self::empty()
    }
}

/// One request an agent asks through [`ASK_USER_TOOL`].
#[derive(Debug, Clone, Copy)]
pub struct AskUserRequest<'a> {
    /// The questions.
    pub questions: &'a AskUserQuestionSet,
    /// How long the request waits for its answers.
    pub timeout: Duration,
    /// The run the call belongs to.
    pub run_id: &'a RunId,
    /// The provider's call id.
    pub call_id: &'a str,
}

/// A host did not take a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AskUserRefusal {
    /// The request is not one the host holds.
    #[error("{0}")]
    Invalid(String),
    /// The host could not hold the request now.
    #[error("{0}")]
    Failed(String),
}

/// Where a host takes the questions its agents ask.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait AskUserHost: Send + Sync {
    /// Hold `request` for a person and answer at once.
    async fn ask(&self, request: AskUserRequest<'_>) -> Result<AskUserResult, AskUserRefusal>;
}

/// A host with no surface that shows questions to a person.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAskUserSurface;

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AskUserHost for NoAskUserSurface {
    async fn ask(&self, _request: AskUserRequest<'_>) -> Result<AskUserResult, AskUserRefusal> {
        Ok(AskUserResult::no_human_surface())
    }
}

/// The `ask_user` tool over a host. Not built: the host is not asked.
pub struct AskUserDispatcher {
    host: Arc<dyn AskUserHost>,
    tools: Arc<[Arc<ToolDef>]>,
}

impl AskUserDispatcher {
    /// The `ask_user` tool whose questions `host` takes.
    #[must_use]
    pub fn new(host: Arc<dyn AskUserHost>) -> Self {
        Self {
            host,
            tools: Arc::from([Arc::new(ToolDef::new(
                ASK_USER_TOOL,
                "Ask a person.",
                serde_json::json!({"type": "object"}),
            ))]),
        }
    }

    fn ask(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let _ = (&self.host, context);
        let text = serde_json::to_string(&AskUserResult::empty())
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        Ok(ToolDispatchOutcome::from(ToolResult::new(
            call.id.to_owned(),
            text,
            false,
        )))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for AskUserDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.ask(call, &ToolDispatchContext::default())
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.ask(call, context)
    }
}
