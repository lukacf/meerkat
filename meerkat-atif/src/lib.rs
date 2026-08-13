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
    pub extra: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Agent {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
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
    let mut steps = Vec::new();
    let mut pending = None;
    let mut session_id: Option<String> = None;
    let mut totals = FinalMetrics::default();
    let mut terminal_status: Option<&'static str> = None;
    for envelope in events {
        match &envelope.payload {
            AgentEvent::RunStarted {
                session_id: id,
                input,
            } => {
                session_id = Some(id.to_string());
                if let RunInput::Content { content } = input {
                    steps.push(Step {
                        step_id: next_id(&steps),
                        timestamp: timestamp(envelope.timestamp_ms),
                        source: StepSource::User,
                        model_name: None,
                        message: atif_content(content),
                        reasoning_content: None,
                        tool_calls: Vec::new(),
                        observation: None,
                        metrics: None,
                        llm_call_count: None,
                    });
                }
            }
            AgentEvent::TurnStarted { .. } => {
                pending = Some(PendingTurn::new(envelope.timestamp_ms));
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
                }),
            AgentEvent::TurnCompleted { usage, .. } => {
                if let Some(turn) = pending.take() {
                    append_agent_step(&mut steps, turn, Some(usage));
                    add_totals(&mut totals, usage);
                }
            }
            AgentEvent::RunCompleted { result, .. } => {
                terminal_status = Some("completed");
                if let Some(mut turn) = pending.take() {
                    if turn.message.is_empty() {
                        turn.message.clone_from(result);
                    }
                    append_agent_step(&mut steps, turn, None);
                }
            }
            AgentEvent::RunFailed { .. } | AgentEvent::ExtractionFailed { .. } => {
                terminal_status = Some("failed");
            }
            _ => {}
        }
    }
    totals.total_steps = steps.len() as u64;
    let extra = terminal_status.map(|status| {
        Map::from_iter([(
            String::from("terminal_status"),
            Value::String(status.to_string()),
        )])
    });
    Ok(Trajectory {
        schema_version: SCHEMA_VERSION.to_string(),
        session_id,
        trajectory_id: None,
        agent,
        steps,
        final_metrics: Some(totals),
        subagent_trajectories: Vec::new(),
        extra,
    })
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
        }),
        llm_call_count: Some(1),
    });
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
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(ContentPart::Text { text: text.clone() }),
                ContentBlock::Image { media_type, data } => Some(ContentPart::Image {
                    source: ImageSource {
                        media_type: media_type.clone(),
                        path: match data {
                            ImageData::Inline { data } => {
                                format!("data:{media_type};base64,{data}")
                            }
                            ImageData::Blob { blob_id } => format!("blob:{blob_id}"),
                        },
                    },
                }),
                _ => None,
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
                    usage: TurnUsage::new(
                        Usage {
                            input_tokens: 4,
                            output_tokens: 2,
                            cache_creation_tokens: None,
                            cache_read_tokens: Some(1),
                            provider_accounting: None,
                        },
                        meerkat_core::ProviderTokenAccounting::openai("test", 4),
                    ),
                },
            ),
        ];
        let trajectory = trajectory_from_events(
            &events,
            Agent {
                name: "meerkat".into(),
                version: "0.8".into(),
                model_name: None,
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
}
