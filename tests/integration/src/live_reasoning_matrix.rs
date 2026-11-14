//! The live reasoning-effort matrix: everything the ignored live rows
//! (`copilot_live_reasoning_effort_matrix`,
//! `direct_provider_live_reasoning_effort`) do apart from reaching an
//! account, kept in this library so its rules are tested without a provider.
//!
//! A run is a list of cells, one per provider family and API. For each cell
//! the driver checks, without any provider request, that a level outside the
//! route's set and a model whose source states no levels are refused, and
//! then sends requests for one model through a real agent loop on the
//! route's own client. The rules, each enforced here and pinned by a unit
//! row below:
//!
//! - **Budget.** A cell sends at most [`REQUESTS_PER_CELL`] provider
//!   requests. [`GatedClient`] stands between the agent loop and the route's
//!   client and counts every request before it is sent; the request over the
//!   budget is refused there and never reaches the client.
//! - **Stop.** The first authentication failure (HTTP 401), refusal of the
//!   credential (403), rate limit (429), exhausted quota or policy stop the
//!   client reports ends the run: the gate refuses every later request of
//!   every cell, a retry of the failed one included.
//! - **Deadline.** No cell starts after the run's deadline.
//! - **Privacy.** A line is built from typed fields only (route, model id,
//!   body shape, levels, failure class, HTTP status): never from a provider's
//!   error text. Every line then passes the [`Redactor`], which replaces the
//!   account's private values and anything shaped like a GitHub token with
//!   [`REDACTED`].
//! - **Receipt.** Every cell a run reaches is also printed as one line the
//!   reasoning effort execution receipt reads: [`RECEIPT_OBSERVATION_PREFIX`]
//!   and one JSON object of typed fields. A failed cell carries the HTTP
//!   status of its failure class and the class as its error code, and
//!   nothing else of the failure.
//!
//! The gate counts requests at the client seam. What a provider client does
//! below that seam is its own: the operator note of the live rows states it.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use meerkat::{
    AgentBuildConfig, AgentFactory, AgentToolDispatcher, BuildAgentError, DynAgent, MemoryStore,
    ToolDef, ToolError,
};
use meerkat_core::lifecycle::run_primitive::{
    AnthropicThinkingConfig, ProviderParamsOverride, ProviderTag,
};
use meerkat_core::model_profile::capabilities::EffortLevel;
use meerkat_core::{
    AgentError, AuthBindingRef, AuthError, Config, ModelApi, Provider, ReasoningEffortRefusal,
    ReasoningEffortRefusalReason, ReasoningEffortReport, ReasoningFactsSource, ReasoningLevelFacts,
    ReasoningRouteObservation, ToolCallView, ToolDispatchOutcome, ToolResult,
};
use meerkat_llm_core::provider_runtime::ProviderAuthError;
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmReplayProjection, LlmRequest, LlmStream,
    PlainModelRoute, PreparedLlmRequest,
};
use serde::Serialize;
use serde_json::json;

/// What stands in a line for a private value.
pub const REDACTED: &str = "<redacted>";

/// What a line starts with that carries one cell of a run for the reasoning
/// effort execution receipt. The rest of the line is one JSON object, a
/// [`ReceiptObservation`].
pub const RECEIPT_OBSERVATION_PREFIX: &str = "reasoning-effort-receipt-observation ";

/// The most provider requests one cell (a provider family on one API) sends.
/// One request is one streamed turn with the probe tool offered. A tool call
/// needs a second request for its result and a structured-output extraction
/// a third, so at this bound the live rows prove neither: the scripted route
/// rows and the built-binary rows do.
pub const REQUESTS_PER_CELL: u32 = 1;

/// The name of the tool every probe request offers.
pub const PROBE_TOOL: &str = "live_probe";

/// The prompt of a probe turn: an answer the model gives in one request.
const PROBE_PROMPT: &str = "Reply with only the word: ready. Do not call any tool.";

/// The text of the error a refused request fails with. Fixed: it names no
/// account, model or provider.
const GATE_REFUSAL: &str = "live matrix gate: no further provider request is sent";

const LADDER: [EffortLevel; 7] = [
    EffortLevel::None,
    EffortLevel::Minimal,
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
    EffortLevel::Max,
];

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// The prefixes GitHub tokens start with.
const TOKEN_PREFIXES: [&str; 6] = ["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];

/// Replaces an account's private values in a line with [`REDACTED`].
///
/// The values are whatever names the account: its login, its plan, the API
/// host it is served on. They are held only in the process that resolved the
/// account and are never written anywhere. Matching ignores ASCII case, and
/// where two values start at the same place the longer one is replaced.
#[derive(Clone, Default)]
pub struct Redactor {
    /// Lower-case, longest first, none empty.
    needles: Vec<String>,
}

impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Redactor({} private values)", self.needles.len())
    }
}

impl Redactor {
    /// A redactor for `private_values`. A value that is empty once trimmed
    /// is left out: it names nothing.
    pub fn new(private_values: impl IntoIterator<Item = String>) -> Self {
        let mut needles: Vec<String> = private_values
            .into_iter()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        needles.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        needles.dedup();
        Self { needles }
    }

    /// A redactor for an account identity: its login, its plan, the API base
    /// it is served on and that base's host.
    pub fn for_account(login: Option<&str>, plan: Option<&str>, api_base: Option<&str>) -> Self {
        let host = api_base.map(|base| {
            let rest = base.split_once("://").map_or(base, |(_, rest)| rest);
            rest.split(['/', ':', '?', '#']).next().unwrap_or(rest)
        });
        Self::new(
            [login, plan, api_base, host]
                .into_iter()
                .flatten()
                .map(str::to_string),
        )
    }

    /// `text` with every private value and every token-shaped word replaced
    /// by [`REDACTED`].
    pub fn redact(&self, text: &str) -> String {
        let lower = text.to_ascii_lowercase();
        let mut out = String::with_capacity(text.len());
        let mut index = 0;
        while index < text.len() {
            if let Some(needle) = self
                .needles
                .iter()
                .find(|needle| lower[index..].starts_with(needle.as_str()))
            {
                out.push_str(REDACTED);
                index += needle.len();
                continue;
            }
            // `index` is a char boundary: it starts at 0 and advances by a
            // whole char or by a matched needle, which is valid UTF-8.
            let Some(character) = text[index..].chars().next() else {
                break;
            };
            out.push(character);
            index += character.len_utf8();
        }
        scrub_token_shapes(&out)
    }
}

/// `text` with every word shaped like a GitHub token (one of the known
/// prefixes and the token characters after it) replaced by [`REDACTED`].
pub fn scrub_token_shapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let found = TOKEN_PREFIXES
            .iter()
            .filter_map(|prefix| rest.find(prefix).map(|at| (at, prefix.len())))
            .min();
        let Some((at, prefix_len)) = found else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at]);
        out.push_str(REDACTED);
        let after = &rest[at + prefix_len..];
        let token_len = after
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(after.len());
        rest = &after[token_len..];
    }
}

// ---------------------------------------------------------------------------
// Cells, shapes and the plan
// ---------------------------------------------------------------------------

/// One cell of the matrix: a provider family on one provider API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellKey {
    pub provider: Provider,
    pub api: ModelApi,
}

/// Which lane a run's lines belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// The GitHub Copilot account routes.
    Copilot,
    /// The direct provider APIs.
    Direct,
}

impl Lane {
    /// The words every line of the lane starts with.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Copilot => "copilot-live reasoning",
            Self::Direct => "direct-live reasoning",
        }
    }

    /// The row of the reasoning effort execution receipt the lane's cells
    /// are observations of.
    pub const fn receipt_row(self) -> &'static str {
        match self {
            Self::Copilot => "live.copilot_matrix",
            Self::Direct => "live.direct_providers",
        }
    }
}

/// The request body shape a level is sent in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyShape {
    /// Copilot Responses, a model with a catalog reasoning row: the full
    /// reasoning payload with the level as its effort.
    FullPayload,
    /// Copilot Responses, a model without a catalog reasoning row: the
    /// effort alone.
    EffortOnly,
    /// Copilot Messages: adaptive thinking with the output effort, always
    /// the pair.
    MessagesAdaptiveEffort,
    /// Copilot Chat Completions: both effort fields.
    ChatDouble,
    /// The direct Responses API.
    ResponsesEffort,
    /// The direct Messages API.
    MessagesEffort,
    /// The direct `generateContent` API.
    GenerateContentThinkingLevel,
}

impl BodyShape {
    /// The shape a level takes on a route: `copilot` for a Copilot backend,
    /// `catalog_reasoning_row` for a model whose catalog row takes
    /// reasoning. `None` for an API that takes no level.
    pub const fn of(copilot: bool, api: ModelApi, catalog_reasoning_row: bool) -> Option<Self> {
        Some(match (copilot, api) {
            (true, ModelApi::Responses) if catalog_reasoning_row => Self::FullPayload,
            (true, ModelApi::Responses) => Self::EffortOnly,
            (true, ModelApi::Messages) => Self::MessagesAdaptiveEffort,
            (true, ModelApi::ChatCompletions) => Self::ChatDouble,
            (false, ModelApi::Responses) => Self::ResponsesEffort,
            (false, ModelApi::Messages) => Self::MessagesEffort,
            (false, ModelApi::GenerateContent) => Self::GenerateContentThinkingLevel,
            (true, ModelApi::GenerateContent)
            | (false, ModelApi::ChatCompletions)
            | (_, ModelApi::Realtime) => return None,
        })
    }

    /// The name a line gives the shape.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullPayload => "full-payload",
            Self::EffortOnly => "effort-only",
            Self::MessagesAdaptiveEffort => "messages-adaptive-effort",
            Self::ChatDouble => "chat-double",
            Self::ResponsesEffort => "responses-effort",
            Self::MessagesEffort => "messages-effort",
            Self::GenerateContentThinkingLevel => "generate-content-thinking-level",
        }
    }

    /// The wire paths a run reports the level in for this shape.
    pub const fn fields(self) -> &'static [&'static str] {
        match self {
            Self::FullPayload | Self::EffortOnly | Self::ResponsesEffort => &["reasoning.effort"],
            Self::MessagesAdaptiveEffort => &["thinking.type", "output_config.effort"],
            Self::ChatDouble => &["reasoning_effort", "reasoning.effort"],
            Self::MessagesEffort => &["output_config.effort"],
            Self::GenerateContentThinkingLevel => {
                &["generationConfig.thinkingConfig.thinkingLevel"]
            }
        }
    }
}

/// What a cell does with the levels its route accepts for a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellPlan {
    /// The levels to send, one request each: at most `requests` of them.
    pub levels: Vec<EffortLevel>,
    /// A level outside the set, to be refused without a request. `None`
    /// when the set is the whole ladder.
    pub refused: Option<EffortLevel>,
}

/// The plan for a model whose route accepts `supported`, within a budget of
/// `requests` provider requests: the lowest level at or above `low` (the
/// lowest of all when there is none), then the highest, as far as the budget
/// goes; and the lowest level outside the set, at or above `low` where there
/// is one.
pub fn plan_cell(supported: &[EffortLevel], requests: u32) -> CellPlan {
    let mut set = supported.to_vec();
    set.sort();
    set.dedup();
    let lowest = set
        .iter()
        .copied()
        .find(|level| *level >= EffortLevel::Low)
        .or_else(|| set.first().copied());
    let mut levels: Vec<EffortLevel> = lowest.into_iter().chain(set.last().copied()).collect();
    levels.dedup();
    levels.truncate(usize::try_from(requests).unwrap_or(usize::MAX));
    let outside = |level: &EffortLevel| !set.contains(level);
    let refused = LADDER
        .into_iter()
        .find(|level| *level >= EffortLevel::Low && outside(level))
        .or_else(|| LADDER.into_iter().find(outside));
    CellPlan { levels, refused }
}

// ---------------------------------------------------------------------------
// Failure classes
// ---------------------------------------------------------------------------

/// Why a cell did not pass: the class a line names. A closed set of fixed
/// words, so a line never carries a provider's own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    // What the route's client reported.
    AuthenticationFailed,
    Forbidden,
    RateLimited,
    QuotaExhausted,
    PolicyStop,
    InvalidRequest,
    ModelNotFound,
    ServerError,
    ServerOverloaded,
    Network,
    IncompleteResponse,
    ContentFiltered,
    LocalRefusal,
    ProviderOther,
    // What the harness found.
    BuildFailed,
    TurnFailed,
    TurnTimeout,
    BudgetSpent,
    ReportMismatch,
    StatedLevelRefused,
    NotRefused,
    RefusalMismatch,
    RefusalDispatched,
    EffortWithoutAdaptiveThinking,
}

impl FailureClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "authentication_failed",
            Self::Forbidden => "forbidden",
            Self::RateLimited => "rate_limited",
            Self::QuotaExhausted => "quota_exhausted",
            Self::PolicyStop => "policy_stop",
            Self::InvalidRequest => "invalid_request",
            Self::ModelNotFound => "model_not_found",
            Self::ServerError => "server_error",
            Self::ServerOverloaded => "server_overloaded",
            Self::Network => "network",
            Self::IncompleteResponse => "incomplete_response",
            Self::ContentFiltered => "content_filtered",
            Self::LocalRefusal => "local_refusal",
            Self::ProviderOther => "provider_other",
            Self::BuildFailed => "build_failed",
            Self::TurnFailed => "turn_failed",
            Self::TurnTimeout => "turn_timeout",
            Self::BudgetSpent => "request_budget_spent",
            Self::ReportMismatch => "report_mismatch",
            Self::StatedLevelRefused => "stated_level_refused",
            Self::NotRefused => "not_refused",
            Self::RefusalMismatch => "refusal_mismatch",
            Self::RefusalDispatched => "refusal_dispatched",
            Self::EffortWithoutAdaptiveThinking => "effort_without_adaptive_thinking",
        }
    }
}

/// A failure the route's client reported: its class and the HTTP status the
/// client's own classification gives that class
/// (`LlmError::from_http_status`), when the class has exactly one. Nothing
/// of the provider's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderFailure {
    pub class: FailureClass,
    pub status: Option<u16>,
}

impl ProviderFailure {
    /// Whether the failure ends the run: an authentication failure (401), a
    /// refused credential (403), a rate limit (429), exhausted quota (the
    /// class providers front with 429 or 402) or a policy stop.
    pub const fn stops_the_run(self) -> bool {
        matches!(
            self.class,
            FailureClass::AuthenticationFailed
                | FailureClass::Forbidden
                | FailureClass::RateLimited
                | FailureClass::QuotaExhausted
                | FailureClass::PolicyStop
        )
    }
}

/// The class and status of an error of the route's client. The error's own
/// message is never read.
pub fn classify(error: &LlmError) -> ProviderFailure {
    let (class, status) = match error {
        LlmError::AuthenticationFailed { .. } => (FailureClass::AuthenticationFailed, Some(401)),
        LlmError::InvalidApiKey => (FailureClass::Forbidden, Some(403)),
        LlmError::RateLimited { .. } => (FailureClass::RateLimited, Some(429)),
        LlmError::QuotaExhausted { .. } => (FailureClass::QuotaExhausted, None),
        LlmError::PolicyStop { .. } => (FailureClass::PolicyStop, None),
        LlmError::ModelNotFound { .. } => (FailureClass::ModelNotFound, Some(404)),
        LlmError::ServerOverloaded => (FailureClass::ServerOverloaded, Some(503)),
        LlmError::ServerError { status, .. } => (FailureClass::ServerError, Some(*status)),
        LlmError::NetworkTimeout { .. } | LlmError::ConnectionReset => {
            (FailureClass::Network, None)
        }
        LlmError::InvalidRequest { .. }
        | LlmError::RequestTooLarge { .. }
        | LlmError::InvalidInputShape { .. }
        | LlmError::InvalidConfig { .. }
        | LlmError::ContextLengthExceeded { .. } => (FailureClass::InvalidRequest, None),
        LlmError::ContentFiltered { .. } => (FailureClass::ContentFiltered, None),
        LlmError::IncompleteResponse { .. } | LlmError::StreamParseError { .. } => {
            (FailureClass::IncompleteResponse, None)
        }
        LlmError::OperationRefused { .. }
        | LlmError::OperationObservationUnavailable
        | LlmError::OperationAuthorizationUnavailable
        | LlmError::OperationReviewRefused { .. }
        | LlmError::ToolChoiceUnsupported { .. } => (FailureClass::LocalRefusal, None),
        _ => (FailureClass::ProviderOther, None),
    };
    ProviderFailure { class, status }
}

/// Why an account did not resolve, as a fixed word: a token GitHub rejected
/// (its identity lookup answered 401), an account GitHub or the provider
/// refused, a lookup that was rate limited or could not answer, or no token
/// at all. The failure's own text, which can name the account, is never
/// read.
pub fn account_failure_class(error: &ProviderAuthError) -> &'static str {
    match error {
        ProviderAuthError::Auth(AuthError::UserReauthRequired) => "token_rejected",
        ProviderAuthError::Auth(AuthError::RefreshFailed(_)) => "account_refused",
        ProviderAuthError::Auth(AuthError::SourceUnavailable { .. }) => {
            "rate_limited_or_unavailable"
        }
        ProviderAuthError::Auth(AuthError::MissingSecret) => "no_token",
        _ => "not_resolved",
    }
}

// ---------------------------------------------------------------------------
// The budget and the gate
// ---------------------------------------------------------------------------

/// Why the gate refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRefusal {
    /// The run stopped on an earlier failure.
    Stopped,
    /// The cell's requests are spent.
    CellBudgetSpent,
}

#[derive(Default)]
struct BudgetState {
    spent: Vec<(CellKey, u32)>,
    stop: Option<ProviderFailure>,
}

/// The counted request budget of one run, shared by every gate of the run:
/// at most `per_cell` requests per cell, and none at all once a failure that
/// stops the run was recorded.
pub struct RequestBudget {
    per_cell: u32,
    state: Mutex<BudgetState>,
}

impl RequestBudget {
    pub fn new(per_cell: u32) -> Arc<Self> {
        Arc::new(Self {
            per_cell,
            state: Mutex::new(BudgetState::default()),
        })
    }

    /// Count one request of `cell`, or refuse it. A refused request is not
    /// counted.
    pub fn admit(&self, cell: CellKey) -> Result<(), GateRefusal> {
        let mut state = lock(&self.state);
        if state.stop.is_some() {
            return Err(GateRefusal::Stopped);
        }
        let index = match state.spent.iter().position(|(key, _)| *key == cell) {
            Some(index) => index,
            None => {
                state.spent.push((cell, 0));
                state.spent.len() - 1
            }
        };
        let spent = &mut state.spent[index].1;
        if *spent >= self.per_cell {
            return Err(GateRefusal::CellBudgetSpent);
        }
        *spent += 1;
        Ok(())
    }

    /// Record a failure of the route's client. The first one that stops the
    /// run is kept, and from then on every request is refused.
    pub fn record_failure(&self, failure: ProviderFailure) {
        let mut state = lock(&self.state);
        if failure.stops_the_run() && state.stop.is_none() {
            state.stop = Some(failure);
        }
    }

    /// The failure the run stopped on.
    pub fn stop(&self) -> Option<ProviderFailure> {
        lock(&self.state).stop
    }

    /// The requests `cell` sent.
    pub fn spent(&self, cell: CellKey) -> u32 {
        lock(&self.state)
            .spent
            .iter()
            .find(|(key, _)| *key == cell)
            .map_or(0, |(_, spent)| *spent)
    }

    /// The requests the run sent.
    pub fn total_spent(&self) -> u32 {
        lock(&self.state)
            .spent
            .iter()
            .map(|(_, spent)| *spent)
            .sum()
    }

    /// The most requests a run over `cells` cells can send.
    pub fn ceiling(&self, cells: usize) -> u32 {
        self.per_cell
            .saturating_mul(u32::try_from(cells).unwrap_or(u32::MAX))
    }
}

/// What one gate saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GateRecord {
    /// Requests the gate passed on to the client.
    pub sent: u32,
    /// Requests the gate refused.
    pub refused: u32,
    /// Requests whose stream ended with the provider's successful terminal
    /// event.
    pub completed: u32,
    /// The last failure the client reported.
    pub failure: Option<ProviderFailure>,
    /// Whether a request on the Messages API carried an output effort
    /// without adaptive thinking.
    pub effort_without_adaptive_thinking: bool,
}

/// Whether `request` carries an Anthropic output effort without adaptive
/// thinking. On a Copilot Messages route the level is always sent as the
/// pair.
pub fn effort_without_adaptive_thinking(request: &LlmRequest) -> bool {
    match &request.provider_params {
        Some(ProviderTag::Anthropic(tag)) => {
            tag.effort.is_some() && tag.thinking != Some(AnthropicThinkingConfig::Adaptive)
        }
        _ => false,
    }
}

/// Stands between an agent loop and the client of a route. Every request is
/// counted against the run's [`RequestBudget`] before it is sent; a request
/// the budget refuses fails here and never reaches the client. Every failure
/// the client reports is classified and recorded on the budget, so the
/// failure that stops the run stops it for every gate. Everything that is
/// not a request is passed through unchanged. A health check is answered
/// here and never forwarded: it would be a request the budget does not
/// count.
pub struct GatedClient {
    inner: Arc<dyn LlmClient>,
    cell: CellKey,
    budget: Arc<RequestBudget>,
    record: Mutex<GateRecord>,
}

impl GatedClient {
    pub fn new(inner: Arc<dyn LlmClient>, cell: CellKey, budget: Arc<RequestBudget>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            cell,
            budget,
            record: Mutex::new(GateRecord::default()),
        })
    }

    /// What this gate saw so far.
    pub fn record(&self) -> GateRecord {
        *lock(&self.record)
    }

    fn admit(&self, request: &LlmRequest) -> Result<(), GateRefusal> {
        let admission = self.budget.admit(self.cell);
        let mut record = lock(&self.record);
        match admission {
            Ok(()) => {
                record.sent += 1;
                record.effort_without_adaptive_thinking |=
                    effort_without_adaptive_thinking(request);
            }
            Err(_) => record.refused += 1,
        }
        admission
    }

    fn failed(&self, error: &LlmError) {
        let failure = classify(error);
        lock(&self.record).failure = Some(failure);
        self.budget.record_failure(failure);
    }

    fn watch<'a>(&'a self, stream: LlmStream<'a>) -> LlmStream<'a> {
        Box::pin(stream.inspect(move |item| match item {
            Err(error) => self.failed(error),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => self.failed(error),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            }) => lock(&self.record).completed += 1,
            Ok(_) => {}
        }))
    }
}

fn refused_stream<'a>() -> LlmStream<'a> {
    Box::pin(futures::stream::once(async {
        Err(LlmError::InvalidRequest {
            message: GATE_REFUSAL.to_string(),
        })
    }))
}

#[async_trait]
impl LlmClient for GatedClient {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(logical_model)
    }

    fn reasoning_route(&self) -> Option<ReasoningRouteObservation> {
        self.inner.reasoning_route()
    }

    fn reasoning_route_of(
        &self,
        projection: &LlmReplayProjection,
    ) -> Option<ReasoningRouteObservation> {
        self.inner.reasoning_route_of(projection)
    }

    fn project_replay_request(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<LlmReplayProjection, LlmError> {
        self.inner.project_replay_request(messages)
    }

    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn output_token_limit(&self) -> Option<u32> {
        self.inner.output_token_limit()
    }

    fn prompt_token_limit(&self, output_budget: u32) -> Option<u32> {
        self.inner.prompt_token_limit(output_budget)
    }

    fn request_pressure(
        &self,
        request: &LlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, LlmError> {
        self.inner.request_pressure(request)
    }

    fn prepared_request_pressure(
        &self,
        request: &PreparedLlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, LlmError> {
        self.inner.prepared_request_pressure(request)
    }

    fn authored_cache_breakpoints(
        &self,
        request: &LlmRequest,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, LlmError> {
        self.inner.authored_cache_breakpoints(request, messages)
    }

    fn prepared_cache_breakpoints(
        &self,
        request: &PreparedLlmRequest,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, LlmError> {
        self.inner.prepared_cache_breakpoints(request, messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        match self.admit(request) {
            Ok(()) => self.watch(self.inner.stream(request)),
            Err(_) => refused_stream(),
        }
    }

    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        match self.admit(request.request()) {
            Ok(()) => self.watch(self.inner.stream_prepared(request)),
            Err(_) => refused_stream(),
        }
    }

    fn provider(&self) -> Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        self.inner.compile_schema(output_schema)
    }
}

// ---------------------------------------------------------------------------
// Lines
// ---------------------------------------------------------------------------

/// Why a cell sent nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The run stopped on an earlier failure.
    Stopped,
    /// The run's deadline passed.
    RunDeadline,
    /// The route serves no model whose source states levels.
    NoModelWithLevels,
    /// The route's source states that the model takes no level.
    ModelTakesNoLevel,
}

impl SkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "run_stopped",
            Self::RunDeadline => "run_deadline",
            Self::NoModelWithLevels => "no_model_with_stated_levels",
            Self::ModelTakesNoLevel => "model_takes_no_level",
        }
    }
}

/// One line of a run, as typed fields. [`MatrixLine::render`] is the only
/// way a line becomes text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatrixLine {
    /// Every level of the cell completed its turn in one request and the
    /// run reported it; the level outside the set was refused without a
    /// request.
    Pass {
        route: String,
        model: String,
        shape: BodyShape,
        levels: Vec<EffortLevel>,
        refused: Option<EffortLevel>,
        requests: u32,
    },
    /// A refusal that sends nothing was given as expected.
    Local {
        route: String,
        model: String,
        refused: ReasoningEffortRefusalReason,
    },
    Fail {
        route: String,
        model: String,
        shape: Option<BodyShape>,
        level: Option<EffortLevel>,
        class: FailureClass,
        status: Option<u16>,
        detail: Option<&'static str>,
    },
    /// The provider answered the cell's request and the turn then asked for
    /// another, which the budget refused. Neither a pass nor a failure.
    Cut {
        route: String,
        model: String,
        shape: BodyShape,
        level: EffortLevel,
    },
    Skip {
        route: String,
        reason: SkipReason,
    },
    /// What a cell did not reach, in counts.
    Note {
        route: String,
        stated_models: u32,
        probed_models: u32,
        stated_levels: u32,
        probed_levels: u32,
    },
    Stop {
        class: FailureClass,
        status: Option<u16>,
        requests: u32,
        ceiling: u32,
    },
    /// The account did not resolve, so the run sent no inference request.
    AccountStop {
        /// A word of [`account_failure_class`].
        class: &'static str,
    },
    Summary {
        pass: u32,
        fail: u32,
        cut: u32,
        skip: u32,
        local: u32,
        requests: u32,
        ceiling: u32,
    },
}

fn reason_word(reason: ReasoningEffortRefusalReason) -> &'static str {
    match reason {
        ReasoningEffortRefusalReason::UnsupportedLevel => "unsupported_level",
        ReasoningEffortRefusalReason::SupportedLevelsUnknown => "supported_levels_unknown",
        ReasoningEffortRefusalReason::SupportedLevelsUnavailable => "supported_levels_unavailable",
        ReasoningEffortRefusalReason::ConflictingProviderEffort => "conflicting_provider_effort",
        ReasoningEffortRefusalReason::ConflictingThinkingBudget => "conflicting_thinking_budget",
        ReasoningEffortRefusalReason::ConflictingThinkingType => "conflicting_thinking_type",
        ReasoningEffortRefusalReason::ConflictingReasoningBody => "conflicting_reasoning_body",
        ReasoningEffortRefusalReason::ConflictingReasoningMode => "conflicting_reasoning_mode",
    }
}

fn level_word(level: Option<EffortLevel>) -> &'static str {
    level.map_or("-", EffortLevel::as_wire_str)
}

fn status_word(status: Option<u16>) -> String {
    status.map_or_else(|| "-".to_string(), |status| status.to_string())
}

impl MatrixLine {
    /// The line as text, for `lane`. Built from the typed fields alone.
    pub fn render(&self, lane: Lane) -> String {
        let label = lane.label();
        match self {
            Self::Pass {
                route,
                model,
                shape,
                levels,
                refused,
                requests,
            } => {
                let levels = levels
                    .iter()
                    .map(|level| level.as_wire_str())
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "{label} PASS route={route} model={model} shape={} levels=[{levels}] \
                     refused={} requests={requests}",
                    shape.as_str(),
                    level_word(*refused),
                )
            }
            Self::Local {
                route,
                model,
                refused,
            } => format!(
                "{label} LOCAL route={route} model={model} refused={} requests=0",
                reason_word(*refused)
            ),
            Self::Fail {
                route,
                model,
                shape,
                level,
                class,
                status,
                detail,
            } => format!(
                "{label} FAIL route={route} model={model} shape={} level={} class={} status={} \
                 detail={}",
                shape.map_or("-", BodyShape::as_str),
                level_word(*level),
                class.as_str(),
                status_word(*status),
                detail.unwrap_or("-"),
            ),
            Self::Cut {
                route,
                model,
                shape,
                level,
            } => format!(
                "{label} CUT route={route} model={model} shape={} level={} \
                 reason=second_request_refused_by_budget first_request=answered",
                shape.as_str(),
                level.as_wire_str(),
            ),
            Self::Skip { route, reason } => {
                format!("{label} SKIP route={route} reason={}", reason.as_str())
            }
            Self::Note {
                route,
                stated_models,
                probed_models,
                stated_levels,
                probed_levels,
            } => format!(
                "{label} NOTE route={route} models_with_stated_levels={stated_models} \
                 models_probed={probed_models} levels_of_probed_model={stated_levels} \
                 levels_probed={probed_levels}"
            ),
            Self::Stop {
                class,
                status,
                requests,
                ceiling,
            } => format!(
                "{label} STOP class={} status={} requests={requests}/{ceiling} \
                 no further request is sent",
                class.as_str(),
                status_word(*status),
            ),
            Self::AccountStop { class } => format!(
                "{label} STOP stage=account class={class} requests=0 no inference request is sent"
            ),
            Self::Summary {
                pass,
                fail,
                cut,
                skip,
                local,
                requests,
                ceiling,
            } => format!(
                "{label} SUMMARY pass={pass} fail={fail} cut={cut} skip={skip} local={local} \
                 requests={requests}/{ceiling} account={REDACTED}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Receipt observations
// ---------------------------------------------------------------------------

/// What a cell did with the level it was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReceiptOutcome {
    /// A request with the level was sent.
    Sent { level: &'static str },
    /// Nothing was sent, for this reason: a word of the refusal reasons or
    /// of [`FailureClass`].
    Refused { reason: &'static str },
}

/// One cell of a run as the reasoning effort execution receipt reads it: an
/// item of a leg result's `observations`. Every member is a typed field and
/// none can hold a message. A failed cell carries the HTTP status of its
/// failure class, when the class has one, and the class as its error code:
/// a word of [`FailureClass`], never a provider's own text or code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReceiptObservation {
    /// The receipt row the cell belongs to ([`Lane::receipt_row`]).
    pub row: &'static str,
    pub provider_family: &'static str,
    pub backend: String,
    pub api: &'static str,
    pub model: String,
    pub requested_level: &'static str,
    pub outcome: ReceiptOutcome,
    /// `pass`, `fail` or `skip`.
    pub result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
}

impl ReceiptObservation {
    /// The observation as its line: the prefix and one JSON object, members
    /// in the order above.
    pub fn line(&self) -> String {
        // Fixed words, two strings and a number: this cannot fail to
        // serialize.
        let object = serde_json::to_string(self).unwrap_or_default();
        format!("{RECEIPT_OBSERVATION_PREFIX}{object}")
    }
}

/// The sink of the live rows. A receipt line goes to stdout, where the
/// receipt's reader takes a row's lines from; every other line goes to
/// stderr, as the live rows print theirs.
pub fn print_line(line: &str) {
    if line.starts_with(RECEIPT_OBSERVATION_PREFIX) {
        println!("{line}");
    } else {
        eprintln!("{line}");
    }
}

// ---------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------

/// A model of a cell: the session's model id, the route's own client for it,
/// the binding of the route (none for a provider's default route) and the
/// configuration its session is built with.
#[derive(Clone)]
pub struct CellModel {
    pub model: String,
    pub client: Arc<dyn LlmClient>,
    pub auth_binding: Option<AuthBindingRef>,
    pub config: Config,
}

/// The model a cell sends its requests for.
#[derive(Clone)]
pub struct ProbeModel {
    pub model: CellModel,
    pub shape: BodyShape,
    /// The levels the route accepts for the model, lowest first.
    pub supported: Vec<EffortLevel>,
}

/// One cell of a run.
#[derive(Clone)]
pub struct MatrixCell {
    pub key: CellKey,
    /// The route as a run reports it (`<backend>/<api>`).
    pub route: String,
    /// The route's backend, as the provider matrix names it.
    pub backend: String,
    /// The source of the route's levels.
    pub facts: ReasoningFactsSource,
    /// The model with stated levels, when the route serves one.
    pub probe: Option<ProbeModel>,
    /// A model whose source states no levels, when the route serves one.
    pub unknown: Option<CellModel>,
    /// How many models of the route state levels.
    pub stated_models: u32,
}

/// The cells of a run, from candidate models in order of preference. Each
/// candidate's route is read from its own client and its levels from the
/// route's own source, both without I/O, the way a session's resolution
/// reads them. Per route the first model whose source states levels is the
/// one probed and the first whose source states none is the one checked for
/// the refusal. A candidate whose client reports no route, whose API takes
/// no level, whose source states that it takes no level or whose account
/// list is not loaded is left out.
pub fn assemble_cells(candidates: Vec<(Provider, CellModel)>) -> Vec<MatrixCell> {
    let mut cells: Vec<MatrixCell> = Vec::new();
    for (provider, model) in candidates {
        let Some(observation) = model.client.reasoning_route() else {
            continue;
        };
        let row = meerkat_models::capabilities_for(provider, &model.model);
        let Some(shape) = BodyShape::of(
            observation.backend.is_copilot(),
            observation.api,
            row.is_some_and(|row| row.supports_reasoning),
        ) else {
            continue;
        };
        let key = CellKey {
            provider,
            api: observation.api,
        };
        let index = match cells.iter().position(|cell| cell.key == key) {
            Some(index) => index,
            None => {
                cells.push(MatrixCell {
                    key,
                    route: observation.route().to_string(),
                    backend: observation.backend.as_str().to_string(),
                    facts: observation.backend.levels_source(),
                    probe: None,
                    unknown: None,
                    stated_models: 0,
                });
                cells.len() - 1
            }
        };
        let cell = &mut cells[index];
        match meerkat_core::supported_reasoning_levels(provider, Some(&observation), row) {
            ReasoningLevelFacts::Stated { levels, .. } if !levels.is_empty() => {
                cell.stated_models += 1;
                if cell.probe.is_none() {
                    cell.probe = Some(ProbeModel {
                        model,
                        shape,
                        supported: levels,
                    });
                }
            }
            ReasoningLevelFacts::Unknown if cell.unknown.is_none() => {
                cell.unknown = Some(model);
            }
            _ => {}
        }
    }
    cells
}

/// The settings of one run.
pub struct MatrixRun {
    pub lane: Lane,
    /// Where the agents of the run keep their state: a directory of the
    /// run's own.
    pub state_root: PathBuf,
    pub budget: Arc<RequestBudget>,
    pub redactor: Redactor,
    /// How long one turn may take.
    pub turn_timeout: Duration,
    /// No cell starts at or after this instant.
    pub deadline: Instant,
    pub max_tokens: u32,
    /// The agent loop's own retries of a failed request. The live rows set
    /// none; whatever is set, the gate refuses a request over the budget.
    pub max_retries: u32,
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatrixSummary {
    pub pass: u32,
    pub fail: u32,
    pub cut: u32,
    pub skip: u32,
    pub local: u32,
    /// Provider requests the gates passed on.
    pub requests: u32,
    /// The most requests the run could send.
    pub ceiling: u32,
    /// The failure the run stopped on.
    pub stopped: Option<ProviderFailure>,
    /// Every line of the run, as printed.
    pub lines: Vec<String>,
}

/// Offers [`PROBE_TOOL`] and counts its calls.
#[derive(Default)]
struct OfferedTool {
    calls: Mutex<u32>,
}

#[async_trait]
impl AgentToolDispatcher for OfferedTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([Arc::new(ToolDef::new(
            PROBE_TOOL,
            "Record the exact nonce supplied by the caller.",
            json!({
                "type": "object",
                "properties": {"nonce": {"type": "string"}},
                "required": ["nonce"],
                "additionalProperties": false
            }),
        ))])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        *lock(&self.calls) += 1;
        Ok(ToolDispatchOutcome::from(ToolResult::new(
            call.id.to_string(),
            json!({"recorded": true}).to_string(),
            false,
        )))
    }
}

struct Emitter<'a> {
    run: &'a MatrixRun,
    sink: &'a (dyn Fn(&str) + Send + Sync),
    summary: MatrixSummary,
}

impl Emitter<'_> {
    /// Render, redact and hand out one line. The only way a line leaves the
    /// driver.
    fn emit(&mut self, line: &MatrixLine) {
        match line {
            MatrixLine::Pass { .. } => self.summary.pass += 1,
            MatrixLine::Fail { .. } => self.summary.fail += 1,
            MatrixLine::Cut { .. } => self.summary.cut += 1,
            MatrixLine::Skip { .. } => self.summary.skip += 1,
            MatrixLine::Local { .. } => self.summary.local += 1,
            MatrixLine::Note { .. }
            | MatrixLine::Stop { .. }
            | MatrixLine::AccountStop { .. }
            | MatrixLine::Summary { .. } => {}
        }
        let text = self.run.redactor.redact(&line.render(self.run.lane));
        (self.sink)(&text);
        self.summary.lines.push(text);
    }

    /// Hand out one cell of the run as its receipt line. It passes the
    /// redactor like every other line.
    fn observe(&mut self, observation: &ReceiptObservation) {
        let text = self.run.redactor.redact(&observation.line());
        (self.sink)(&text);
        self.summary.lines.push(text);
    }
}

/// The receipt observation of one model and level of `cell`. `failure` is
/// the class of a failed cell and the HTTP status of that class.
fn observation(
    run: &MatrixRun,
    cell: &MatrixCell,
    model: &str,
    requested: EffortLevel,
    outcome: ReceiptOutcome,
    failure: Option<(FailureClass, Option<u16>)>,
    skipped: bool,
) -> ReceiptObservation {
    ReceiptObservation {
        row: run.lane.receipt_row(),
        provider_family: cell.key.provider.as_str(),
        backend: cell.backend.clone(),
        api: cell.key.api.as_str(),
        model: model.to_string(),
        requested_level: requested.as_wire_str(),
        outcome,
        result: match (failure, skipped) {
            (Some(_), _) => "fail",
            (None, true) => "skip",
            (None, false) => "pass",
        },
        http_status: failure.and_then(|(_, status)| status),
        error_code: failure.map(|(class, _)| class.as_str()),
    }
}

/// What a failed refusal check did with its level: sent it when the check
/// reached the provider, and otherwise nothing, for the failure's class.
fn failed_refusal_outcome(class: FailureClass, level: EffortLevel) -> ReceiptOutcome {
    if class == FailureClass::RefusalDispatched {
        ReceiptOutcome::Sent {
            level: level.as_wire_str(),
        }
    } else {
        ReceiptOutcome::Refused {
            reason: class.as_str(),
        }
    }
}

async fn build_agent(
    run: &MatrixRun,
    key: CellKey,
    model: &CellModel,
    gate: Arc<GatedClient>,
    level: EffortLevel,
) -> Result<DynAgent, BuildAgentError> {
    let factory = AgentFactory::new(run.state_root.join("sessions"))
        .runtime_root(run.state_root.join("runtime"));
    let store = Arc::new(
        factory
            .build_store_adapter(Arc::new(MemoryStore::new()))
            .await,
    );
    let mut config = model.config.clone();
    config.model_fallback.enabled = Some(false);
    config.retry.max_retries = run.max_retries;
    config.retry.initial_delay = Duration::from_millis(1);
    config.retry.max_delay = Duration::from_millis(5);
    // The factory builds the session as it builds every session: on the
    // route's binding, so what it decides from the route (a Copilot route
    // sends no provider-native tool) is decided here too. The gate stands
    // where the route's client would, and a client handed to the factory
    // makes it resolve no credential.
    let mut build = AgentBuildConfig::new(model.model.clone());
    build.provider = Some(key.provider);
    build.auth_binding = model.auth_binding.clone();
    build.max_tokens = Some(run.max_tokens);
    build.provider_params = Some(ProviderParamsOverride {
        reasoning_effort: Some(level),
        ..Default::default()
    });
    build.llm_client_override = Some(gate);
    build.tool_dispatcher_override = Some(Arc::new(OfferedTool::default()));
    build.session_store_override = Some(store);
    factory.build_agent(build, &config).await
}

/// A session that must be refused, at its build or by its run, without a
/// provider request: the typed refusal, or why there is none.
async fn refused_without_request(
    run: &MatrixRun,
    key: CellKey,
    model: &CellModel,
    level: EffortLevel,
) -> Result<ReasoningEffortRefusal, FailureClass> {
    let before = run.budget.spent(key);
    let gate = GatedClient::new(Arc::clone(&model.client), key, Arc::clone(&run.budget));
    let refusal = match build_agent(run, key, model, Arc::clone(&gate), level).await {
        Err(BuildAgentError::ReasoningEffortRefused(refusal)) => Ok(*refusal),
        Err(_) => Err(FailureClass::BuildFailed),
        Ok(mut agent) => {
            match tokio::time::timeout(run.turn_timeout, agent.run(PROBE_PROMPT.to_string().into()))
                .await
            {
                Ok(Err(AgentError::ReasoningEffortRefused(refusal))) => Ok(*refusal),
                Ok(_) => Err(FailureClass::NotRefused),
                Err(_) => Err(FailureClass::TurnTimeout),
            }
        }
    };
    let record = gate.record();
    if run.budget.spent(key) != before || record.sent != 0 || record.refused != 0 {
        return Err(FailureClass::RefusalDispatched);
    }
    refusal
}

/// What one probe turn came to.
enum ProbeOutcome {
    Passed,
    Cut,
    Failed {
        class: FailureClass,
        status: Option<u16>,
        detail: Option<&'static str>,
        /// Whether the turn's request reached the route's client.
        sent: bool,
    },
}

fn failed(class: FailureClass, sent: bool) -> ProbeOutcome {
    ProbeOutcome::Failed {
        class,
        status: None,
        detail: None,
        sent,
    }
}

/// The first way the run's report differs from what one request at `level`
/// on the cell's route must report.
fn report_mismatch(
    report: Option<&ReasoningEffortReport>,
    cell: &MatrixCell,
    probe: &ProbeModel,
    level: EffortLevel,
) -> Option<&'static str> {
    let Some(report) = report else {
        return Some("no_report");
    };
    let [sent] = report.sent.as_slice() else {
        return Some("sent_rows");
    };
    if report.requested != level || sent.level != level {
        return Some("level");
    }
    if sent.route.to_string() != cell.route || sent.route.api != cell.key.api {
        return Some("route");
    }
    if sent.provider != cell.key.provider || sent.model != probe.model.model {
        return Some("model");
    }
    if sent.facts != cell.facts {
        return Some("facts");
    }
    if !sent
        .fields
        .iter()
        .map(String::as_str)
        .eq(probe.shape.fields().iter().copied())
    {
        return Some("fields");
    }
    if sent.requests != 1 {
        return Some("requests");
    }
    None
}

async fn probe(
    run: &MatrixRun,
    cell: &MatrixCell,
    probe: &ProbeModel,
    level: EffortLevel,
) -> ProbeOutcome {
    let gate = GatedClient::new(
        Arc::clone(&probe.model.client),
        cell.key,
        Arc::clone(&run.budget),
    );
    let mut agent = match build_agent(run, cell.key, &probe.model, Arc::clone(&gate), level).await {
        Ok(agent) => agent,
        Err(BuildAgentError::ReasoningEffortRefused(_)) => {
            return failed(FailureClass::StatedLevelRefused, false);
        }
        Err(_) => return failed(FailureClass::BuildFailed, false),
    };
    let outcome =
        tokio::time::timeout(run.turn_timeout, agent.run(PROBE_PROMPT.to_string().into())).await;
    let record = gate.record();
    let sent = record.sent > 0;
    if probe.shape == BodyShape::MessagesAdaptiveEffort && record.effort_without_adaptive_thinking {
        return failed(FailureClass::EffortWithoutAdaptiveThinking, sent);
    }
    match outcome {
        Err(_) => failed(FailureClass::TurnTimeout, sent),
        Ok(Ok(result)) => {
            if record.sent != 1 || record.completed != 1 || record.refused != 0 {
                return ProbeOutcome::Failed {
                    class: FailureClass::ReportMismatch,
                    status: None,
                    detail: Some("gate_count"),
                    sent,
                };
            }
            match report_mismatch(result.reasoning_effort.as_ref(), cell, probe, level) {
                None => ProbeOutcome::Passed,
                Some(detail) => ProbeOutcome::Failed {
                    class: FailureClass::ReportMismatch,
                    status: None,
                    detail: Some(detail),
                    sent,
                },
            }
        }
        Ok(Err(error)) => {
            if let Some(failure) = record.failure {
                ProbeOutcome::Failed {
                    class: failure.class,
                    status: failure.status,
                    detail: None,
                    sent,
                }
            } else if record.refused > 0 && record.completed > 0 {
                ProbeOutcome::Cut
            } else if record.refused > 0 {
                failed(FailureClass::BudgetSpent, sent)
            } else if matches!(error, AgentError::ReasoningEffortRefused(_)) {
                failed(FailureClass::StatedLevelRefused, sent)
            } else {
                failed(FailureClass::TurnFailed, sent)
            }
        }
    }
}

async fn run_cell(emitter: &mut Emitter<'_>, cell: &MatrixCell) {
    let run = emitter.run;
    let route = cell.route.clone();

    // The refusals first: they send nothing.
    if let Some(unknown) = &cell.unknown {
        let level = EffortLevel::Low;
        let (line, seen) = match refused_without_request(run, cell.key, unknown, level).await {
            Ok(refusal)
                if refusal.reason == ReasoningEffortRefusalReason::SupportedLevelsUnknown
                    && refusal.supported.is_none()
                    && refusal.facts.is_none() =>
            {
                (
                    MatrixLine::Local {
                        route: route.clone(),
                        model: unknown.model.clone(),
                        refused: refusal.reason,
                    },
                    observation(
                        run,
                        cell,
                        &unknown.model,
                        level,
                        ReceiptOutcome::Refused {
                            reason: reason_word(refusal.reason),
                        },
                        None,
                        false,
                    ),
                )
            }
            other => {
                let class = other.err().unwrap_or(FailureClass::RefusalMismatch);
                (
                    MatrixLine::Fail {
                        route: route.clone(),
                        model: unknown.model.clone(),
                        shape: None,
                        level: Some(level),
                        class,
                        status: None,
                        detail: Some("unknown_levels"),
                    },
                    observation(
                        run,
                        cell,
                        &unknown.model,
                        level,
                        failed_refusal_outcome(class, level),
                        Some((class, None)),
                        false,
                    ),
                )
            }
        };
        emitter.emit(&line);
        emitter.observe(&seen);
    }

    let Some(probe_model) = &cell.probe else {
        emitter.emit(&MatrixLine::Skip {
            route,
            reason: SkipReason::NoModelWithLevels,
        });
        return;
    };
    let model = probe_model.model.model.clone();
    let plan = plan_cell(&probe_model.supported, run.budget.per_cell);
    if plan.levels.is_empty() {
        emitter.emit(&MatrixLine::Skip {
            route,
            reason: SkipReason::ModelTakesNoLevel,
        });
        return;
    }

    let mut cell_failed = false;
    if let Some(level) = plan.refused {
        match refused_without_request(run, cell.key, &probe_model.model, level).await {
            Ok(refusal)
                if refusal.reason == ReasoningEffortRefusalReason::UnsupportedLevel
                    && refusal.supported.as_deref() == Some(probe_model.supported.as_slice())
                    && refusal.facts == Some(cell.facts) =>
            {
                emitter.observe(&observation(
                    run,
                    cell,
                    &model,
                    level,
                    ReceiptOutcome::Refused {
                        reason: reason_word(refusal.reason),
                    },
                    None,
                    false,
                ));
            }
            other => {
                cell_failed = true;
                let class = other.err().unwrap_or(FailureClass::RefusalMismatch);
                emitter.emit(&MatrixLine::Fail {
                    route: route.clone(),
                    model: model.clone(),
                    shape: Some(probe_model.shape),
                    level: Some(level),
                    class,
                    status: None,
                    detail: Some("level_outside_the_set"),
                });
                emitter.observe(&observation(
                    run,
                    cell,
                    &model,
                    level,
                    failed_refusal_outcome(class, level),
                    Some((class, None)),
                    false,
                ));
            }
        }
    }

    let mut passed = Vec::new();
    for level in plan.levels.iter().copied() {
        if run.budget.stop().is_some() {
            break;
        }
        let sent_level = ReceiptOutcome::Sent {
            level: level.as_wire_str(),
        };
        match probe(run, cell, probe_model, level).await {
            ProbeOutcome::Passed => {
                passed.push(level);
                emitter.observe(&observation(
                    run, cell, &model, level, sent_level, None, false,
                ));
            }
            ProbeOutcome::Cut => {
                cell_failed = true;
                emitter.emit(&MatrixLine::Cut {
                    route: route.clone(),
                    model: model.clone(),
                    shape: probe_model.shape,
                    level,
                });
                emitter.observe(&observation(
                    run, cell, &model, level, sent_level, None, true,
                ));
            }
            ProbeOutcome::Failed {
                class,
                status,
                detail,
                sent,
            } => {
                cell_failed = true;
                emitter.emit(&MatrixLine::Fail {
                    route: route.clone(),
                    model: model.clone(),
                    shape: Some(probe_model.shape),
                    level: Some(level),
                    class,
                    status,
                    detail,
                });
                let outcome = if sent {
                    sent_level
                } else {
                    ReceiptOutcome::Refused {
                        reason: class.as_str(),
                    }
                };
                emitter.observe(&observation(
                    run,
                    cell,
                    &model,
                    level,
                    outcome,
                    Some((class, status)),
                    false,
                ));
            }
        }
    }
    if !cell_failed && passed.len() == plan.levels.len() {
        emitter.emit(&MatrixLine::Pass {
            route: route.clone(),
            model,
            shape: probe_model.shape,
            levels: passed.clone(),
            refused: plan.refused,
            requests: run.budget.spent(cell.key),
        });
    }
    emitter.emit(&MatrixLine::Note {
        route,
        stated_models: cell.stated_models,
        probed_models: 1,
        stated_levels: u32::try_from(probe_model.supported.len()).unwrap_or(u32::MAX),
        probed_levels: u32::try_from(passed.len()).unwrap_or(u32::MAX),
    });
}

/// Run `cells` in order and hand every line to `sink` as it is made. A cell
/// does nothing once the run stopped or its deadline passed. The summary
/// holds the same lines and the counts.
pub async fn run_matrix(
    run: &MatrixRun,
    cells: &[MatrixCell],
    sink: &(dyn Fn(&str) + Send + Sync),
) -> MatrixSummary {
    let ceiling = run.budget.ceiling(cells.len());
    let mut emitter = Emitter {
        run,
        sink,
        summary: MatrixSummary {
            ceiling,
            ..MatrixSummary::default()
        },
    };
    let mut stop_reported = false;
    for cell in cells {
        let reason = if run.budget.stop().is_some() {
            Some(SkipReason::Stopped)
        } else if Instant::now() >= run.deadline {
            Some(SkipReason::RunDeadline)
        } else {
            None
        };
        if let Some(reason) = reason {
            emitter.emit(&MatrixLine::Skip {
                route: cell.route.clone(),
                reason,
            });
            continue;
        }
        run_cell(&mut emitter, cell).await;
        if !stop_reported && let Some(stop) = run.budget.stop() {
            stop_reported = true;
            emitter.emit(&MatrixLine::Stop {
                class: stop.class,
                status: stop.status,
                requests: run.budget.total_spent(),
                ceiling,
            });
        }
    }
    emitter.summary.requests = run.budget.total_spent();
    emitter.summary.stopped = run.budget.stop();
    let summary_line = MatrixLine::Summary {
        pass: emitter.summary.pass,
        fail: emitter.summary.fail,
        cut: emitter.summary.cut,
        skip: emitter.summary.skip,
        local: emitter.summary.local,
        requests: emitter.summary.requests,
        ceiling,
    };
    emitter.emit(&summary_line);
    emitter.summary
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::VecDeque;

    use meerkat_copilot::{CopilotProviderRoute, GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID};
    use meerkat_core::lifecycle::run_primitive::{AnthropicEffort, ReasoningEffort};
    use meerkat_core::provider_matrix::{
        AnthropicBackendKind, GoogleBackendKind, OpenAiBackendKind,
    };
    use meerkat_core::{
        AccountLevelStatement, BindingId, BindingOrigin, RealmId, ReasoningBackend, StopReason,
    };

    use super::*;

    // -----------------------------------------------------------------------
    // The scripted transport
    // -----------------------------------------------------------------------

    /// What the scripted transport answers one request with.
    enum Reply {
        /// A streamed answer that ends the turn.
        Text,
        /// A streamed call of the probe tool: the turn then asks again.
        ToolCall,
        /// The provider's failure.
        Fails(LlmError),
    }

    /// What a request that reached the scripted transport carried.
    #[derive(Debug, Clone)]
    struct Seen {
        tag: Option<ProviderTag>,
        tools: Vec<String>,
    }

    /// A route's client with no provider behind it: it reports the route it
    /// is given, records every request that reaches it and answers each
    /// from its script.
    struct Scripted {
        provider: Provider,
        route: ReasoningRouteObservation,
        replies: Mutex<VecDeque<Reply>>,
        requests: Mutex<Vec<Seen>>,
        health_checks: Mutex<u32>,
    }

    impl Scripted {
        fn requests(&self) -> Vec<Seen> {
            lock(&self.requests).clone()
        }
    }

    #[async_trait]
    impl LlmClient for Scripted {
        fn reasoning_route(&self) -> Option<ReasoningRouteObservation> {
            Some(self.route.clone())
        }

        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            lock(&self.requests).push(Seen {
                tag: request.provider_params.clone(),
                tools: request
                    .tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect(),
            });
            let usage = LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    self.provider,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            let done = |stop_reason| LlmEvent::Done {
                outcome: LlmDoneOutcome::success(stop_reason),
            };
            let events = match lock(&self.replies).pop_front() {
                Some(Reply::Text) => vec![
                    Ok(LlmEvent::TextDelta {
                        delta: "ready".to_string(),
                        meta: None,
                    }),
                    Ok(usage),
                    Ok(done(StopReason::EndTurn)),
                ],
                Some(Reply::ToolCall) => vec![
                    Ok(LlmEvent::ToolCallComplete {
                        id: "call_1".to_string(),
                        name: PROBE_TOOL.to_string(),
                        args: json!({"nonce": "n"}),
                        meta: None,
                    }),
                    Ok(usage),
                    Ok(done(StopReason::ToolUse)),
                ],
                Some(Reply::Fails(error)) => vec![Err(error)],
                None => vec![Err(LlmError::Unknown {
                    message: "the script holds no further reply".to_string(),
                })],
            };
            Box::pin(futures::stream::iter(events))
        }

        fn provider(&self) -> Provider {
            self.provider
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            *lock(&self.health_checks) += 1;
            Ok(())
        }
    }

    fn copilot_backend(provider: Provider) -> ReasoningBackend {
        match provider {
            Provider::OpenAI => ReasoningBackend::OpenAi(OpenAiBackendKind::Copilot),
            Provider::Anthropic => ReasoningBackend::Anthropic(AnthropicBackendKind::Copilot),
            Provider::Gemini => ReasoningBackend::Google(GoogleBackendKind::Copilot),
            other => panic!("no Copilot backend for {other:?}"),
        }
    }

    /// A scripted client on the Copilot route of `provider` and `api` whose
    /// account entry states `account`.
    fn scripted(
        provider: Provider,
        api: ModelApi,
        account: AccountLevelStatement,
        replies: Vec<Reply>,
    ) -> Arc<Scripted> {
        Arc::new(Scripted {
            provider,
            route: ReasoningRouteObservation {
                backend: copilot_backend(provider),
                api,
                account,
                operator_levels: None,
            },
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
            health_checks: Mutex::new(0),
        })
    }

    fn stated(levels: &[EffortLevel]) -> AccountLevelStatement {
        AccountLevelStatement::Stated(levels.to_vec())
    }

    /// The `nth` catalog model of `provider`, its default first.
    fn catalog_model(provider: Provider, nth: usize) -> String {
        let mut models: Vec<&str> = meerkat_models::default_model(provider)
            .into_iter()
            .chain(meerkat_models::allowed_models(provider))
            .collect();
        let mut seen = Vec::new();
        models.retain(|model| {
            let new = !seen.contains(model);
            seen.push(*model);
            new
        });
        models
            .get(nth)
            .unwrap_or_else(|| panic!("the catalog has {} {provider:?} models", nth + 1))
            .to_string()
    }

    /// The three Copilot routes of one account, as a realm declares them
    /// for an operator token. Nothing reads the variable the auth profiles
    /// name: a session built on a finished client resolves no credential.
    fn copilot_routes() -> meerkat_core::RealmConfigSection {
        let mut backend = serde_json::Map::new();
        let mut auth = serde_json::Map::new();
        let mut binding = serde_json::Map::new();
        for route in CopilotProviderRoute::ALL {
            let id = route.binding_id();
            let provider = route.provider().as_str();
            backend.insert(
                id.to_string(),
                json!({"provider": provider, "backend_kind": "copilot"}),
            );
            auth.insert(
                id.to_string(),
                json!({
                    "provider": provider,
                    "auth_method": "github_copilot_token",
                    "source": {"kind": "env", "env": "MATRIX_ROW_TOKEN_THAT_IS_NEVER_SET"},
                }),
            );
            binding.insert(
                id.to_string(),
                json!({
                    "backend_profile": id,
                    "auth_profile": id,
                    "credential_account": GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID,
                }),
            );
        }
        serde_json::from_value(json!({"backend": backend, "auth": auth, "binding": binding}))
            .expect("the Copilot routes of a realm")
    }

    /// The `nth` catalog model of `provider` on the scripted `client`, with
    /// the Copilot binding of the provider's family.
    fn candidate(provider: Provider, nth: usize, client: &Arc<Scripted>) -> (Provider, CellModel) {
        let mut config = Config::default();
        config.realm.insert("global".to_string(), copilot_routes());
        let route = CopilotProviderRoute::ALL
            .iter()
            .find(|route| route.provider() == provider)
            .unwrap_or_else(|| panic!("no Copilot route for {provider:?}"));
        (
            provider,
            CellModel {
                model: catalog_model(provider, nth),
                client: Arc::clone(client) as Arc<dyn LlmClient>,
                auth_binding: Some(AuthBindingRef {
                    realm: RealmId::global(),
                    binding: BindingId::parse(route.binding_id()).expect("a binding id"),
                    profile: None,
                    origin: BindingOrigin::Configured,
                }),
                config,
            },
        )
    }

    fn run_in(temp: &tempfile::TempDir, max_retries: u32) -> MatrixRun {
        MatrixRun {
            lane: Lane::Copilot,
            state_root: temp.path().to_path_buf(),
            budget: RequestBudget::new(REQUESTS_PER_CELL),
            redactor: Redactor::default(),
            turn_timeout: Duration::from_secs(60),
            deadline: Instant::now() + Duration::from_secs(600),
            max_tokens: 256,
            max_retries,
        }
    }

    /// Run the cells; the lines handed to the sink are the summary's.
    async fn drive(run: &MatrixRun, cells: &[MatrixCell]) -> MatrixSummary {
        let printed = Mutex::new(Vec::new());
        let summary = run_matrix(run, cells, &|line: &str| {
            lock(&printed).push(line.to_string());
        })
        .await;
        assert_eq!(*lock(&printed), summary.lines);
        summary
    }

    /// The lines of a run that people read: every line but the receipt's.
    fn human(summary: &MatrixSummary) -> Vec<&str> {
        summary
            .lines
            .iter()
            .map(String::as_str)
            .filter(|line| !line.starts_with(RECEIPT_OBSERVATION_PREFIX))
            .collect()
    }

    /// The receipt observations of a run, in the order it printed them.
    fn observations(summary: &MatrixSummary) -> Vec<serde_json::Value> {
        summary
            .lines
            .iter()
            .filter_map(|line| line.strip_prefix(RECEIPT_OBSERVATION_PREFIX))
            .map(|object| serde_json::from_str(object).expect("one JSON object"))
            .collect()
    }

    /// An observation of the Copilot lane as the receipt reads it.
    fn copilot_observation(
        provider: Provider,
        api: &str,
        model: &str,
        requested: &str,
        outcome: serde_json::Value,
        result: &str,
    ) -> serde_json::Value {
        json!({
            "row": "live.copilot_matrix",
            "provider_family": provider.as_str(),
            "backend": "copilot",
            "api": api,
            "model": model,
            "requested_level": requested,
            "outcome": outcome,
            "result": result,
        })
    }

    fn sent_outcome(level: &str) -> serde_json::Value {
        json!({"kind": "sent", "level": level})
    }

    fn refused_outcome(reason: &str) -> serde_json::Value {
        json!({"kind": "refused", "reason": reason})
    }

    const OPENAI_RESPONSES: CellKey = CellKey {
        provider: Provider::OpenAI,
        api: ModelApi::Responses,
    };

    // -----------------------------------------------------------------------
    // Redaction
    // -----------------------------------------------------------------------

    #[test]
    fn redaction_replaces_every_private_value() {
        let redactor = Redactor::for_account(
            Some("Octo-Probe"),
            Some("business"),
            Some("https://api.business.example-copilot.test/"),
        );
        // The login in another case, the plan, the API base and its host.
        assert_eq!(
            redactor.redact(
                "login=octo-probe plan=Business base=https://api.business.example-copilot.test/ \
                 host=API.business.example-copilot.test model=gpt-5.5"
            ),
            "login=<redacted> plan=<redacted> base=<redacted> host=<redacted> model=gpt-5.5"
        );
        // Where two values start at the same place the longer one goes.
        let nested = Redactor::new(["octo".to_string(), "octo-org".to_string()]);
        assert_eq!(
            nested.redact("org=octo-org user=octo"),
            "org=<redacted> user=<redacted>"
        );
        // A value that names nothing replaces nothing, and text outside the
        // values is kept byte for byte.
        let blank = Redactor::new([String::new(), "   ".to_string()]);
        assert_eq!(
            blank.redact("levels=[low,high] caf\u{e9}"),
            "levels=[low,high] caf\u{e9}"
        );
        assert_eq!(Redactor::default().redact("unchanged"), "unchanged");
        // The redactor's own debug form holds no value.
        assert!(!format!("{redactor:?}").contains("octo"));
    }

    #[test]
    fn redaction_scrubs_token_shapes() {
        let token = format!("{}_{}", "ghp", "a1B2c3D4e5");
        let fine_grained = format!("{}_{}", "github_pat", "11AAAA_bbbb");
        let line = format!("bearer={token}; other={fine_grained} status=401");
        assert_eq!(
            Redactor::default().redact(&line),
            "bearer=<redacted>; other=<redacted> status=401"
        );
        assert_eq!(scrub_token_shapes("no token here"), "no token here");
    }

    // -----------------------------------------------------------------------
    // Lines
    // -----------------------------------------------------------------------

    #[test]
    fn pass_line_has_the_documented_form() {
        let line = MatrixLine::Pass {
            route: "copilot/responses".to_string(),
            model: "model-a".to_string(),
            shape: BodyShape::FullPayload,
            levels: vec![EffortLevel::Low, EffortLevel::Xhigh],
            refused: Some(EffortLevel::Max),
            requests: 2,
        };
        assert_eq!(
            line.render(Lane::Copilot),
            "copilot-live reasoning PASS route=copilot/responses model=model-a \
             shape=full-payload levels=[low,xhigh] refused=max requests=2"
        );
        let direct = MatrixLine::Pass {
            route: "anthropic_api/messages".to_string(),
            model: "model-b".to_string(),
            shape: BodyShape::MessagesEffort,
            levels: vec![EffortLevel::Low],
            refused: None,
            requests: 1,
        };
        assert_eq!(
            direct.render(Lane::Direct),
            "direct-live reasoning PASS route=anthropic_api/messages model=model-b \
             shape=messages-effort levels=[low] refused=- requests=1"
        );
    }

    #[test]
    fn every_other_line_has_its_form() {
        let route = || "copilot/messages".to_string();
        let model = || "model-a".to_string();
        let rendered = [
            MatrixLine::Local {
                route: route(),
                model: model(),
                refused: ReasoningEffortRefusalReason::SupportedLevelsUnknown,
            },
            MatrixLine::Fail {
                route: route(),
                model: model(),
                shape: Some(BodyShape::MessagesAdaptiveEffort),
                level: Some(EffortLevel::High),
                class: FailureClass::ServerError,
                status: Some(502),
                detail: None,
            },
            MatrixLine::Fail {
                route: route(),
                model: model(),
                shape: None,
                level: None,
                class: FailureClass::ReportMismatch,
                status: None,
                detail: Some("fields"),
            },
            MatrixLine::Cut {
                route: route(),
                model: model(),
                shape: BodyShape::MessagesAdaptiveEffort,
                level: EffortLevel::Low,
            },
            MatrixLine::Skip {
                route: route(),
                reason: SkipReason::RunDeadline,
            },
            MatrixLine::Note {
                route: route(),
                stated_models: 10,
                probed_models: 1,
                stated_levels: 5,
                probed_levels: 1,
            },
            MatrixLine::Stop {
                class: FailureClass::RateLimited,
                status: Some(429),
                requests: 2,
                ceiling: 4,
            },
            MatrixLine::Summary {
                pass: 1,
                fail: 1,
                cut: 0,
                skip: 2,
                local: 1,
                requests: 2,
                ceiling: 4,
            },
        ]
        .map(|line| line.render(Lane::Copilot));
        assert_eq!(
            rendered,
            [
                "copilot-live reasoning LOCAL route=copilot/messages model=model-a \
                 refused=supported_levels_unknown requests=0",
                "copilot-live reasoning FAIL route=copilot/messages model=model-a \
                 shape=messages-adaptive-effort level=high class=server_error status=502 detail=-",
                "copilot-live reasoning FAIL route=copilot/messages model=model-a shape=- level=- \
                 class=report_mismatch status=- detail=fields",
                "copilot-live reasoning CUT route=copilot/messages model=model-a \
                 shape=messages-adaptive-effort level=low \
                 reason=second_request_refused_by_budget first_request=answered",
                "copilot-live reasoning SKIP route=copilot/messages reason=run_deadline",
                "copilot-live reasoning NOTE route=copilot/messages models_with_stated_levels=10 \
                 models_probed=1 levels_of_probed_model=5 levels_probed=1",
                "copilot-live reasoning STOP class=rate_limited status=429 requests=2/4 \
                 no further request is sent",
                "copilot-live reasoning SUMMARY pass=1 fail=1 cut=0 skip=2 local=1 requests=2/4 \
                 account=<redacted>",
            ]
        );
    }

    /// A cell for the receipt is the prefix and one JSON object of typed
    /// fields, in a fixed order; the status and the error code are members
    /// of a failed cell only.
    #[test]
    fn a_receipt_observation_is_one_line_of_typed_fields() {
        let failed = ReceiptObservation {
            row: Lane::Copilot.receipt_row(),
            provider_family: "openai",
            backend: "copilot".to_string(),
            api: "responses",
            model: "model-a".to_string(),
            requested_level: "low",
            outcome: ReceiptOutcome::Sent { level: "low" },
            result: "fail",
            http_status: Some(429),
            error_code: Some("rate_limited"),
        };
        assert_eq!(
            failed.line(),
            concat!(
                "reasoning-effort-receipt-observation ",
                r#"{"row":"live.copilot_matrix","provider_family":"openai","backend":"copilot","#,
                r#""api":"responses","model":"model-a","requested_level":"low","#,
                r#""outcome":{"kind":"sent","level":"low"},"result":"fail","#,
                r#""http_status":429,"error_code":"rate_limited"}"#,
            )
        );
        let refused = ReceiptObservation {
            row: Lane::Direct.receipt_row(),
            backend: "openai_api".to_string(),
            requested_level: "max",
            outcome: ReceiptOutcome::Refused {
                reason: "unsupported_level",
            },
            result: "pass",
            http_status: None,
            error_code: None,
            ..failed
        };
        assert_eq!(
            refused.line(),
            concat!(
                "reasoning-effort-receipt-observation ",
                r#"{"row":"live.direct_providers","provider_family":"openai","backend":"openai_api","#,
                r#""api":"responses","model":"model-a","requested_level":"max","#,
                r#""outcome":{"kind":"refused","reason":"unsupported_level"},"result":"pass"}"#,
            )
        );
    }

    #[test]
    fn body_shapes_name_the_route_and_its_fields() {
        let shape = |copilot, api, row| BodyShape::of(copilot, api, row).map(BodyShape::as_str);
        assert_eq!(shape(true, ModelApi::Responses, true), Some("full-payload"));
        assert_eq!(shape(true, ModelApi::Responses, false), Some("effort-only"));
        assert_eq!(
            shape(true, ModelApi::Messages, true),
            Some("messages-adaptive-effort")
        );
        assert_eq!(
            shape(true, ModelApi::ChatCompletions, false),
            Some("chat-double")
        );
        assert_eq!(
            shape(false, ModelApi::Responses, true),
            Some("responses-effort")
        );
        assert_eq!(
            shape(false, ModelApi::Messages, true),
            Some("messages-effort")
        );
        assert_eq!(
            shape(false, ModelApi::GenerateContent, true),
            Some("generate-content-thinking-level")
        );
        assert_eq!(shape(true, ModelApi::Realtime, true), None);
        assert_eq!(
            BodyShape::MessagesAdaptiveEffort.fields(),
            ["thinking.type", "output_config.effort"]
        );
        assert_eq!(
            BodyShape::ChatDouble.fields(),
            ["reasoning_effort", "reasoning.effort"]
        );
        assert_eq!(BodyShape::FullPayload.fields(), ["reasoning.effort"]);
    }

    // -----------------------------------------------------------------------
    // Classes
    // -----------------------------------------------------------------------

    /// The client's own classification of an HTTP answer, then the class and
    /// status a line carries and whether the run stops. The provider's text
    /// is in none of it.
    #[test]
    fn a_failure_is_its_class_and_status_and_never_its_text() {
        let text = r#"{"error":{"message":"private provider text"}}"#;
        let table: [(u16, &str, Option<u16>, bool); 8] = [
            (401, "authentication_failed", Some(401), true),
            (403, "forbidden", Some(403), true),
            (429, "rate_limited", Some(429), true),
            (402, "quota_exhausted", None, true),
            (400, "invalid_request", None, false),
            (404, "model_not_found", Some(404), false),
            (500, "server_error", Some(500), false),
            (503, "server_overloaded", Some(503), false),
        ];
        for (http, class, status, stops) in table {
            let failure = classify(&LlmError::from_http_status(http, text.to_string(), None));
            assert_eq!(
                (
                    failure.class.as_str(),
                    failure.status,
                    failure.stops_the_run()
                ),
                (class, status, stops),
                "HTTP {http}"
            );
            let line = MatrixLine::Fail {
                route: "copilot/responses".to_string(),
                model: "model-a".to_string(),
                shape: Some(BodyShape::FullPayload),
                level: Some(EffortLevel::Low),
                class: failure.class,
                status: failure.status,
                detail: None,
            }
            .render(Lane::Copilot);
            assert!(!line.contains("private"), "{line}");
        }
        let stop = classify(&LlmError::PolicyStop {
            code: "c".to_string(),
            message: text.to_string(),
        });
        assert!(stop.stops_the_run());
        assert_eq!(
            classify(&LlmError::ConnectionReset),
            ProviderFailure {
                class: FailureClass::Network,
                status: None
            }
        );
    }

    /// An account that does not resolve is a fixed word, whatever its
    /// failure says, and its line says that nothing was sent.
    #[test]
    fn an_account_failure_is_a_fixed_word() {
        let private = "private text naming octo-probe".to_string();
        let auth = ProviderAuthError::Auth;
        let table = [
            (auth(AuthError::UserReauthRequired), "token_rejected"),
            (
                auth(AuthError::RefreshFailed(private.clone())),
                "account_refused",
            ),
            (
                auth(AuthError::SourceUnavailable {
                    reason: private.clone(),
                }),
                "rate_limited_or_unavailable",
            ),
            (auth(AuthError::MissingSecret), "no_token"),
            (
                ProviderAuthError::SourceResolutionFailed(private.clone()),
                "not_resolved",
            ),
        ];
        for (error, class) in table {
            assert_eq!(account_failure_class(&error), class);
        }
        assert_eq!(
            MatrixLine::AccountStop {
                class: "token_rejected"
            }
            .render(Lane::Copilot),
            "copilot-live reasoning STOP stage=account class=token_rejected requests=0 \
             no inference request is sent"
        );
    }

    // -----------------------------------------------------------------------
    // The plan and the budget
    // -----------------------------------------------------------------------

    #[test]
    fn a_cell_plans_one_level_per_request_and_one_level_outside() {
        use EffortLevel::{High, Low, Max, Medium, Minimal, None as Off, Xhigh};
        let plan = |supported: &[EffortLevel], requests| {
            let plan = plan_cell(supported, requests);
            (plan.levels, plan.refused)
        };
        // One request: the lowest level at or above `low`.
        assert_eq!(plan(&[Low, Medium, High], 1), (vec![Low], Some(Xhigh)));
        assert_eq!(
            plan(&[Off, Minimal, Medium, High], 1),
            (vec![Medium], Some(Low))
        );
        // Two requests: the lowest and the highest.
        assert_eq!(
            plan(&[Low, Medium, High], 2),
            (vec![Low, High], Some(Xhigh))
        );
        // More requests than levels to tell apart send no level twice.
        assert_eq!(plan(&[High], 3), (vec![High], Some(Low)));
        // Only levels below `low`: the lowest of them.
        assert_eq!(plan(&[Off, Minimal], 1), (vec![Off], Some(Low)));
        // An unordered, repeated statement is read as its set.
        assert_eq!(plan(&[High, Low, High], 2), (vec![Low, High], Some(Medium)));
        // The whole ladder leaves no level outside; no level leaves nothing
        // to send.
        assert_eq!(plan(&LADDER, 1), (vec![Low], None));
        assert_eq!(plan(&[], 1), (vec![], Some(Low)));
        assert_eq!(plan(&[Low, Max], 0), (vec![], Some(Medium)));
    }

    #[test]
    fn the_budget_admits_one_request_per_cell_and_none_after_a_stop() {
        let messages = CellKey {
            provider: Provider::Anthropic,
            api: ModelApi::Messages,
        };
        let budget = RequestBudget::new(REQUESTS_PER_CELL);
        assert_eq!(REQUESTS_PER_CELL, 1);
        assert_eq!(budget.ceiling(4), 4);
        assert_eq!(budget.admit(OPENAI_RESPONSES), Ok(()));
        assert_eq!(
            budget.admit(OPENAI_RESPONSES),
            Err(GateRefusal::CellBudgetSpent)
        );
        assert_eq!(
            budget.admit(OPENAI_RESPONSES),
            Err(GateRefusal::CellBudgetSpent)
        );
        // Another cell has its own request; a refused one is not counted.
        assert_eq!(budget.admit(messages), Ok(()));
        assert_eq!(
            (budget.spent(OPENAI_RESPONSES), budget.spent(messages)),
            (1, 1)
        );
        assert_eq!(budget.total_spent(), 2);

        // A failure that does not stop the run changes nothing; one that
        // does closes every cell, also one that sent nothing yet.
        let budget = RequestBudget::new(2);
        budget.record_failure(ProviderFailure {
            class: FailureClass::ServerError,
            status: Some(500),
        });
        assert_eq!(budget.stop(), None);
        assert_eq!(budget.admit(OPENAI_RESPONSES), Ok(()));
        let rate_limit = ProviderFailure {
            class: FailureClass::RateLimited,
            status: Some(429),
        };
        budget.record_failure(rate_limit);
        budget.record_failure(ProviderFailure {
            class: FailureClass::AuthenticationFailed,
            status: Some(401),
        });
        assert_eq!(budget.stop(), Some(rate_limit), "the first stop is kept");
        assert_eq!(budget.admit(OPENAI_RESPONSES), Err(GateRefusal::Stopped));
        assert_eq!(budget.admit(messages), Err(GateRefusal::Stopped));
        assert_eq!(budget.total_spent(), 1);
    }

    // -----------------------------------------------------------------------
    // The gate
    // -----------------------------------------------------------------------

    async fn drain(stream: LlmStream<'_>) -> Vec<Result<LlmEvent, LlmError>> {
        stream.collect().await
    }

    #[tokio::test]
    async fn the_gate_refuses_the_request_over_the_budget_before_the_client() {
        let client = scripted(
            Provider::OpenAI,
            ModelApi::Responses,
            stated(&[EffortLevel::Low]),
            vec![Reply::Text, Reply::Text],
        );
        let budget = RequestBudget::new(REQUESTS_PER_CELL);
        let gate = GatedClient::new(client.clone(), OPENAI_RESPONSES, Arc::clone(&budget));
        let request = LlmRequest::new("model-a", Vec::new());

        let first = drain(gate.stream(&request)).await;
        assert!(first.iter().all(Result::is_ok), "{first:?}");
        let second = drain(gate.stream(&request)).await;
        assert!(
            matches!(second.as_slice(), [Err(LlmError::InvalidRequest { message })] if message == GATE_REFUSAL),
            "{second:?}"
        );
        // A second gate of the same cell shares the cell's budget.
        let other = GatedClient::new(client.clone(), OPENAI_RESPONSES, Arc::clone(&budget));
        assert!(matches!(
            drain(other.stream(&request)).await.as_slice(),
            [Err(LlmError::InvalidRequest { .. })]
        ));

        assert_eq!(client.requests().len(), 1, "one request reached the client");
        assert_eq!(
            gate.record(),
            GateRecord {
                sent: 1,
                refused: 1,
                completed: 1,
                failure: None,
                effort_without_adaptive_thinking: false,
            }
        );
        assert_eq!(other.record().refused, 1);
        assert_eq!(budget.total_spent(), 1);
        // The gate reports the client's route and never forwards a health
        // check, which would be a request nothing counts.
        assert_eq!(gate.reasoning_route(), client.reasoning_route());
        gate.health_check().await.expect("answered by the gate");
        assert_eq!(*lock(&client.health_checks), 0);
    }

    #[test]
    fn an_output_effort_without_adaptive_thinking_is_seen() {
        use meerkat_core::lifecycle::run_primitive::{AnthropicProviderTag, OpenAiProviderTag};
        let request = |tag: Option<ProviderTag>| {
            let mut request = LlmRequest::new("model-a", Vec::new());
            request.provider_params = tag;
            request
        };
        let anthropic = |effort, thinking| {
            request(Some(ProviderTag::Anthropic(AnthropicProviderTag {
                effort,
                thinking,
                ..Default::default()
            })))
        };
        let low = Some(AnthropicEffort::Low);
        assert!(effort_without_adaptive_thinking(&anthropic(low, None)));
        assert!(effort_without_adaptive_thinking(&anthropic(
            low,
            Some(AnthropicThinkingConfig::Enabled {
                budget_tokens: 1024
            })
        )));
        assert!(!effort_without_adaptive_thinking(&anthropic(
            low,
            Some(AnthropicThinkingConfig::Adaptive)
        )));
        assert!(!effort_without_adaptive_thinking(&anthropic(None, None)));
        assert!(!effort_without_adaptive_thinking(&request(None)));
        assert!(!effort_without_adaptive_thinking(&request(Some(
            ProviderTag::OpenAi(OpenAiProviderTag {
                reasoning_effort: Some(ReasoningEffort::Low),
                ..Default::default()
            })
        ))));
    }

    // -----------------------------------------------------------------------
    // The driver, on a real agent loop over the scripted transport
    // -----------------------------------------------------------------------

    #[test]
    fn cells_take_the_first_stated_and_the_first_unknown_model_of_a_route() {
        let levels = [EffortLevel::Low, EffortLevel::High];
        let responses = |account| scripted(Provider::OpenAI, ModelApi::Responses, account, vec![]);
        let silent = responses(AccountLevelStatement::NotStated);
        let first = responses(stated(&levels));
        let second = responses(stated(&[EffortLevel::Medium]));
        let empty = responses(stated(&[]));
        let unloaded = responses(AccountLevelStatement::ListUnavailable);
        let messages = scripted(
            Provider::Anthropic,
            ModelApi::Messages,
            stated(&levels),
            vec![],
        );
        let cells = assemble_cells(vec![
            candidate(Provider::OpenAI, 0, &silent),
            candidate(Provider::OpenAI, 1, &first),
            candidate(Provider::OpenAI, 2, &second),
            candidate(Provider::OpenAI, 3, &empty),
            candidate(Provider::OpenAI, 4, &unloaded),
            candidate(Provider::Anthropic, 0, &messages),
        ]);
        let [responses_cell, messages_cell] = cells.as_slice() else {
            panic!("two routes, two cells: {}", cells.len());
        };
        assert_eq!(responses_cell.key, OPENAI_RESPONSES);
        assert_eq!(responses_cell.route, "copilot/responses");
        assert_eq!(responses_cell.facts, ReasoningFactsSource::Account);
        assert_eq!(responses_cell.stated_models, 2);
        let probe = responses_cell.probe.as_ref().expect("a model with levels");
        assert_eq!(probe.model.model, catalog_model(Provider::OpenAI, 1));
        assert_eq!(probe.supported, levels);
        assert_eq!(
            responses_cell
                .unknown
                .as_ref()
                .map(|model| model.model.clone()),
            Some(catalog_model(Provider::OpenAI, 0))
        );
        assert_eq!(messages_cell.route, "copilot/messages");
        assert_eq!(
            messages_cell.probe.as_ref().map(|probe| probe.shape),
            Some(BodyShape::MessagesAdaptiveEffort)
        );
        assert!(messages_cell.unknown.is_none());
    }

    /// Every cell sends exactly one request, with the level in the route's
    /// own fields, and passes; the two refusals of a cell reach no client.
    #[tokio::test]
    async fn each_cell_sends_one_request_and_passes() {
        let levels = [EffortLevel::Low, EffortLevel::Medium, EffortLevel::High];
        let responses = scripted(
            Provider::OpenAI,
            ModelApi::Responses,
            stated(&levels),
            vec![Reply::Text],
        );
        let silent = scripted(
            Provider::OpenAI,
            ModelApi::Responses,
            AccountLevelStatement::NotStated,
            vec![Reply::Text],
        );
        let messages = scripted(
            Provider::Anthropic,
            ModelApi::Messages,
            stated(&[EffortLevel::Low, EffortLevel::High]),
            vec![Reply::Text],
        );
        let chat = scripted(
            Provider::Gemini,
            ModelApi::ChatCompletions,
            stated(&levels),
            vec![Reply::Text],
        );
        let cells = assemble_cells(vec![
            candidate(Provider::OpenAI, 0, &responses),
            candidate(Provider::OpenAI, 1, &silent),
            candidate(Provider::Anthropic, 0, &messages),
            candidate(Provider::Gemini, 0, &chat),
        ]);
        let temp = tempfile::TempDir::new().expect("a directory");
        let run = run_in(&temp, 0);

        let summary = drive(&run, &cells).await;

        let openai = catalog_model(Provider::OpenAI, 0);
        let shape = cells[0].probe.as_ref().expect("probed").shape.as_str();
        assert_eq!(
            human(&summary),
            [
                format!(
                    "copilot-live reasoning LOCAL route=copilot/responses model={} \
                     refused=supported_levels_unknown requests=0",
                    catalog_model(Provider::OpenAI, 1)
                ),
                format!(
                    "copilot-live reasoning PASS route=copilot/responses model={openai} \
                     shape={shape} levels=[low] refused=xhigh requests=1"
                ),
                "copilot-live reasoning NOTE route=copilot/responses models_with_stated_levels=1 \
                 models_probed=1 levels_of_probed_model=3 levels_probed=1"
                    .to_string(),
                format!(
                    "copilot-live reasoning PASS route=copilot/messages model={} \
                     shape=messages-adaptive-effort levels=[low] refused=medium requests=1",
                    catalog_model(Provider::Anthropic, 0)
                ),
                "copilot-live reasoning NOTE route=copilot/messages models_with_stated_levels=1 \
                 models_probed=1 levels_of_probed_model=2 levels_probed=1"
                    .to_string(),
                format!(
                    "copilot-live reasoning PASS route=copilot/chat_completions model={} \
                     shape=chat-double levels=[low] refused=xhigh requests=1",
                    catalog_model(Provider::Gemini, 0)
                ),
                "copilot-live reasoning NOTE route=copilot/chat_completions \
                 models_with_stated_levels=1 models_probed=1 levels_of_probed_model=3 \
                 levels_probed=1"
                    .to_string(),
                "copilot-live reasoning SUMMARY pass=3 fail=0 cut=0 skip=0 local=1 requests=3/3 \
                 account=<redacted>"
                    .to_string(),
            ]
        );
        assert_eq!(
            (summary.requests, summary.ceiling, summary.stopped),
            (3, 3, None)
        );

        // The receipt reads the same run as one observation per cell and
        // level: what was refused without a request and what was sent.
        let anthropic = catalog_model(Provider::Anthropic, 0);
        let gemini = catalog_model(Provider::Gemini, 0);
        let cell = copilot_observation;
        assert_eq!(
            observations(&summary),
            [
                cell(
                    Provider::OpenAI,
                    "responses",
                    &catalog_model(Provider::OpenAI, 1),
                    "low",
                    refused_outcome("supported_levels_unknown"),
                    "pass"
                ),
                cell(
                    Provider::OpenAI,
                    "responses",
                    &openai,
                    "xhigh",
                    refused_outcome("unsupported_level"),
                    "pass"
                ),
                cell(
                    Provider::OpenAI,
                    "responses",
                    &openai,
                    "low",
                    sent_outcome("low"),
                    "pass"
                ),
                cell(
                    Provider::Anthropic,
                    "messages",
                    &anthropic,
                    "medium",
                    refused_outcome("unsupported_level"),
                    "pass"
                ),
                cell(
                    Provider::Anthropic,
                    "messages",
                    &anthropic,
                    "low",
                    sent_outcome("low"),
                    "pass"
                ),
                cell(
                    Provider::Gemini,
                    "chat_completions",
                    &gemini,
                    "xhigh",
                    refused_outcome("unsupported_level"),
                    "pass"
                ),
                cell(
                    Provider::Gemini,
                    "chat_completions",
                    &gemini,
                    "low",
                    sent_outcome("low"),
                    "pass"
                ),
            ]
        );

        // One request per cell reached its client, none the silent model's.
        // Each carried the level in its route's typed fields and offered the
        // probe tool alone. The sessions are built on the routes' bindings,
        // so, as on every Copilot session, no provider-native tool rides
        // along.
        assert_eq!(silent.requests().len(), 0);
        let sent = responses.requests();
        let [
            Seen {
                tag: Some(ProviderTag::OpenAi(tag)),
                tools,
            },
        ] = sent.as_slice()
        else {
            panic!("one OpenAI request: {sent:?}");
        };
        assert_eq!(tag.reasoning_effort, Some(ReasoningEffort::Low));
        assert!(tag.web_search.is_none(), "{tag:?}");
        assert_eq!(tools, &[PROBE_TOOL]);
        let sent = messages.requests();
        let [
            Seen {
                tag: Some(ProviderTag::Anthropic(tag)),
                tools,
            },
        ] = sent.as_slice()
        else {
            panic!("one Anthropic request: {sent:?}");
        };
        assert_eq!(
            (tag.effort, tag.thinking.clone()),
            (
                Some(AnthropicEffort::Low),
                Some(AnthropicThinkingConfig::Adaptive)
            )
        );
        assert!(tag.web_search.is_none(), "{tag:?}");
        assert_eq!(tools, &[PROBE_TOOL]);
        let sent = chat.requests();
        let [Seen { tag, tools }] = sent.as_slice() else {
            panic!("one Gemini request: {sent:?}");
        };
        assert!(
            !matches!(tag, Some(ProviderTag::Gemini(tag)) if tag.google_search.is_some()),
            "{tag:?}"
        );
        assert_eq!(tools, &[PROBE_TOOL]);
    }

    /// Two cells whose first answers `error`, run with an agent loop that
    /// would retry a failed request twice: what the clients saw and the
    /// summary.
    async fn stopped_run(error: LlmError) -> (usize, usize, MatrixSummary) {
        let levels = [EffortLevel::Low, EffortLevel::High];
        let first = scripted(
            Provider::OpenAI,
            ModelApi::Responses,
            stated(&levels),
            vec![Reply::Fails(error), Reply::Text, Reply::Text],
        );
        let second = scripted(
            Provider::Anthropic,
            ModelApi::Messages,
            stated(&levels),
            vec![Reply::Text],
        );
        let cells = assemble_cells(vec![
            candidate(Provider::OpenAI, 0, &first),
            candidate(Provider::Anthropic, 0, &second),
        ]);
        let temp = tempfile::TempDir::new().expect("a directory");
        let run = run_in(&temp, 2);
        let summary = drive(&run, &cells).await;
        (first.requests().len(), second.requests().len(), summary)
    }

    /// The first HTTP 401 a client reports ends the run: the request is not
    /// sent again and no other cell sends one. The line carries the class
    /// and the status and nothing of the provider's text.
    ///
    /// The gate sits at the client seam. Below it, a GitHub Copilot client
    /// answers one HTTP 401 itself before the error surfaces here: its
    /// authorizer asks for a fresh authorization
    /// (`CopilotAuthorizer::observe_response_with_receipt`), which looks the
    /// account up again (one identity lookup, one model list) and the client
    /// sends the same request once more: three requests more for that
    /// request. When the second answer is a 401 too, the failure reaches the
    /// gate and the run stops, so a live run pays those three once. When the
    /// second answer succeeds, nothing reaches the gate and a later cell can
    /// pay them again. That resend is the product's and is not in this row.
    #[tokio::test]
    async fn the_first_401_stops_the_run() {
        let error = LlmError::from_http_status(
            401,
            r#"{"error":{"message":"private provider text for octo-probe"}}"#.to_string(),
            None,
        );
        let (first, second, summary) = stopped_run(error).await;
        assert_eq!((first, second), (1, 0), "one request in all");
        let openai = catalog_model(Provider::OpenAI, 0);
        let reasoning_row = meerkat_models::canonical()
            .capabilities_for(Provider::OpenAI, &openai)
            .is_some_and(|row| row.supports_reasoning);
        let shape = BodyShape::of(true, ModelApi::Responses, reasoning_row).map(BodyShape::as_str);
        assert_eq!(
            human(&summary),
            [
                format!(
                    "copilot-live reasoning FAIL route=copilot/responses model={openai} \
                     shape={} level=low class=authentication_failed status=401 detail=-",
                    shape.expect("a shape")
                ),
                "copilot-live reasoning NOTE route=copilot/responses models_with_stated_levels=1 \
                 models_probed=1 levels_of_probed_model=2 levels_probed=0"
                    .to_string(),
                "copilot-live reasoning STOP class=authentication_failed status=401 requests=1/2 \
                 no further request is sent"
                    .to_string(),
                "copilot-live reasoning SKIP route=copilot/messages reason=run_stopped".to_string(),
                "copilot-live reasoning SUMMARY pass=0 fail=1 cut=0 skip=1 local=0 requests=1/2 \
                 account=<redacted>"
                    .to_string(),
            ]
        );
        assert_eq!(
            summary.stopped,
            Some(ProviderFailure {
                class: FailureClass::AuthenticationFailed,
                status: Some(401)
            })
        );
        // The receipt reads the failed cell as typed fields: the status, and
        // the failure class as its error code. The cell that never started
        // is no observation.
        let mut failed = copilot_observation(
            Provider::OpenAI,
            "responses",
            &openai,
            "low",
            sent_outcome("low"),
            "fail",
        );
        failed["http_status"] = json!(401);
        failed["error_code"] = json!("authentication_failed");
        assert_eq!(
            observations(&summary),
            [
                copilot_observation(
                    Provider::OpenAI,
                    "responses",
                    &openai,
                    "medium",
                    refused_outcome("unsupported_level"),
                    "pass",
                ),
                failed,
            ]
        );
        assert!(summary.lines.iter().all(|line| !line.contains("private")));
    }

    /// The first HTTP 429 a client reports ends the run, although the agent
    /// loop would retry a rate limit: the gate refuses the retries, and no
    /// other cell sends a request.
    ///
    /// Below the client seam nothing is sent again on a 429: the Copilot
    /// authorizer asks for a fresh authorization on a 401 only, and the
    /// clients answer every other status by returning its class
    /// (`LlmError::from_http_status`).
    #[tokio::test]
    async fn the_first_429_stops_the_run() {
        // The provider's own wait of 50 ms: longer than the loop's backoff
        // here, so the loop waits that long and not its 30 second floor.
        let error = LlmError::from_http_status(429, "private provider text".to_string(), Some(50));
        assert!(error.is_retryable(), "the agent loop retries this class");
        let (first, second, summary) = stopped_run(error).await;
        assert_eq!((first, second), (1, 0), "one request in all");
        assert_eq!(
            summary.stopped,
            Some(ProviderFailure {
                class: FailureClass::RateLimited,
                status: Some(429)
            })
        );
        assert_eq!((summary.pass, summary.fail, summary.skip), (0, 1, 1));
        assert!(
            summary.lines.iter().any(|line| line
                == "copilot-live reasoning STOP class=rate_limited status=429 requests=1/2 \
                    no further request is sent"),
            "{:?}",
            summary.lines
        );
        let failed = observations(&summary).pop().expect("the failed cell");
        assert_eq!(
            (
                &failed["outcome"],
                &failed["result"],
                &failed["http_status"],
                &failed["error_code"]
            ),
            (
                &sent_outcome("low"),
                &json!("fail"),
                &json!(429),
                &json!("rate_limited")
            )
        );
        assert!(summary.lines.iter().all(|line| !line.contains("private")));
    }

    /// A turn that asks for a second request (the model called the offered
    /// tool) is cut at the budget: one request reached the client, and the
    /// cell is neither a pass nor a failure.
    #[tokio::test]
    async fn a_turn_that_asks_for_a_second_request_is_cut_at_the_budget() {
        let client = scripted(
            Provider::OpenAI,
            ModelApi::Responses,
            stated(&[EffortLevel::Low, EffortLevel::High]),
            vec![Reply::ToolCall, Reply::Text],
        );
        let cells = assemble_cells(vec![candidate(Provider::OpenAI, 0, &client)]);
        let temp = tempfile::TempDir::new().expect("a directory");
        let run = run_in(&temp, 2);
        let summary = drive(&run, &cells).await;
        assert_eq!(client.requests().len(), 1, "the second request is not sent");
        assert_eq!(
            (
                summary.pass,
                summary.fail,
                summary.cut,
                summary.requests,
                summary.stopped
            ),
            (0, 0, 1, 1, None),
            "{:?}",
            summary.lines
        );
        let lines = human(&summary);
        assert!(
            lines[0].contains(" CUT route=copilot/responses ")
                && lines[0].ends_with(
                    "level=low reason=second_request_refused_by_budget first_request=answered"
                ),
            "{lines:?}"
        );
        // For the receipt a cut cell is skipped: sent, and neither passed
        // nor failed.
        assert_eq!(
            observations(&summary).pop(),
            Some(copilot_observation(
                Provider::OpenAI,
                "responses",
                &catalog_model(Provider::OpenAI, 0),
                "low",
                sent_outcome("low"),
                "skip",
            ))
        );
    }

    /// A refusal check that reaches the provider is a failure and spends the
    /// cell's only request: the cell never sends a second one.
    #[tokio::test]
    async fn a_refusal_that_reaches_the_provider_spends_the_cell() {
        let client = scripted(
            Provider::OpenAI,
            ModelApi::Responses,
            stated(&[EffortLevel::Low, EffortLevel::Medium]),
            vec![Reply::Text, Reply::Text],
        );
        let mut cells = assemble_cells(vec![candidate(Provider::OpenAI, 0, &client)]);
        // The cell is told a narrower set than the route accepts, so the
        // level it expects to be refused is sent instead.
        cells[0].probe.as_mut().expect("probed").supported = vec![EffortLevel::Low];
        let temp = tempfile::TempDir::new().expect("a directory");
        let run = run_in(&temp, 0);
        let summary = drive(&run, &cells).await;
        assert_eq!(client.requests().len(), 1, "never a second request");
        assert_eq!((summary.pass, summary.fail, summary.requests), (0, 2, 1));
        let lines = human(&summary);
        assert!(
            lines[0].ends_with(
                "level=medium class=refusal_dispatched status=- detail=level_outside_the_set"
            ) && lines[1].ends_with("level=low class=request_budget_spent status=- detail=-"),
            "{lines:?}"
        );
        // The receipt reads both failures by their class: the level that was
        // sent where it had to be refused, and the level that was never sent.
        let openai = catalog_model(Provider::OpenAI, 0);
        let mut dispatched = copilot_observation(
            Provider::OpenAI,
            "responses",
            &openai,
            "medium",
            sent_outcome("medium"),
            "fail",
        );
        dispatched["error_code"] = json!("refusal_dispatched");
        let mut spent = copilot_observation(
            Provider::OpenAI,
            "responses",
            &openai,
            "low",
            refused_outcome("request_budget_spent"),
            "fail",
        );
        spent["error_code"] = json!("request_budget_spent");
        assert_eq!(observations(&summary), [dispatched, spent]);
    }

    /// No cell starts at or after the run's deadline.
    #[tokio::test]
    async fn a_run_past_its_deadline_sends_nothing() {
        let client = scripted(
            Provider::OpenAI,
            ModelApi::Responses,
            stated(&[EffortLevel::Low]),
            vec![Reply::Text],
        );
        let cells = assemble_cells(vec![candidate(Provider::OpenAI, 0, &client)]);
        let temp = tempfile::TempDir::new().expect("a directory");
        let mut run = run_in(&temp, 0);
        run.deadline = Instant::now();
        let summary = drive(&run, &cells).await;
        assert_eq!(client.requests().len(), 0);
        assert_eq!(
            summary.lines,
            [
                "copilot-live reasoning SKIP route=copilot/responses reason=run_deadline",
                "copilot-live reasoning SUMMARY pass=0 fail=0 cut=0 skip=1 local=0 requests=0/1 \
                 account=<redacted>",
            ]
        );
    }

    /// Every line a run hands out went through the redactor: a private value
    /// that a typed field happens to hold is replaced too.
    #[tokio::test]
    async fn every_line_of_a_run_passes_the_redactor() {
        let client = scripted(
            Provider::OpenAI,
            ModelApi::Responses,
            stated(&[EffortLevel::Low]),
            vec![Reply::Text],
        );
        let cells = assemble_cells(vec![candidate(Provider::OpenAI, 0, &client)]);
        let temp = tempfile::TempDir::new().expect("a directory");
        let mut run = run_in(&temp, 0);
        let private = catalog_model(Provider::OpenAI, 0);
        run.redactor = Redactor::new([private.to_uppercase()]);
        let summary = drive(&run, &cells).await;
        assert_eq!(summary.pass, 1, "{:?}", summary.lines);
        assert!(
            human(&summary)[0].contains(" PASS route=copilot/responses model=<redacted> "),
            "{:?}",
            summary.lines
        );
        let cells = observations(&summary);
        assert_eq!(cells.len(), 2, "{cells:?}");
        assert!(cells.iter().all(|cell| cell["model"] == REDACTED));
        assert!(summary.lines.iter().all(|line| !line.contains(&private)));
    }
}
