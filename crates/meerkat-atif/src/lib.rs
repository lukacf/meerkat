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
    timestamp: Option<String>,
    message: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    observations: Vec<ObservationResult>,
}

impl PendingTurn {
    fn new(timestamp_ms: u64) -> Self {
        Self {
            timestamp: timestamp(timestamp_ms),
            message: String::new(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
            observations: Vec::new(),
        }
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
        match &envelope.payload {
            AgentEvent::RunStarted {
                session_id: id,
                input,
            } => {
                self.session_id = Some(id.to_string());
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
            AgentEvent::TurnStarted { .. } => {
                *pending = Some(PendingTurn::new(envelope.timestamp_ms));
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
            AgentEvent::RunCompleted { result, .. } => {
                self.terminal_status = Some("completed");
                if let Some(mut turn) = pending.take() {
                    if turn.message.is_empty() {
                        turn.message.clone_from(result);
                    }
                    append_agent_step(steps, turn, None);
                }
            }
            AgentEvent::RunFailed { error_report, .. } => {
                self.terminal_status = Some("failed");
                self.failure_detail = Some(error_report.message.clone());
                flush_failed_turn(steps, pending, error_report.message.clone());
            }
            AgentEvent::ExtractionFailed {
                last_output,
                reason,
                ..
            } => {
                self.terminal_status = Some("failed");
                let detail = format!("{reason}; last_output={last_output}");
                self.failure_detail = Some(detail.clone());
                flush_failed_turn(steps, pending, detail);
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
        model_name: None,
        message: AtifContent::Text(turn.message),
        reasoning_content: (!turn.reasoning.is_empty()).then_some(turn.reasoning),
        tool_calls: turn.tool_calls,
        observation: (!turn.observations.is_empty()).then_some(Observation {
            results: turn.observations,
        }),
        metrics: usage.map(|usage| Metrics {
            prompt_tokens: Some(usage.presented_tokens()),
            completion_tokens: Some(usage.output_tokens),
            cached_tokens: usage.cache_read_tokens,
            cost_usd: None,
            logprobs: None,
            prompt_token_ids: None,
            completion_token_ids: None,
            extra: None,
        }),
        llm_call_count: Some(1),
        extra: None,
        reasoning_effort: None,
        is_copied_context: None,
    });
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
                .saturating_add(cached),
        );
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
