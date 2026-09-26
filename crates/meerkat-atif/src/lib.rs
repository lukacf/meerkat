//! ATIF-v1.7 models and conversion from Meerkat's canonical agent events.
//!
//! The exporter consumes committed event envelopes. It does not observe a
//! surface-specific stream and therefore produces the same trajectory for
//! CLI, REST, RPC, MCP, embedded, and MobKit hosts.

#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use chrono::{DateTime, SecondsFormat, Utc};
use meerkat_core::event::{AgentEvent, EventEnvelope};
use meerkat_core::{ContentBlock, ContentInput, ImageData, RunInput, TurnUsage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const SCHEMA_VERSION: &str = "ATIF-v1.7";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Trajectory {
    pub schema_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory_id: Option<String>,
    pub agent: Agent,
    pub steps: Vec<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_metrics: Option<FinalMetrics>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subagent_trajectories: Vec<Trajectory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continued_trajectory_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Agent {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_definitions: Option<Vec<Map<String, Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Step {
    pub step_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub source: StepSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    pub message: AtifContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<Observation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_call_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_copied_context: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StepSource {
    System,
    User,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AtifContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentPart {
    Text { text: String },
    Image { source: ImageSource },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSource {
    pub media_type: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub function_name: String,
    pub arguments: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub results: Vec<ObservationResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservationResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<AtifContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_trajectory_ref: Option<SubagentTrajectoryRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubagentTrajectoryRef {
    pub trajectory_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trajectory_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Metrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_token_ids: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_token_ids: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FinalMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cached_tokens: Option<u64>,
    pub total_steps: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExportError {
    #[error("tool call {0} did not contain a JSON object of arguments")]
    InvalidArguments(String),
}

#[derive(Debug, Clone)]
struct PendingTurn {
    /// The loop's turn counter from the `TurnStarted` that opened this turn;
    /// `None` when a content event arrived with no announced turn.
    turn_number: Option<u32>,
    timestamp: Option<String>,
    message: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    observations: Vec<ObservationResult>,
}

impl PendingTurn {
    fn new(timestamp_ms: u64) -> Self {
        Self {
            turn_number: None,
            timestamp: timestamp(timestamp_ms),
            message: String::new(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
            observations: Vec::new(),
        }
    }

    fn started(turn_number: u32, timestamp_ms: u64) -> Self {
        Self {
            turn_number: Some(turn_number),
            ..Self::new(timestamp_ms)
        }
    }

    /// Whether the turn recorded any output: text, reasoning, a tool call or
    /// a tool observation.
    fn recorded_output(&self) -> bool {
        !self.message.is_empty()
            || !self.reasoning.is_empty()
            || !self.tool_calls.is_empty()
            || !self.observations.is_empty()
    }
}

/// Export one session's committed event stream as an ATIF trajectory.
pub fn trajectory_from_events(
    events: &[EventEnvelope<AgentEvent>],
    agent: Agent,
) -> Result<Trajectory, ExportError> {
    let mut builder = TrajectoryBuilder::new();
    builder.extend(events)?;
    Ok(builder.finish(agent))
}

/// Incremental exporter: fold committed event pages into a trajectory as they
/// are read, so a host replaying a durable log never holds the whole log in
/// memory. The exported document is the fold's only accumulation.
#[derive(Debug, Default)]
pub struct TrajectoryBuilder {
    steps: Vec<Step>,
    pending: Option<PendingTurn>,
    session_id: Option<String>,
    totals: FinalMetrics,
    terminal_status: Option<&'static str>,
    failure_detail: Option<String>,
    retained_bytes: usize,
    charged_steps: usize,
    /// Between a `RunCompleted` that requires extraction and the extraction
    /// outcome. Extraction streams its JSON as text deltas; they belong to
    /// the extraction steps the outcome event records, not to a new turn.
    extraction_open: bool,
}

impl TrajectoryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Retained payload bytes in the steps folded so far: a strict lower bound
    /// on the serialized size of the document this fold will emit.
    ///
    /// It sums only the UTF-8 lengths of strings that serialize into the
    /// document, so it never exceeds the real size, and a host may refuse a
    /// fold whose bound already passed a response limit knowing the finished
    /// document would pass it too. The live pending turn is excluded (it is
    /// bounded by one turn and may still be replaced by a later
    /// `TextComplete`), which keeps the bound conservative.
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// Name the exported session before any event is folded in. A `RunStarted`
    /// event replaces this with the identity recorded in the log; a log with no
    /// run keeps it, so an eventless session still exports a trajectory that
    /// names the session it came from.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Fold one committed event envelope into the trajectory under
    /// construction.
    pub fn push(&mut self, envelope: &EventEnvelope<AgentEvent>) -> Result<(), ExportError> {
        let steps = &mut self.steps;
        let pending = &mut self.pending;
        if self.extraction_open
            && matches!(
                &envelope.payload,
                AgentEvent::TextDelta { .. }
                    | AgentEvent::TextComplete { .. }
                    | AgentEvent::ReasoningDelta { .. }
                    | AgentEvent::ReasoningComplete { .. }
            )
        {
            return Ok(());
        }
        match &envelope.payload {
            AgentEvent::RunStarted {
                session_id: id,
                input,
            } => {
                self.session_id = Some(id.to_string());
                // A new run opens with no extraction phase in progress, even
                // when an earlier run's extraction never published an outcome.
                self.extraction_open = false;
                // A turn still pending belongs to an earlier run that ended
                // without a terminal event. It precedes this run's input.
                close_unfinished_turn(steps, pending.take());
                if let RunInput::Content { content } = input {
                    steps.push(Step {
                        step_id: next_id(steps),
                        timestamp: timestamp(envelope.timestamp_ms),
                        source: StepSource::User,
                        model_name: None,
                        message: atif_content(content),
                        reasoning_content: None,
                        tool_calls: Vec::new(),
                        observation: None,
                        metrics: None,
                        llm_call_count: None,
                        extra: None,
                        reasoning_effort: None,
                        is_copied_context: None,
                    });
                }
            }
            AgentEvent::TurnStarted { turn_number } => {
                match pending.take() {
                    // The loop re-announces the turn it is already in when a
                    // compaction boundary sends it back to rebuild the request
                    // (`CallingLlmGate::Repoll` does not advance the turn
                    // counter). The earlier announcement produced no committed
                    // response, so the re-entered request replaces it instead
                    // of becoming an empty, unmetered step.
                    Some(turn) if turn.turn_number == Some(*turn_number) => {}
                    // Every other provider request is a step. A turn still
                    // pending here never published `TurnCompleted`: logs
                    // written before tool-loop turns published their
                    // completion carry one such turn per tool round. It is
                    // kept as an unmetered step rather than overwritten, which
                    // used to reduce a tool-using run to its final answer.
                    turn => close_unfinished_turn(steps, turn),
                }
                *pending = Some(PendingTurn::started(*turn_number, envelope.timestamp_ms));
            }
            AgentEvent::ReasoningDelta { delta } => pending
                .get_or_insert_with(|| PendingTurn::new(envelope.timestamp_ms))
                .reasoning
                .push_str(delta),
            AgentEvent::ReasoningComplete { content } => {
                pending
                    .get_or_insert_with(|| PendingTurn::new(envelope.timestamp_ms))
                    .reasoning = content.clone();
            }
            AgentEvent::TextDelta { delta } => pending
                .get_or_insert_with(|| PendingTurn::new(envelope.timestamp_ms))
                .message
                .push_str(delta),
            AgentEvent::TextComplete { content } => {
                pending
                    .get_or_insert_with(|| PendingTurn::new(envelope.timestamp_ms))
                    .message = content.clone();
            }
            AgentEvent::ToolCallRequested { id, name, args } => {
                let arguments = args
                    .as_value()
                    .as_object()
                    .cloned()
                    .ok_or_else(|| ExportError::InvalidArguments(id.clone()))?;
                pending
                    .get_or_insert_with(|| PendingTurn::new(envelope.timestamp_ms))
                    .tool_calls
                    .push(ToolCall {
                        tool_call_id: id.clone(),
                        function_name: name.clone(),
                        arguments,
                        extra: None,
                    });
            }
            AgentEvent::ToolExecutionCompleted {
                id,
                content,
                is_error,
                ..
            } => pending
                .get_or_insert_with(|| PendingTurn::new(envelope.timestamp_ms))
                .observations
                .push(ObservationResult {
                    source_call_id: Some(id.clone()),
                    content: Some(atif_blocks(content)),
                    extra: (*is_error)
                        .then(|| Map::from_iter([(String::from("is_error"), Value::Bool(true))])),
                    subagent_trajectory_ref: None,
                }),
            AgentEvent::ToolExecutionTimedOut { id, timeout_ms, .. } => pending
                .get_or_insert_with(|| PendingTurn::new(envelope.timestamp_ms))
                .observations
                .push(ObservationResult {
                    source_call_id: Some(id.clone()),
                    content: Some(AtifContent::Text(format!(
                        "tool execution timed out after {timeout_ms}ms"
                    ))),
                    extra: Some(Map::from_iter([(
                        String::from("timed_out"),
                        Value::Bool(true),
                    )])),
                    subagent_trajectory_ref: None,
                }),
            AgentEvent::ServerToolContent { id, content, kind } => pending
                .get_or_insert_with(|| PendingTurn::new(envelope.timestamp_ms))
                .observations
                .push(ObservationResult {
                    source_call_id: id.clone(),
                    content: Some(AtifContent::Text(content.to_string())),
                    extra: Some(Map::from_iter([(
                        String::from("server_tool_kind"),
                        serde_json::to_value(kind).unwrap_or(Value::Null),
                    )])),
                    subagent_trajectory_ref: None,
                }),
            AgentEvent::TurnCompleted { usage, .. } => {
                if let Some(turn) = pending.take() {
                    // An unaccounted turn is exported as a step with no usage
                    // block and contributes nothing to the totals. Folding a
                    // zero in would publish a measurement that was never made
                    // and silently understate the trajectory's real cost.
                    append_agent_step(steps, turn, usage.as_ref());
                    if let Some(usage) = usage.as_ref() {
                        add_totals(&mut self.totals, usage);
                    }
                }
            }
            AgentEvent::RunCompleted {
                result,
                extraction_required,
                ..
            } => {
                self.terminal_status = Some("completed");
                self.extraction_open = *extraction_required;
                if let Some(mut turn) = pending.take() {
                    if turn.message.is_empty() {
                        turn.message.clone_from(result);
                    }
                    append_agent_step(steps, turn, None);
                }
            }
            AgentEvent::RunFailed { error_report, .. } => {
                // A run can fail inside its extraction phase (after
                // `RunCompleted { extraction_required: true }`) without an
                // extraction outcome event. The phase ends with the run.
                self.extraction_open = false;
                self.terminal_status = Some("failed");
                self.failure_detail = Some(error_report.message.clone());
                flush_failed_turn(steps, pending, error_report.message.clone());
            }
            AgentEvent::ExtractionSucceeded {
                structured_output,
                request_usage,
                origin,
                ..
            } => {
                self.extraction_open = false;
                if let Some(turn) = pending.take() {
                    append_agent_step(steps, turn, None);
                }
                // The main run closed at `RunCompleted`; each extraction
                // request that followed is its own step. A success from an
                // extraction request proves at least one request was answered,
                // so a log without rows (an unmeasured provider, or a log
                // written before extraction published them) still records one
                // unmetered step. When validate-first accepted the run's final
                // reply (which the step closed at `RunCompleted` already
                // records), no extraction request was sent and no extraction
                // step is added.
                let attempts = if origin.is_extraction_request() {
                    request_usage.len().max(1)
                } else {
                    0
                };
                for attempt in 0..attempts {
                    let usage = request_usage.get(attempt);
                    let message = if attempt + 1 == attempts {
                        structured_output.to_string()
                    } else {
                        String::new()
                    };
                    append_extraction_step(steps, envelope.timestamp_ms, message, usage, None);
                    if let Some(usage) = usage {
                        add_totals(&mut self.totals, usage);
                    }
                }
            }
            AgentEvent::ExtractionFailed {
                last_output,
                reason,
                request_usage,
                ..
            } => {
                self.terminal_status = Some("failed");
                self.extraction_open = false;
                let detail = format!("{reason}; last_output={last_output}");
                self.failure_detail = Some(detail.clone());
                // A turn still pending here is the main turn of a run whose
                // extraction setup failed before `RunCompleted`.
                flush_failed_turn(steps, pending, detail.clone());
                let attempts = request_usage.len();
                for (attempt, usage) in request_usage.iter().enumerate() {
                    let failure = (attempt + 1 == attempts).then(|| detail.clone());
                    append_extraction_step(
                        steps,
                        envelope.timestamp_ms,
                        String::new(),
                        Some(usage),
                        failure,
                    );
                    add_totals(&mut self.totals, usage);
                }
            }
            _ => {}
        }
        // Steps are charged once, after this fold step has finished with them:
        // `flush_failed_turn` still edits the step it just appended, and a
        // pending turn's text can be replaced up to the moment it becomes a
        // step, so charging any earlier would overcount.
        while self.charged_steps < self.steps.len() {
            if let Some(step) = self.steps.get(self.charged_steps) {
                self.retained_bytes = self.retained_bytes.saturating_add(step_payload_bytes(step));
            }
            self.charged_steps = self.charged_steps.saturating_add(1);
        }
        Ok(())
    }

    /// Fold a page of committed event envelopes in replay order.
    pub fn extend<'envelope, I>(&mut self, envelopes: I) -> Result<(), ExportError>
    where
        I: IntoIterator<Item = &'envelope EventEnvelope<AgentEvent>>,
    {
        for envelope in envelopes {
            self.push(envelope)?;
        }
        Ok(())
    }

    /// Close the fold and emit the trajectory document.
    pub fn finish(self, agent: Agent) -> Trajectory {
        let Self {
            steps,
            pending: _,
            session_id,
            mut totals,
            terminal_status,
            failure_detail,
            retained_bytes: _,
            charged_steps: _,
            extraction_open: _,
        } = self;
        totals.total_steps = steps.len() as u64;
        let extra = terminal_status.map(|status| {
            let mut extra = Map::from_iter([(
                String::from("terminal_status"),
                Value::String(status.to_string()),
            )]);
            if let Some(detail) = failure_detail {
                extra.insert(String::from("failure_detail"), Value::String(detail));
            }
            extra
        });
        Trajectory {
            schema_version: SCHEMA_VERSION.to_string(),
            session_id,
            trajectory_id: None,
            agent,
            steps,
            final_metrics: Some(totals),
            subagent_trajectories: Vec::new(),
            notes: None,
            continued_trajectory_ref: None,
            extra,
        }
    }
}

impl Trajectory {
    /// Serialize this trajectory in the interchange format used by Harbor.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Embed independently exported member trajectories in a parent document.
    /// Each embedded trajectory gets a document identity used by ATIF refs.
    pub fn with_subagent_trajectories(mut self, trajectories: Vec<Trajectory>) -> Self {
        self.subagent_trajectories = trajectories
            .into_iter()
            .enumerate()
            .map(|(index, mut trajectory)| {
                if trajectory.trajectory_id.is_none() {
                    trajectory.trajectory_id = Some(format!("subagent-{}", index + 1));
                }
                trajectory
            })
            .collect();
        self
    }
}

/// Retained payload bytes of one finished step: only the UTF-8 lengths of
/// strings that serialize into the document, so the sum stays a lower bound on
/// the serialized size (JSON quoting, keys, and punctuation only add to it, and
/// string escaping only expands).
fn step_payload_bytes(step: &Step) -> usize {
    let mut bytes = content_payload_bytes(&step.message);
    if let Some(reasoning) = &step.reasoning_content {
        bytes = bytes.saturating_add(reasoning.len());
    }
    for call in &step.tool_calls {
        bytes = bytes
            .saturating_add(call.tool_call_id.len())
            .saturating_add(call.function_name.len())
            .saturating_add(map_payload_bytes(&call.arguments));
    }
    if let Some(observation) = &step.observation {
        for result in &observation.results {
            if let Some(source_call_id) = &result.source_call_id {
                bytes = bytes.saturating_add(source_call_id.len());
            }
            if let Some(content) = &result.content {
                bytes = bytes.saturating_add(content_payload_bytes(content));
            }
        }
    }
    if let Some(extra) = &step.extra {
        bytes = bytes.saturating_add(map_payload_bytes(extra));
    }
    bytes
}

fn content_payload_bytes(content: &AtifContent) -> usize {
    match content {
        AtifContent::Text(text) => text.len(),
        AtifContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => text.len(),
                ContentPart::Image { source } => {
                    source.media_type.len().saturating_add(source.path.len())
                }
            })
            .fold(0usize, usize::saturating_add),
    }
}

fn json_payload_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Array(items) => items
            .iter()
            .map(json_payload_bytes)
            .fold(0usize, usize::saturating_add),
        Value::Object(map) => map_payload_bytes(map),
        // Numbers, booleans, and null are structure, not payload; leaving them
        // out is what keeps this a lower bound.
        _ => 0,
    }
}

fn map_payload_bytes(map: &Map<String, Value>) -> usize {
    map.iter()
        .map(|(key, value)| key.len().saturating_add(json_payload_bytes(value)))
        .fold(0usize, usize::saturating_add)
}

fn append_agent_step(steps: &mut Vec<Step>, turn: PendingTurn, usage: Option<&TurnUsage>) {
    steps.push(Step {
        step_id: next_id(steps),
        timestamp: turn.timestamp,
        source: StepSource::Agent,
        model_name: usage.map(|usage| usage.accounting().model.clone()),
        message: AtifContent::Text(turn.message),
        reasoning_content: (!turn.reasoning.is_empty()).then_some(turn.reasoning),
        tool_calls: turn.tool_calls,
        observation: (!turn.observations.is_empty()).then_some(Observation {
            results: turn.observations,
        }),
        metrics: usage.map(step_metrics),
        llm_call_count: Some(1),
        extra: None,
        reasoning_effort: None,
        is_copied_context: None,
    });
}

/// One structured-output extraction request as an agent step, marked so a
/// consumer can tell it from the agentic turns that produced the answer.
fn append_extraction_step(
    steps: &mut Vec<Step>,
    timestamp_ms: u64,
    message: String,
    usage: Option<&TurnUsage>,
    failure_detail: Option<String>,
) {
    let mut extra = Map::from_iter([(
        String::from("request_kind"),
        Value::String(String::from("structured_output_extraction")),
    )]);
    if let Some(detail) = failure_detail {
        extra.insert(String::from("failure_detail"), Value::String(detail));
    }
    steps.push(Step {
        step_id: next_id(steps),
        timestamp: timestamp(timestamp_ms),
        source: StepSource::Agent,
        model_name: usage.map(|usage| usage.accounting().model.clone()),
        message: AtifContent::Text(message),
        reasoning_content: None,
        tool_calls: Vec::new(),
        observation: None,
        metrics: usage.map(step_metrics),
        llm_call_count: Some(1),
        extra: Some(extra),
        reasoning_effort: None,
        is_copied_context: None,
    });
}

/// Per-step metrics for one provider request. `prompt_tokens` is the
/// presented input on every provider (never the raw, for Anthropic
/// uncached-only, counter), so step metrics add up to the run total.
fn step_metrics(usage: &TurnUsage) -> Metrics {
    let mut extra = Map::new();
    if let Some(written) = usage.cache_creation_tokens {
        extra.insert(
            String::from("cache_creation_tokens"),
            Value::from(written.min(usage.presented_tokens())),
        );
    }
    if let Some(reasoning) = usage.reasoning_tokens {
        extra.insert(
            String::from("reasoning_tokens"),
            Value::from(reasoning.min(usage.output_tokens)),
        );
    }
    Metrics {
        prompt_tokens: Some(usage.presented_tokens()),
        completion_tokens: Some(usage.output_tokens),
        cached_tokens: usage
            .cache_read_tokens
            .map(|cached| cached.min(usage.presented_tokens())),
        cost_usd: None,
        logprobs: None,
        prompt_token_ids: None,
        completion_token_ids: None,
        extra: (!extra.is_empty()).then_some(extra),
    }
}

/// Export a turn that ended without `TurnCompleted` as an unmetered step. A
/// turn that recorded no output is not evidence of an answered provider
/// request, so it is dropped rather than exported as an empty step that claims
/// one LLM call.
fn close_unfinished_turn(steps: &mut Vec<Step>, turn: Option<PendingTurn>) {
    if let Some(turn) = turn.filter(PendingTurn::recorded_output) {
        append_agent_step(steps, turn, None);
    }
}

fn flush_failed_turn(steps: &mut Vec<Step>, pending: &mut Option<PendingTurn>, detail: String) {
    if let Some(turn) = pending.take() {
        append_agent_step(steps, turn, None);
        if let Some(step) = steps.last_mut() {
            step.extra = Some(Map::from_iter([(
                String::from("failure_detail"),
                Value::String(detail),
            )]));
        }
    }
}

fn add_totals(totals: &mut FinalMetrics, usage: &TurnUsage) {
    totals.total_prompt_tokens = Some(
        totals
            .total_prompt_tokens
            .unwrap_or_default()
            .saturating_add(usage.presented_tokens()),
    );
    totals.total_completion_tokens = Some(
        totals
            .total_completion_tokens
            .unwrap_or_default()
            .saturating_add(usage.output_tokens),
    );
    if let Some(cached) = usage.cache_read_tokens {
        totals.total_cached_tokens = Some(
            totals
                .total_cached_tokens
                .unwrap_or_default()
                .saturating_add(cached.min(usage.presented_tokens())),
        );
    }
    let details = [
        (
            "total_cache_creation_tokens",
            usage
                .cache_creation_tokens
                .map(|written| written.min(usage.presented_tokens())),
        ),
        (
            "total_reasoning_tokens",
            usage
                .reasoning_tokens
                .map(|reasoning| reasoning.min(usage.output_tokens)),
        ),
    ];
    for (key, count) in details {
        let Some(count) = count else { continue };
        let extra = totals.extra.get_or_insert_with(Map::new);
        let previous = extra.get(key).and_then(Value::as_u64).unwrap_or_default();
        extra.insert(key.to_string(), Value::from(previous.saturating_add(count)));
    }
}
fn next_id(steps: &[Step]) -> u64 {
    steps.len() as u64 + 1
}
fn timestamp(ms: u64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(ms as i64)
        .map(|d| d.to_rfc3339_opts(SecondsFormat::Millis, true))
}
fn atif_content(content: &ContentInput) -> AtifContent {
    match content {
        ContentInput::Text(text) => AtifContent::Text(text.clone()),
        ContentInput::Blocks(blocks) => atif_blocks(blocks),
    }
}
fn atif_blocks(blocks: &[ContentBlock]) -> AtifContent {
    if blocks
        .iter()
        .all(|block| matches!(block, ContentBlock::Text { .. }))
    {
        return AtifContent::Text(
            blocks
                .iter()
                .map(ContentBlock::text_projection)
                .collect::<Vec<_>>()
                .join(""),
        );
    }
    AtifContent::Parts(
        blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => ContentPart::Text { text: text.clone() },
                ContentBlock::Image { media_type, data } => ContentPart::Image {
                    source: ImageSource {
                        media_type: media_type.clone(),
                        path: match data {
                            ImageData::Inline { data } => {
                                format!("data:{media_type};base64,{data}")
                            }
                            ImageData::Blob { blob_id } => format!("blob:{blob_id}"),
                        },
                    },
                },
                block => ContentPart::Text {
                    text: block.text_projection().into_owned(),
                },
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::event::{EventEnvelope, EventSourceIdentity};
    use meerkat_core::{AgentEvent, ContentInput, RunInput, SessionId, Usage};

    #[test]
    fn exports_user_tool_and_agent_steps_with_metrics() {
        let id = SessionId::new();
        let events = vec![
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                1,
                None,
                AgentEvent::RunStarted {
                    session_id: id.clone(),
                    input: RunInput::Content {
                        content: ContentInput::Text("hello".into()),
                    },
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                2,
                None,
                AgentEvent::TurnStarted { turn_number: 0 },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                3,
                None,
                AgentEvent::ToolCallRequested {
                    id: "call-1".into(),
                    name: "echo".into(),
                    args: meerkat_core::event::ToolCallArguments::from_value(
                        serde_json::json!({"x": 1}),
                    )
                    .unwrap(),
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                4,
                None,
                AgentEvent::ToolExecutionCompleted {
                    id: "call-1".into(),
                    name: "echo".into(),
                    content: ContentBlock::text_vec("ok".into()),
                    is_error: false,
                    duration_ms: 1,
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                5,
                None,
                AgentEvent::TextComplete {
                    content: "done".into(),
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id),
                6,
                None,
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(TurnUsage::new(
                        Usage {
                            input_tokens: 4,
                            output_tokens: 2,
                            cache_creation_tokens: None,
                            cache_read_tokens: Some(1),
                            reasoning_tokens: None,
                            provider_accounting: None,
                        },
                        meerkat_core::ProviderTokenAccounting::openai("test", 4),
                    )),
                },
            ),
        ];
        let trajectory = trajectory_from_events(
            &events,
            Agent {
                name: "meerkat".into(),
                version: "0.8".into(),
                model_name: None,
                tool_definitions: None,
                extra: None,
            },
        )
        .unwrap();
        assert_eq!(trajectory.steps.len(), 2);
        assert_eq!(trajectory.steps[1].tool_calls[0].function_name, "echo");
        assert_eq!(
            trajectory.steps[1].metrics.as_ref().unwrap().prompt_tokens,
            Some(4)
        );
        assert_eq!(trajectory.schema_version, "ATIF-v1.7");
    }

    fn test_agent() -> Agent {
        Agent {
            name: "meerkat".into(),
            version: "0.8".into(),
            model_name: None,
            tool_definitions: None,
            extra: None,
        }
    }

    /// An existing session with an empty durable log is an empty trajectory
    /// that still names its session, not a missing session.
    #[test]
    fn eventless_log_exports_an_empty_trajectory_naming_its_session() {
        let id = SessionId::new();
        let trajectory = TrajectoryBuilder::new()
            .with_session_id(id.to_string())
            .finish(test_agent());
        assert_eq!(
            trajectory.session_id.as_deref(),
            Some(id.to_string().as_str())
        );
        assert!(trajectory.steps.is_empty());
        assert_eq!(
            trajectory.final_metrics.as_ref().map(|m| m.total_steps),
            Some(0)
        );
        // No terminal status is claimed for a session that never ran.
        assert!(trajectory.extra.is_none());
    }

    /// Folding page by page must produce the same document as one slice, so a
    /// paginating host never diverges from the whole-log exporter.
    #[test]
    fn paged_folding_matches_whole_slice_export() {
        let id = SessionId::new();
        let run_session_id = id.clone();
        let events = vec![
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                1,
                None,
                AgentEvent::RunStarted {
                    session_id: id.clone(),
                    input: RunInput::Content {
                        content: ContentInput::Text("hello".into()),
                    },
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                2,
                None,
                AgentEvent::TurnStarted { turn_number: 0 },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                3,
                None,
                AgentEvent::TextDelta {
                    delta: "par".into(),
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                4,
                None,
                AgentEvent::TextDelta {
                    delta: "tial".into(),
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                5,
                None,
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(TurnUsage::new(
                        Usage {
                            input_tokens: 3,
                            output_tokens: 1,
                            cache_creation_tokens: None,
                            cache_read_tokens: None,
                            reasoning_tokens: None,
                            provider_accounting: None,
                        },
                        meerkat_core::ProviderTokenAccounting::openai("test", 3),
                    )),
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id),
                6,
                None,
                AgentEvent::RunCompleted {
                    session_id: run_session_id,
                    result: "partial".into(),
                    structured_output: None,
                    extraction_required: false,
                    usage: Usage::default().into(),
                    terminal_cause_kind: None,
                },
            ),
        ];
        let whole = trajectory_from_events(&events, test_agent()).unwrap();
        // Every page size, so every boundary placement is swept: the delta run
        // is cut in the middle at some of these, and a turn spans pages at
        // others.
        for page_size in 1..=events.len() {
            let mut builder = TrajectoryBuilder::new();
            for page in events.chunks(page_size) {
                builder.extend(page).unwrap();
            }
            let retained_bytes = builder.retained_bytes();
            let paged = builder.finish(test_agent());
            assert_eq!(paged, whole, "page size {page_size} diverged");
            assert_eq!(paged.steps[1].message, AtifContent::Text("partial".into()));
            // The fold's byte bound is what a host budgets against, so it must
            // never exceed the document it is bounding, at any page size.
            assert!(
                retained_bytes <= paged.to_json().unwrap().len(),
                "page size {page_size}: retained bound {retained_bytes} exceeds the document"
            );
            assert!(
                retained_bytes >= "hello".len() + "partial".len(),
                "page size {page_size}: retained bound {retained_bytes} misses folded text"
            );
        }
    }

    /// The retained-byte bound must stay a lower bound on the serialized
    /// document for the payload shapes that dominate a tool-heavy log, since a
    /// host refuses exports on it.
    #[test]
    fn retained_bytes_stays_below_the_serialized_document() {
        let id = SessionId::new();
        let arguments = serde_json::json!({
            "query": "a".repeat(4096),
            "nested": { "path": "b".repeat(2048), "flag": true, "count": 7 },
            "items": ["c".repeat(512), 12, null],
        });
        let events = vec![
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                1,
                None,
                AgentEvent::TurnStarted { turn_number: 0 },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                2,
                None,
                AgentEvent::ReasoningComplete {
                    content: "d".repeat(1024),
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                3,
                None,
                AgentEvent::ToolCallRequested {
                    id: "call-1".into(),
                    name: "search".into(),
                    args: meerkat_core::event::ToolCallArguments::from_value(arguments).unwrap(),
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id.clone()),
                4,
                None,
                AgentEvent::ToolExecutionCompleted {
                    id: "call-1".into(),
                    name: "search".into(),
                    content: ContentBlock::text_vec("e".repeat(8192)),
                    is_error: false,
                    duration_ms: 3,
                },
            ),
            EventEnvelope::new_with_source(
                EventSourceIdentity::session(id),
                5,
                None,
                AgentEvent::TextComplete {
                    content: "f".repeat(256),
                },
            ),
        ];
        let mut builder = TrajectoryBuilder::new();
        builder.extend(&events).unwrap();
        // The turn never completed, so its text is still pending and excluded.
        assert_eq!(builder.retained_bytes(), 0);
        builder
            .push(&EventEnvelope::new_with_source(
                EventSourceIdentity::session(SessionId::new()),
                6,
                None,
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(TurnUsage::new(
                        Usage::default(),
                        meerkat_core::ProviderTokenAccounting::openai("test", 0),
                    )),
                },
            ))
            .unwrap();
        let retained_bytes = builder.retained_bytes();
        let document = builder.finish(test_agent()).to_json().unwrap();
        assert!(
            retained_bytes >= 8192 + 4096 + 2048 + 1024 + 512 + 256,
            "the bound must account for the payloads it folded: {retained_bytes}"
        );
        assert!(
            retained_bytes <= document.len(),
            "the bound {retained_bytes} must not exceed the document it bounds ({} bytes)",
            document.len()
        );
    }

    fn envelope(id: &SessionId, seq: u64, event: AgentEvent) -> EventEnvelope<AgentEvent> {
        EventEnvelope::new_with_source(EventSourceIdentity::session(id.clone()), seq, None, event)
    }

    fn openai_usage(prompt: u64, output: u64, cached: u64, reasoning: u64) -> TurnUsage {
        TurnUsage::new(
            Usage {
                input_tokens: prompt,
                output_tokens: output,
                cache_creation_tokens: None,
                cache_read_tokens: Some(cached),
                reasoning_tokens: Some(reasoning),
                provider_accounting: None,
            },
            meerkat_core::ProviderTokenAccounting::openai("gpt-test", prompt),
        )
    }

    /// The events of one structured-output run: a tool-loop turn, the
    /// answering turn, and two extraction requests. `tool_turn_completes`
    /// selects the current log shape (the tool-loop turn publishes
    /// `TurnCompleted`) or the shape earlier releases wrote (it does not, and
    /// extraction publishes no rows).
    fn tool_and_extraction_run(
        id: &SessionId,
        tool_turn_completes: bool,
    ) -> Vec<EventEnvelope<AgentEvent>> {
        let mut events = vec![
            AgentEvent::RunStarted {
                session_id: id.clone(),
                input: RunInput::Content {
                    content: ContentInput::Text("review this".into()),
                },
            },
            AgentEvent::TurnStarted { turn_number: 0 },
            AgentEvent::ToolCallRequested {
                id: "call-1".into(),
                name: "read_file".into(),
                args: meerkat_core::event::ToolCallArguments::from_value(
                    serde_json::json!({"path": "src/lib.rs"}),
                )
                .unwrap(),
            },
            AgentEvent::ToolExecutionCompleted {
                id: "call-1".into(),
                name: "read_file".into(),
                content: ContentBlock::text_vec("fn main() {}".into()),
                is_error: false,
                duration_ms: 2,
            },
        ];
        if tool_turn_completes {
            events.push(AgentEvent::TurnCompleted {
                stop_reason: meerkat_core::StopReason::ToolUse,
                usage: Some(openai_usage(1000, 10, 0, 3)),
            });
        }
        events.extend([
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::TextComplete {
                content: "looks fine".into(),
            },
            AgentEvent::TurnCompleted {
                stop_reason: meerkat_core::StopReason::EndTurn,
                usage: Some(openai_usage(1200, 20, 1000, 4)),
            },
            AgentEvent::RunCompleted {
                session_id: id.clone(),
                result: "looks fine".into(),
                structured_output: None,
                extraction_required: true,
                usage: Usage::default().into(),
                terminal_cause_kind: None,
            },
            // Extraction streams its JSON as deltas; they are not a turn.
            AgentEvent::TextDelta {
                delta: r#"{"comments": []}"#.into(),
            },
            AgentEvent::ExtractionSucceeded {
                session_id: id.clone(),
                structured_output: serde_json::json!({"comments": []}),
                schema_warnings: None,
                request_usage: if tool_turn_completes {
                    vec![
                        openai_usage(1250, 25, 1200, 5),
                        openai_usage(1300, 30, 1250, 6),
                    ]
                } else {
                    Vec::new()
                },
                origin: meerkat_core::StructuredOutputOrigin::ExtractionRequest,
            },
        ]);
        events
            .into_iter()
            .enumerate()
            .map(|(index, event)| envelope(id, index as u64 + 1, event))
            .collect()
    }

    /// Every provider request of the run is a step with its own metrics: the
    /// tool-loop turn, the answering turn, and each extraction request.
    #[test]
    fn every_provider_request_is_a_step_with_metrics() {
        let id = SessionId::new();
        let trajectory =
            trajectory_from_events(&tool_and_extraction_run(&id, true), test_agent()).unwrap();
        let agent_steps = trajectory
            .steps
            .iter()
            .filter(|step| step.source == StepSource::Agent)
            .collect::<Vec<_>>();
        assert_eq!(
            agent_steps.len(),
            4,
            "tool turn, answer, two extraction requests"
        );

        let tool_step = agent_steps[0];
        assert_eq!(tool_step.tool_calls[0].function_name, "read_file");
        assert_eq!(
            tool_step.observation.as_ref().unwrap().results[0]
                .source_call_id
                .as_deref(),
            Some("call-1")
        );
        let prompts = agent_steps
            .iter()
            .map(|step| {
                step.metrics
                    .as_ref()
                    .and_then(|metrics| metrics.prompt_tokens)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            prompts,
            vec![Some(1000), Some(1200), Some(1250), Some(1300)]
        );
        assert_eq!(
            agent_steps[1].message,
            AtifContent::Text("looks fine".into())
        );
        for step in &agent_steps[2..] {
            assert_eq!(
                step.extra
                    .as_ref()
                    .and_then(|extra| extra.get("request_kind")),
                Some(&Value::String("structured_output_extraction".into()))
            );
        }
        assert_eq!(
            agent_steps[3].message,
            AtifContent::Text(r#"{"comments":[]}"#.into()),
            "the last extraction request carries the extracted output"
        );
        assert_eq!(
            agent_steps[3]
                .metrics
                .as_ref()
                .and_then(|metrics| metrics.extra.as_ref())
                .and_then(|extra| extra.get("reasoning_tokens")),
            Some(&Value::from(6))
        );
        assert_eq!(agent_steps[0].model_name.as_deref(), Some("gpt-test"));

        let totals = trajectory.final_metrics.as_ref().unwrap();
        assert_eq!(totals.total_steps, 5);
        assert_eq!(totals.total_prompt_tokens, Some(1000 + 1200 + 1250 + 1300));
        assert_eq!(totals.total_completion_tokens, Some(10 + 20 + 25 + 30));
        assert_eq!(totals.total_cached_tokens, Some(1000 + 1200 + 1250));
        assert_eq!(
            totals
                .extra
                .as_ref()
                .and_then(|extra| extra.get("total_reasoning_tokens")),
            Some(&Value::from(3 + 4 + 5 + 6))
        );
    }

    /// The log shape earlier releases wrote: tool-loop turns never published
    /// `TurnCompleted` and extraction published no rows. The exporter used to
    /// overwrite each such turn at the next `TurnStarted`, so a tool-using
    /// run exported only its final answer. Every turn now survives, unmetered
    /// where the log recorded no accounting.
    #[test]
    fn a_log_without_tool_turn_completions_keeps_every_turn() {
        let id = SessionId::new();
        let trajectory =
            trajectory_from_events(&tool_and_extraction_run(&id, false), test_agent()).unwrap();
        let agent_steps = trajectory
            .steps
            .iter()
            .filter(|step| step.source == StepSource::Agent)
            .collect::<Vec<_>>();
        assert_eq!(
            agent_steps.len(),
            3,
            "tool turn, answer, the extraction request"
        );
        assert_eq!(agent_steps[0].tool_calls[0].function_name, "read_file");
        assert!(agent_steps[0].observation.is_some());
        assert!(
            agent_steps[0].metrics.is_none(),
            "a turn the log never accounted for is unmetered, not zero"
        );
        assert_eq!(
            agent_steps[1]
                .metrics
                .as_ref()
                .and_then(|metrics| metrics.prompt_tokens),
            Some(1200)
        );
        assert!(agent_steps[2].metrics.is_none());
        assert_eq!(
            trajectory
                .final_metrics
                .as_ref()
                .unwrap()
                .total_prompt_tokens,
            Some(1200)
        );
    }

    /// Validate-first: the run's final reply already validated, so no
    /// extraction request was sent. The answering turn is the only agent step;
    /// the success adds no extraction step, metered or not.
    #[test]
    fn a_final_reply_structured_output_adds_no_extraction_step() {
        let id = SessionId::new();
        let reply = r#"{"comments":[]}"#;
        let events = [
            AgentEvent::RunStarted {
                session_id: id.clone(),
                input: RunInput::Content {
                    content: ContentInput::Text("review this".into()),
                },
            },
            AgentEvent::TurnStarted { turn_number: 0 },
            AgentEvent::TextComplete {
                content: reply.into(),
            },
            AgentEvent::TurnCompleted {
                stop_reason: meerkat_core::StopReason::EndTurn,
                usage: Some(openai_usage(1200, 20, 1000, 4)),
            },
            AgentEvent::RunCompleted {
                session_id: id.clone(),
                result: reply.into(),
                structured_output: None,
                extraction_required: true,
                usage: Usage::default().into(),
                terminal_cause_kind: None,
            },
            AgentEvent::ExtractionSucceeded {
                session_id: id.clone(),
                structured_output: serde_json::json!({"comments": []}),
                schema_warnings: None,
                request_usage: Vec::new(),
                origin: meerkat_core::StructuredOutputOrigin::FinalReply,
            },
        ]
        .into_iter()
        .enumerate()
        .map(|(index, event)| envelope(&id, index as u64 + 1, event))
        .collect::<Vec<_>>();
        let trajectory = trajectory_from_events(&events, test_agent()).unwrap();
        let agent_steps = trajectory
            .steps
            .iter()
            .filter(|step| step.source == StepSource::Agent)
            .collect::<Vec<_>>();
        assert_eq!(
            agent_steps.len(),
            1,
            "only the answering turn made a request"
        );
        assert_eq!(agent_steps[0].message, AtifContent::Text(reply.into()));
        assert!(
            agent_steps[0]
                .extra
                .as_ref()
                .and_then(|extra| extra.get("request_kind"))
                .is_none(),
            "the answering turn is not an extraction step"
        );
        let totals = trajectory.final_metrics.as_ref().unwrap();
        assert_eq!(totals.total_steps, 2, "the user input and the answer");
        assert_eq!(totals.total_prompt_tokens, Some(1200));
    }

    /// A failed extraction keeps the failure on the step of its last request.
    #[test]
    fn failed_extraction_requests_are_steps_with_the_failure_on_the_last() {
        let id = SessionId::new();
        let events = vec![
            envelope(&id, 1, AgentEvent::TurnStarted { turn_number: 0 }),
            envelope(
                &id,
                2,
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(openai_usage(100, 5, 0, 0)),
                },
            ),
            envelope(
                &id,
                3,
                AgentEvent::RunCompleted {
                    session_id: id.clone(),
                    result: "answer".into(),
                    structured_output: None,
                    extraction_required: true,
                    usage: Usage::default().into(),
                    terminal_cause_kind: None,
                },
            ),
            envelope(
                &id,
                4,
                AgentEvent::ExtractionFailed {
                    session_id: id.clone(),
                    last_output: "answer".into(),
                    attempts: 2,
                    reason: "schema mismatch".into(),
                    request_usage: vec![openai_usage(110, 6, 100, 0), openai_usage(120, 7, 110, 0)],
                },
            ),
        ];
        let trajectory = trajectory_from_events(&events, test_agent()).unwrap();
        assert_eq!(trajectory.steps.len(), 3);
        assert!(
            trajectory.steps[1]
                .extra
                .as_ref()
                .unwrap()
                .get("failure_detail")
                .is_none()
        );
        assert!(
            trajectory.steps[2]
                .extra
                .as_ref()
                .unwrap()
                .get("failure_detail")
                .is_some()
        );
        assert_eq!(
            trajectory
                .final_metrics
                .as_ref()
                .unwrap()
                .total_prompt_tokens,
            Some(100 + 110 + 120)
        );
        assert_eq!(
            trajectory.extra.as_ref().unwrap().get("terminal_status"),
            Some(&Value::String("failed".into()))
        );
    }

    fn envelopes(id: &SessionId, events: Vec<AgentEvent>) -> Vec<EventEnvelope<AgentEvent>> {
        events
            .into_iter()
            .enumerate()
            .map(|(index, event)| envelope(id, index as u64 + 1, event))
            .collect()
    }

    fn run_started(id: &SessionId, text: &str) -> AgentEvent {
        AgentEvent::RunStarted {
            session_id: id.clone(),
            input: RunInput::Content {
                content: ContentInput::Text(text.into()),
            },
        }
    }

    fn run_completed(id: &SessionId, result: &str, extraction_required: bool) -> AgentEvent {
        AgentEvent::RunCompleted {
            session_id: id.clone(),
            result: result.into(),
            structured_output: None,
            extraction_required,
            usage: Usage::default().into(),
            terminal_cause_kind: None,
        }
    }

    fn agent_steps(trajectory: &Trajectory) -> Vec<&Step> {
        trajectory
            .steps
            .iter()
            .filter(|step| step.source == StepSource::Agent)
            .collect()
    }

    /// A compaction boundary sends the loop back to rebuild its request
    /// without advancing the turn counter, so the same turn is announced twice
    /// (the sequence captured from the real loop with a curator compactor:
    /// `turn_started`, `compaction_started`, `compaction_completed`,
    /// `turn_started`, ...). The second announcement replaces the first
    /// instead of leaving an empty, unmetered step behind that claims an LLM
    /// call nobody made. The retry path's repoll can follow an attempt that
    /// streamed partial text before it failed; that text is superseded too.
    #[test]
    fn a_compaction_repoll_that_reannounces_the_turn_adds_no_step() {
        let id = SessionId::new();
        for first_attempt_streamed in [false, true] {
            let mut events = vec![
                run_started(&id, "summarize"),
                AgentEvent::TurnStarted { turn_number: 0 },
            ];
            if first_attempt_streamed {
                events.push(AgentEvent::TextDelta {
                    delta: "partial".into(),
                });
            }
            events.extend([
                AgentEvent::CompactionStarted {
                    input_tokens: 90_000,
                    estimated_history_tokens: 95_000,
                    message_count: 40,
                },
                AgentEvent::CompactionCompleted {
                    summary_tokens: 800,
                    messages_before: 40,
                    messages_after: 3,
                },
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextDelta {
                    delta: "answer".into(),
                },
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(openai_usage(3000, 40, 0, 0)),
                },
                run_completed(&id, "answer", false),
            ]);
            let trajectory = trajectory_from_events(&envelopes(&id, events), test_agent()).unwrap();
            let steps = agent_steps(&trajectory);
            assert_eq!(
                steps.len(),
                1,
                "one provider request answered (first attempt streamed: {first_attempt_streamed})"
            );
            assert_eq!(steps[0].message, AtifContent::Text("answer".into()));
            assert_eq!(
                steps[0].metrics.as_ref().and_then(|m| m.prompt_tokens),
                Some(3000)
            );
            let totals = trajectory.final_metrics.as_ref().unwrap();
            assert_eq!(totals.total_steps, 2, "the user step and the answer");
            assert_eq!(totals.total_prompt_tokens, Some(3000));
        }
    }

    /// A tool-loop turn that published no completion is still a step when the
    /// next turn has a new number, and a turn announced without any output
    /// before the next turn is not one.
    #[test]
    fn an_unfinished_turn_is_a_step_only_when_it_recorded_output() {
        let id = SessionId::new();
        let events = vec![
            run_started(&id, "go"),
            AgentEvent::TurnStarted { turn_number: 0 },
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::ReasoningComplete {
                content: "look first".into(),
            },
            AgentEvent::TurnStarted { turn_number: 2 },
            AgentEvent::TextComplete {
                content: "done".into(),
            },
            run_completed(&id, "done", false),
        ];
        let trajectory = trajectory_from_events(&envelopes(&id, events), test_agent()).unwrap();
        let steps = agent_steps(&trajectory);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].reasoning_content.as_deref(), Some("look first"));
        assert_eq!(steps[1].message, AtifContent::Text("done".into()));
    }

    /// Each run restarts the loop's turn counter. A turn left pending by a run
    /// that ended without a terminal event is exported before the next run's
    /// input, and the next run's first `TurnStarted` (same number, new run) is
    /// not mistaken for a re-announcement of it.
    #[test]
    fn a_turn_left_pending_by_an_interrupted_run_precedes_the_next_run() {
        let id = SessionId::new();
        let events = vec![
            run_started(&id, "first"),
            AgentEvent::TurnStarted { turn_number: 0 },
            AgentEvent::ToolCallRequested {
                id: "call-1".into(),
                name: "shell".into(),
                args: meerkat_core::event::ToolCallArguments::from_value(
                    serde_json::json!({"command": "ls"}),
                )
                .unwrap(),
            },
            run_started(&id, "second"),
            AgentEvent::TurnStarted { turn_number: 0 },
            AgentEvent::TextComplete {
                content: "second answer".into(),
            },
            AgentEvent::TurnCompleted {
                stop_reason: meerkat_core::StopReason::EndTurn,
                usage: Some(openai_usage(500, 5, 0, 0)),
            },
            run_completed(&id, "second answer", false),
        ];
        let trajectory = trajectory_from_events(&envelopes(&id, events), test_agent()).unwrap();
        let sources = trajectory
            .steps
            .iter()
            .map(|step| step.source)
            .collect::<Vec<_>>();
        assert_eq!(
            sources,
            vec![
                StepSource::User,
                StepSource::Agent,
                StepSource::User,
                StepSource::Agent
            ]
        );
        assert_eq!(trajectory.steps[1].tool_calls[0].function_name, "shell");
        assert!(trajectory.steps[1].metrics.is_none());
        assert_eq!(
            trajectory.steps[3].message,
            AtifContent::Text("second answer".into())
        );
    }

    /// An extraction phase that ends without an outcome event (the run fails
    /// inside it, or the log simply moves on to the next run) must not keep
    /// swallowing text and reasoning: the next run's answer is exported.
    #[test]
    fn an_extraction_phase_without_an_outcome_does_not_hide_later_runs() {
        let id = SessionId::new();
        for fails_inside_extraction in [true, false] {
            let mut events = vec![
                run_started(&id, "first"),
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "first answer".into(),
                },
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(openai_usage(100, 5, 0, 0)),
                },
                run_completed(&id, "first answer", true),
                AgentEvent::TextDelta {
                    delta: r#"{"partial": "#.into(),
                },
            ];
            if fails_inside_extraction {
                events.push(AgentEvent::RunFailed {
                    session_id: id.clone(),
                    error_report: meerkat_core::AgentErrorReport::from_agent_error(
                        &meerkat_core::AgentError::InternalError(
                            "max tokens reached during extraction".into(),
                        ),
                    ),
                    terminal_cause_kind: None,
                });
            }
            events.extend([
                run_started(&id, "second"),
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::ReasoningComplete {
                    content: "thinking".into(),
                },
                AgentEvent::TextComplete {
                    content: "second answer".into(),
                },
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(openai_usage(200, 6, 0, 0)),
                },
            ]);
            let trajectory = trajectory_from_events(&envelopes(&id, events), test_agent()).unwrap();
            let steps = agent_steps(&trajectory);
            assert_eq!(
                steps.len(),
                2,
                "the extraction deltas are not a step (run failed: {fails_inside_extraction})"
            );
            let last = steps[1];
            assert_eq!(last.message, AtifContent::Text("second answer".into()));
            assert_eq!(last.reasoning_content.as_deref(), Some("thinking"));
        }
    }

    /// Embedded member trajectories get the document identity ATIF refs use.
    #[test]
    fn subagent_trajectories_receive_document_identities() {
        let parent = TrajectoryBuilder::new()
            .with_session_id("parent")
            .finish(test_agent());
        let member = TrajectoryBuilder::new()
            .with_session_id("member")
            .finish(test_agent());
        let named = Trajectory {
            trajectory_id: Some("explicit".into()),
            ..TrajectoryBuilder::new()
                .with_session_id("named-member")
                .finish(test_agent())
        };
        let embedded = parent.with_subagent_trajectories(vec![member, named]);
        assert_eq!(
            embedded.subagent_trajectories[0].trajectory_id.as_deref(),
            Some("subagent-1")
        );
        assert_eq!(
            embedded.subagent_trajectories[1].trajectory_id.as_deref(),
            Some("explicit")
        );
    }
}
