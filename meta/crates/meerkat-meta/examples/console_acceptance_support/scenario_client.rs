//! Scripted model choices over the real runtime tool dispatcher.
//!
//! This client records requests and emits tool calls. Only the runtime creates
//! tool results, WorkGraph revisions, peer receipts, and image artifacts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{Message, Provider, StopReason, TurnUsage, Usage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Notify;

/// Opt-in ordering control for a real peer turn. It only blocks the scripted
/// model; delivery, tool execution, and console projection stay runtime-owned.
#[derive(Default)]
pub struct ModelBarrier {
    current: Mutex<Option<Arc<BarrierRun>>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BarrierPlan {
    id: String,
    match_text: String,
    source: String,
    /// Hold a later model request only after this exact native tool result.
    /// Existing peer barriers omit this and retain their workgraph handshake.
    #[serde(default)]
    after_tool_result: Option<String>,
}

struct BarrierRun {
    plan: BarrierPlan,
    progress: Mutex<BarrierProgress>,
    release: Notify,
}

#[derive(Default)]
struct BarrierProgress {
    entered: bool,
    released: bool,
    requests: usize,
    waiting_requests: usize,
}

struct BarrierWaitGuard<'a>(&'a BarrierRun);

impl Drop for BarrierWaitGuard<'_> {
    fn drop(&mut self) {
        self.0
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .waiting_requests -= 1;
    }
}

impl ModelBarrier {
    pub fn control(&self, command: &Value) -> Result<Value, String> {
        match command.get("action").and_then(Value::as_str) {
            Some("arm") => {
                let plan: BarrierPlan = serde_json::from_value(
                    command.get("plan").cloned().ok_or("missing barrier plan")?,
                )
                .map_err(|error| error.to_string())?;
                if plan.id.is_empty()
                    || plan.id.len() > 48
                    || !plan
                        .id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                    || plan.match_text.is_empty()
                    || plan.source.is_empty()
                    || plan
                        .after_tool_result
                        .as_ref()
                        .is_some_and(String::is_empty)
                {
                    return Err("barrier needs a valid id, nonempty match_text and source".into());
                }
                let mut current = self
                    .current
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if current.is_some() {
                    return Err("this fixture already has an armed barrier".into());
                }
                *current = Some(Arc::new(BarrierRun {
                    plan,
                    progress: Mutex::new(BarrierProgress::default()),
                    release: Notify::new(),
                }));
                Ok(current.as_ref().unwrap().snapshot())
            }
            Some("status" | "release") => {
                let current = self
                    .current
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let run = current.as_ref().ok_or("no barrier is armed")?;
                if command.get("id").and_then(Value::as_str) != Some(run.plan.id.as_str()) {
                    return Err("barrier id does not match".into());
                }
                if command["action"] == "release" {
                    run.progress
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .released = true;
                    run.release.notify_waiters();
                }
                Ok(run.snapshot())
            }
            _ => Err("barrier action must be arm, status or release".into()),
        }
    }

    fn matching_request(&self, messages: &[Message]) -> Option<(Arc<BarrierRun>, usize)> {
        let (start, input) = messages
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, message)| barrier_input_text(message).map(|text| (index, text)))?;
        let current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let run = current.as_ref()?;
        if !input.contains(&run.plan.match_text) {
            return None;
        }
        if let Some(expected) = &run.plan.after_tool_result {
            let results = messages[start..].iter().flat_map(|message| match message {
                Message::ToolResults { results, .. } => results.as_slice(),
                _ => &[],
            });
            let mut matching = results.filter(|result| result.tool_use_id == *expected);
            let result = matching.next()?;
            if result.is_error || matching.next().is_some() {
                return None;
            }
        }
        Some((run.clone(), start))
    }
}

// Incoming comms are typed notices in the real runtime transcript. A newer
// operator or peer input closes the previous boundary, including when its text
// is empty, so an old marker cannot capture an unrelated turn or continuation.
fn barrier_input_text(message: &Message) -> Option<String> {
    match message {
        Message::User(user) if user.transcript_role.is_conversational() => {
            Some(user.text_content())
        }
        Message::SystemNotice(notice) => {
            let mut input = None;
            for block in &notice.blocks {
                if let meerkat_core::SystemNoticeBlock::Comms {
                    direction: meerkat_core::SystemNoticeDirection::Incoming,
                    content,
                    ..
                } = block
                {
                    let text = input.get_or_insert_with(String::new);
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&meerkat_core::types::text_content(content));
                }
            }
            input
        }
        _ => None,
    }
}

impl BarrierRun {
    fn snapshot(&self) -> Value {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        json!({
            "id": self.plan.id, "match_text": self.plan.match_text,
            "phase": if progress.released { "released" } else if progress.entered { "entered" } else { "armed" },
            "entered": progress.entered, "released": progress.released, "requests": progress.requests,
            "waiting_requests": progress.waiting_requests,
        })
    }

    async fn wait_for_release(&self) {
        {
            let mut progress = self
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            progress.entered = true;
            progress.requests += 1;
            progress.waiting_requests += 1;
        }
        // The fixture must not mistake a cancelled model stream for a live
        // held request. Dropping the stream releases this observation too.
        let _waiting = BarrierWaitGuard(self);
        loop {
            let released = self.release.notified();
            tokio::pin!(released);
            // Register before reading the flag so release cannot be lost in
            // the gap between checking state and awaiting the notification.
            released.as_mut().enable();
            if self
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .released
            {
                return;
            }
            released.await;
        }
    }
}

fn barrier_turn(plan: &BarrierPlan, messages: &[Message]) -> Result<Turn, String> {
    if plan.after_tool_result.is_some() {
        return Ok(Turn::Text(plan.source.clone()));
    }
    let id = format!("fixture-{}-peer-ready", plan.id);
    for message in messages.iter().rev() {
        if let Message::ToolResults { results, .. } = message
            && let Some(result) = results.iter().find(|result| result.tool_use_id == id)
        {
            let value: Value =
                serde_json::from_str(&result.text_content()).map_err(|error| error.to_string())?;
            if result.is_error || !value.get("items").is_some_and(Value::is_array) {
                return Err(format!("barrier peer ready tool failed: {value}"));
            }
            return Ok(Turn::Text(plan.source.clone()));
        }
    }
    if messages.iter().any(|message| matches!(message, Message::BlockAssistant(assistant) if assistant.tool_calls().any(|call| call.id == id))) {
        return Err("barrier peer ready tool has no runtime result".into());
    }
    Ok(Turn::Tools(vec![PlannedCall {
        id,
        name: "workgraph_ready",
        args: json!({ "labels": [format!("fixture-{}", plan.id)] }),
    }]))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ModelPlan {
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_blocks: Vec<String>,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default = "default_chunk_chars")]
    pub chunk_chars: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<ScenarioPlan>,
}

fn default_chunk_chars() -> usize {
    32
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioKind {
    Workgraph,
    WorkgraphEvents,
    Peer,
    Image,
    Routine,
    AssistantIdentity,
    McpApps,
    McpAppsPolling,
}

/// A unique run id also supplies the explicit trigger in the user message:
/// `[fixture:RUN_ID]`. Ordinary and incoming peer turns never trigger a script.
/// The host must keep this plan stable until that interaction finishes.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioPlan {
    pub kind: ScenarioKind,
    pub run_id: String,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub peer_id: Option<String>,
    #[serde(default)]
    pub peer_body: Option<String>,
    #[serde(default)]
    pub image_provider: Option<String>,
    #[serde(default)]
    pub image_prompt: Option<String>,
}

impl ScenarioPlan {
    pub fn trigger(&self) -> String {
        format!("[fixture:{}]", self.run_id)
    }

    fn call_id(&self, step: &str) -> String {
        format!("fixture-{}-{step}", self.run_id)
    }

    fn label(&self) -> String {
        format!("fixture-{}", self.run_id)
    }

    fn validate(&self) -> Result<(), String> {
        if self.run_id.is_empty()
            || self.run_id.len() > 48
            || !self
                .run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return Err(
                "run_id must have 1-48 ASCII letters, digits, underscores, or hyphens".into(),
            );
        }
        Ok(())
    }
}

pub struct RecordingClient {
    pub plan: Arc<Mutex<ModelPlan>>,
    pub requests: Arc<Mutex<Vec<Value>>>,
    pub barrier: Arc<ModelBarrier>,
}

impl RecordingClient {
    pub fn new(
        plan: Arc<Mutex<ModelPlan>>,
        requests: Arc<Mutex<Vec<Value>>>,
        barrier: Arc<ModelBarrier>,
    ) -> Self {
        Self {
            plan,
            requests,
            barrier,
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for RecordingClient {
    fn provider(&self) -> Provider {
        Provider::Other
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(serde_json::to_value(request).unwrap_or(Value::Null));
        let plan = self
            .plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let barrier = self.barrier.matching_request(&request.messages);
        let turn = if let Some((run, start)) = &barrier {
            barrier_turn(&run.plan, &request.messages[*start..])
                .unwrap_or_else(|error| Turn::Text(format!("Acceptance scenario stopped: {error}")))
        } else if matches!(request.messages.last(), Some(Message::User(user))
            if user.text_content().contains("You have been spawned as"))
        {
            // The runtime retries an empty successful response. Bootstrap must
            // finish with visible output before the member can accept work.
            Turn::Text("Ready.".into())
        } else if let Some((scenario, active)) = plan.scenario.as_ref().and_then(|scenario| {
            active_messages(scenario, &request.messages).map(|active| (scenario, active))
        }) {
            scenario_turn(scenario, active)
                .unwrap_or_else(|error| Turn::Text(format!("Acceptance scenario stopped: {error}")))
        } else {
            Turn::Text(plan.source.clone())
        };
        let provider = meerkat_models::infer_provider(&request.model).unwrap_or(self.provider());
        let usage = TurnUsage::host_declared(provider, &request.model, Usage::default());
        Box::pin(async_stream::stream! {
            if let Some((barrier, _)) = barrier {
                barrier.wait_for_release().await;
            }
            let (text, calls) = match turn {
                Turn::Text(text) => (Some(text), Vec::new()),
                Turn::Tools(calls) => (None, calls),
                Turn::TextAndTools { text, calls } => (Some(text), calls),
            };
            if let Some(text) = text {
                for reasoning in &plan.reasoning_blocks {
                    let chars: Vec<char> = reasoning.chars().collect();
                    for chunk in chars.chunks(plan.chunk_chars.clamp(1, 4096)) {
                        if plan.delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(plan.delay_ms.min(1000))).await;
                        }
                        yield Ok(LlmEvent::ReasoningDelta { delta: chunk.iter().collect() });
                    }
                    yield Ok(LlmEvent::ReasoningComplete { text: reasoning.clone(), meta: None });
                }
                let chars: Vec<char> = text.chars().collect();
                for chunk in chars.chunks(plan.chunk_chars.clamp(1, 4096)) {
                    if plan.delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(plan.delay_ms.min(1000))).await;
                    }
                    yield Ok(LlmEvent::TextDelta { delta: chunk.iter().collect(), meta: None });
                }
            }
            let stop_reason = if calls.is_empty() { StopReason::EndTurn } else { StopReason::ToolUse };
            for call in calls {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: call.id, name: call.name.into(), args: call.args, meta: None,
                });
            }
            yield Ok(LlmEvent::UsageUpdate { usage });
            yield Ok(LlmEvent::Done { outcome: LlmDoneOutcome::success(stop_reason)});
        })
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[derive(Debug)]
enum Turn {
    Text(String),
    Tools(Vec<PlannedCall>),
    TextAndTools {
        text: String,
        calls: Vec<PlannedCall>,
    },
}

#[derive(Debug)]
struct PlannedCall {
    id: String,
    name: &'static str,
    args: Value,
}

fn active_messages<'a>(scenario: &ScenarioPlan, messages: &'a [Message]) -> Option<&'a [Message]> {
    let start = messages.iter().rposition(|message| {
        matches!(message, Message::User(user) if user.transcript_role.is_conversational())
    })?;
    let Message::User(user) = &messages[start] else {
        return None;
    };
    // Prefix matching prevents a peer quoting a trigger later in its body from
    // accidentally becoming the operator's scripted request.
    user.text_content()
        .trim_start()
        .starts_with(&scenario.trigger())
        .then_some(&messages[start..])
}

struct RecordedTurn<'a> {
    scenario: &'a ScenarioPlan,
    messages: &'a [Message],
}

impl RecordedTurn<'_> {
    fn result(&self, step: &str) -> Result<Option<Value>, String> {
        let id = self.scenario.call_id(step);
        for message in self.messages.iter().rev() {
            if let Message::ToolResults { results, .. } = message
                && let Some(result) = results.iter().find(|result| result.tool_use_id == id)
            {
                let text = result.text_content();
                if result.is_error {
                    return Err(format!("{step}: {text}"));
                }
                let value: Value = serde_json::from_str(&text)
                    .map_err(|error| format!("{step} returned invalid JSON: {error}; {text}"))?;
                if !value.is_object() || value.get("error").is_some_and(|error| !error.is_null()) {
                    return Err(format!("{step} returned an unexpected result: {value}"));
                }
                return Ok(Some(value));
            }
        }
        if self.messages.iter().any(|message| {
            matches!(message,
            Message::BlockAssistant(assistant) if assistant.tool_calls().any(|call| call.id == id))
        }) {
            return Err(format!(
                "{step} was already requested without a recorded result; refusing to repeat it"
            ));
        }
        Ok(None)
    }

    fn call(&self, step: &str, name: &'static str, args: Value) -> PlannedCall {
        PlannedCall {
            id: self.scenario.call_id(step),
            name,
            args,
        }
    }

    fn item(&self, step: &str, expected_id: Option<&str>) -> Result<ObservedItem, String> {
        let value = self
            .result(step)?
            .ok_or_else(|| format!("{step} has no recorded result"))?;
        let item = value
            .get("item")
            .ok_or_else(|| format!("{step} has no item: {value}"))?;
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| format!("{step} has no item id: {value}"))?;
        if expected_id.is_some_and(|expected| expected != id) {
            return Err(format!("{step} returned another item: {id}"));
        }
        Ok(ObservedItem {
            id: id.to_owned(),
            revision: item
                .get("revision")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("{step} has no revision: {value}"))?,
            status: item
                .get("status")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{step} has no status: {value}"))?
                .to_owned(),
        })
    }

    fn pending(&self, specs: &[(&str, &'static str, Value)]) -> Result<Option<Turn>, String> {
        let mut calls = Vec::new();
        for (step, name, args) in specs {
            if self.result(step)?.is_none() {
                calls.push(self.call(step, name, args.clone()));
            }
        }
        Ok((!calls.is_empty()).then_some(Turn::Tools(calls)))
    }
}

struct ObservedItem {
    id: String,
    revision: u64,
    status: String,
}

fn required<'a>(value: &'a Option<String>, field: &str) -> Result<&'a str, String> {
    value
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("host must configure {field}"))
}

fn scenario_turn(scenario: &ScenarioPlan, messages: &[Message]) -> Result<Turn, String> {
    scenario.validate()?;
    let recorded = RecordedTurn { scenario, messages };
    match scenario.kind {
        ScenarioKind::Workgraph => workgraph_turn(&recorded),
        ScenarioKind::WorkgraphEvents => workgraph_events_turn(&recorded),
        ScenarioKind::Peer => peer_turn(&recorded),
        ScenarioKind::Image => image_turn(&recorded),
        ScenarioKind::Routine => routine_turn(&recorded),
        ScenarioKind::AssistantIdentity => assistant_identity_turn(&recorded),
        ScenarioKind::McpApps => mcp_apps_turn(&recorded),
        ScenarioKind::McpAppsPolling => mcp_apps_polling_turn(&recorded),
    }
}

fn mcp_apps_turn(recorded: &RecordedTurn<'_>) -> Result<Turn, String> {
    mcp_apps_views_turn(recorded, &["display"])
}

fn mcp_apps_polling_turn(recorded: &RecordedTurn<'_>) -> Result<Turn, String> {
    mcp_apps_views_turn(recorded, &["display", "display-two", "display-three"])
}

fn mcp_apps_views_turn(recorded: &RecordedTurn<'_>, views: &[&str]) -> Result<Turn, String> {
    let Some(loaded) = recorded.result("load-display")? else {
        return Ok(Turn::Tools(vec![recorded.call(
            "load-display",
            "tool_catalog_load",
            json!({"names": ["display"]}),
        )]));
    };
    let admitted = ["accepted_names", "noop_names"].iter().any(|field| {
        loaded
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|names| names.iter().any(|name| name == "display"))
    });
    if loaded.get("catalog_exact") != Some(&Value::Bool(true)) || !admitted {
        return Err(format!("native catalog did not stage display: {loaded}"));
    }
    for view in views {
        if recorded.result(view)?.is_none() {
            let arguments = if views.len() == 1 {
                json!({"query": "records"})
            } else {
                json!({"query": "records", "viewId": view})
            };
            return Ok(Turn::Tools(vec![recorded.call(view, "display", arguments)]));
        }
    }
    Ok(Turn::Text("The MCP App is ready.".into()))
}

const ASSISTANT_IDENTITY_SOURCE: &str = "## Release evidence\n\nKeep A\u{030a}, å and 🚀 exactly.\n\n| Check | Result |\n| --- | --- |\n| WorkGraph | Reviewed through the runtime |";

fn assistant_identity_turn(recorded: &RecordedTurn<'_>) -> Result<Turn, String> {
    for step in ["identity-first", "identity-second"] {
        match recorded.result(step)? {
            Some(value) if value.get("items").is_some_and(Value::is_array) => {}
            Some(_) => return Err(format!("{step}: runtime ready result has no items array")),
            None => {
                return Ok(Turn::TextAndTools {
                    text: ASSISTANT_IDENTITY_SOURCE.into(),
                    calls: vec![recorded.call(
                        step,
                        "workgraph_ready",
                        json!({"labels": [recorded.scenario.label()]}),
                    )],
                });
            }
        }
    }
    Ok(Turn::Text(ASSISTANT_IDENTITY_SOURCE.into()))
}

fn routine_turn(recorded: &RecordedTurn<'_>) -> Result<Turn, String> {
    let steps = [
        (
            "notes",
            "read_file",
            json!({"path":"release-notes.txt"}),
            false,
        ),
        ("files", "list_files", json!({}), false),
        ("missing", "read_file", json!({"path":"missing.txt"}), true),
        (
            "late",
            "read_file",
            json!({"path":"late-review.txt"}),
            false,
        ),
        ("ready", "workgraph_ready", json!({}), false),
    ];
    for (index, (step, _, _, expected_error)) in steps.iter().enumerate() {
        let id = recorded.scenario.call_id(step);
        let result = recorded
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::ToolResults { results, .. } => {
                    results.iter().find(|result| result.tool_use_id == id)
                }
                _ => None,
            });
        if let Some(result) = result {
            if result.is_error != *expected_error {
                return Err(format!("{step}: unexpected runtime outcome"));
            }
            continue;
        }
        if recorded.messages.iter().any(|message| matches!(message, Message::BlockAssistant(assistant) if assistant.tool_calls().any(|call| call.id == id))) {
            return Err(format!("{step}: requested tool has no runtime result"));
        }
        if index < 2 {
            if index != 0 {
                return Err("initial routine tool batch has incomplete results".into());
            }
            return Ok(Turn::Tools(
                steps[..2]
                    .iter()
                    .map(|(step, name, args, _)| recorded.call(step, name, args.clone()))
                    .collect(),
            ));
        }
        return Ok(Turn::Tools(vec![recorded.call(
            step,
            steps[index].1,
            steps[index].2.clone(),
        )]));
    }
    Ok(Turn::Text("Routine workspace review complete. Two initial reads succeeded, the missing file stayed visible, and the delayed review arrived intact.".into()))
}

fn workgraph_turn(recorded: &RecordedTurn<'_>) -> Result<Turn, String> {
    let scenario = recorded.scenario;
    let owner_id = required(
        &scenario.owner_id,
        "owner_id (canonical WorkGraph agent owner)",
    )?;
    let create = |title: &str, description: &str| {
        json!({
            "title": title, "description": description, "labels": [scenario.label()],
            "completion_policy": {"kind": "self_attest"},
        })
    };
    if let Some(turn) = recorded.pending(&[
        ("create-review", "workgraph_create", create("Review source change", "Inspect the source change and uploaded release diagram.")),
        ("create-render", "workgraph_create", create("Review release badge", "Inspect the release badge before publishing.")),
        ("create-publish", "workgraph_create", create("Publish release candidate", "Both review prerequisites must complete first. This fixture item does not deploy anything.")),
    ])? { return Ok(turn); }

    let review = recorded.item("create-review", None)?;
    let render = recorded.item("create-render", None)?;
    let publish = recorded.item("create-publish", None)?;
    if review.id == render.id || review.id == publish.id || render.id == publish.id {
        return Err("create returned duplicate item ids".into());
    }
    if let Some(turn) = recorded.pending(&[
        (
            "link-review",
            "workgraph_link",
            json!({"kind": "blocks", "from_id": review.id, "to_id": publish.id}),
        ),
        (
            "link-render",
            "workgraph_link",
            json!({"kind": "blocks", "from_id": render.id, "to_id": publish.id}),
        ),
    ])? {
        return Ok(turn);
    }
    if let Some(turn) = recorded.pending(&[
        (
            "ready-before",
            "workgraph_ready",
            json!({"labels": [scenario.label()]}),
        ),
        ("get-review", "workgraph_get", json!({"id": review.id})),
        ("get-render", "workgraph_get", json!({"id": render.id})),
    ])? {
        return Ok(turn);
    }
    let ready_before = recorded
        .result("ready-before")?
        .ok_or("missing ready-before result")?;
    let ready = item_ids(&ready_before)?;
    if ready.contains(&publish.id.as_str()) {
        return Err("dependent item was ready before its prerequisites completed".into());
    }
    if !ready.contains(&review.id.as_str()) || !ready.contains(&render.id.as_str()) {
        return Err("unclaimed prerequisite items were absent from the ready result".into());
    }
    let current_review = recorded.item("get-review", Some(&review.id))?;
    let current_render = recorded.item("get-render", Some(&render.id))?;
    let claim = |item: &ObservedItem| {
        json!({
            "id": item.id, "expected_revision": item.revision,
            "owner": {"key": {"kind": "agent", "id": owner_id}}, "lease_seconds": 300,
        })
    };
    if let Some(turn) = recorded.pending(&[
        ("claim-review", "workgraph_claim", claim(&current_review)),
        ("claim-render", "workgraph_claim", claim(&current_render)),
    ])? {
        return Ok(turn);
    }
    let claimed_review = recorded.item("claim-review", Some(&review.id))?;
    let claimed_render = recorded.item("claim-render", Some(&render.id))?;
    for item in [&claimed_review, &claimed_render] {
        if item.status != "in_progress" {
            return Err(format!(
                "claim returned status {} for {}",
                item.status, item.id
            ));
        }
    }
    let close = |item: &ObservedItem| {
        json!({
            "id": item.id, "expected_revision": item.revision, "status": "completed",
        })
    };
    if let Some(turn) = recorded.pending(&[
        ("close-review", "workgraph_close", close(&claimed_review)),
        ("close-render", "workgraph_close", close(&claimed_render)),
    ])? {
        return Ok(turn);
    }
    for (step, id) in [("close-review", &review.id), ("close-render", &render.id)] {
        if recorded.item(step, Some(id))?.status != "completed" {
            return Err(format!("{step} did not complete its prerequisite"));
        }
    }
    if let Some(turn) = recorded.pending(&[
        (
            "ready-after",
            "workgraph_ready",
            json!({"labels": [scenario.label()]}),
        ),
        ("get-publish", "workgraph_get", json!({"id": publish.id})),
    ])? {
        return Ok(turn);
    }
    let ready_after = recorded
        .result("ready-after")?
        .ok_or("missing ready-after result")?;
    if !item_ids(&ready_after)?.contains(&publish.id.as_str()) {
        return Err("dependent item did not become ready after its prerequisites completed".into());
    }
    let current_publish = recorded.item("get-publish", Some(&publish.id))?;
    if let Some(turn) =
        recorded.pending(&[("claim-publish", "workgraph_claim", claim(&current_publish))])?
    {
        return Ok(turn);
    }
    let claimed_publish = recorded.item("claim-publish", Some(&publish.id))?;
    if claimed_publish.status != "in_progress" {
        return Err("dependent claim did not enter in_progress".into());
    }
    if let Some(turn) =
        recorded.pending(&[("close-publish", "workgraph_close", close(&claimed_publish))])?
    {
        return Ok(turn);
    }
    if recorded.item("close-publish", Some(&publish.id))?.status != "completed" {
        return Err("dependent item did not complete".into());
    }
    if let Some(turn) = recorded.pending(&[(
        "snapshot",
        "workgraph_snapshot",
        json!({
            "labels": [scenario.label()], "include_terminal": true,
        }),
    )])? {
        return Ok(turn);
    }
    let result = recorded
        .result("snapshot")?
        .ok_or("missing final snapshot")?;
    let snapshot = result
        .get("snapshot")
        .ok_or("final result has no snapshot")?;
    let items = snapshot
        .get("items")
        .and_then(Value::as_array)
        .ok_or("snapshot has no items")?;
    for item in [&review, &render, &publish] {
        if !items
            .iter()
            .any(|observed| observed["id"] == item.id && observed["status"] == "completed")
        {
            return Err(format!(
                "final snapshot does not confirm completed item {}",
                item.id
            ));
        }
    }
    let edges = snapshot
        .get("edges")
        .and_then(Value::as_array)
        .ok_or("snapshot has no edges")?;
    for prerequisite in [&review, &render] {
        if !edges.iter().any(|edge| {
            edge["kind"] == "blocks"
                && edge["from_id"] == prerequisite.id
                && edge["to_id"] == publish.id
        }) {
            return Err(format!(
                "final snapshot does not confirm dependency from {}",
                prerequisite.id
            ));
        }
    }
    Ok(Turn::Text(format!(
        "## WorkGraph scenario complete\n\nThe runtime snapshot confirms two prerequisites and their dependent item are completed. The dependent was absent from the ready set before its prerequisites closed and present afterward.\n\n| Item | Runtime id | Status |\n| --- | --- | --- |\n| Source review | `{}` | completed |\n| Badge review | `{}` | completed |\n| Release candidate | `{}` | completed |\n\nThese are fixture WorkGraph transitions; no deployment was performed.",
        review.id, render.id, publish.id,
    )))
}

fn workgraph_events_turn(recorded: &RecordedTurn<'_>) -> Result<Turn, String> {
    if let Some(turn) = recorded.pending(&[(
        "create-event-item",
        "workgraph_create",
        json!({
            "title": "Review WorkGraph activity",
            "description": "Review the recorded activity for this acceptance item.",
            "labels": [recorded.scenario.label()],
            "completion_policy": {"kind": "self_attest"},
        }),
    )])? {
        return Ok(turn);
    }
    let item = recorded.item("create-event-item", None)?;
    if item.status != "open" {
        return Err(format!("create-event-item returned status {}", item.status));
    }
    let poll = json!({"limit": 100});
    if let Some(turn) = recorded.pending(&[("events-first", "workgraph_events", poll.clone())])? {
        return Ok(turn);
    }
    let first = recorded
        .result("events-first")?
        .ok_or("missing first event poll")?;
    let after_seq = workgraph_event_watermark(&first, &item.id)?;
    if let Some(turn) = recorded.pending(&[("events-repeat", "workgraph_events", poll)])? {
        return Ok(turn);
    }
    let repeated = recorded
        .result("events-repeat")?
        .ok_or("missing overlapping event poll")?;
    if repeated != first {
        return Err("overlapping WorkGraph event polls changed without a graph mutation".into());
    }
    if let Some(turn) = recorded.pending(&[(
        "events-empty",
        "workgraph_events",
        json!({"limit": 100, "after_seq": after_seq}),
    )])? {
        return Ok(turn);
    }
    if recorded.result("events-empty")? != Some(json!({"events": []})) {
        return Err("event poll after the observed watermark must be exactly empty".into());
    }
    Ok(Turn::Text(format!(
        "## WorkGraph event review complete\n\nThe runtime returned the same recorded events for item `{}` in both overlapping polls. No newer events were returned after sequence `{after_seq}`.",
        item.id,
    )))
}

fn workgraph_event_watermark(value: &Value, item_id: &str) -> Result<i64, String> {
    let events = value
        .get("events")
        .and_then(Value::as_array)
        .filter(|events| !events.is_empty())
        .ok_or("first event poll must contain recorded events")?;
    let mut sequences = std::collections::BTreeSet::new();
    for event in events {
        if event.get("item_id").and_then(Value::as_str) != Some(item_id) {
            return Err("first event poll contains an event for another or missing item".into());
        }
        let seq = event
            .get("seq")
            .and_then(Value::as_i64)
            .filter(|seq| (0..=9_007_199_254_740_991).contains(seq))
            .ok_or("first event poll has no browser-safe integer sequence")?;
        if !sequences.insert(seq) {
            return Err("first event poll repeats a sequence".into());
        }
        if event
            .get("kind")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err("first event poll has no event kind".into());
        }
        let at = event
            .get("at")
            .and_then(Value::as_str)
            .ok_or("first event poll has no timestamp")?;
        chrono::DateTime::parse_from_rfc3339(at)
            .map_err(|error| format!("first event poll has an invalid timestamp: {error}"))?;
    }
    sequences
        .last()
        .copied()
        .ok_or_else(|| "first event poll has no watermark".into())
}

fn item_ids(value: &Value) -> Result<Vec<&str>, String> {
    value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("ready result has no items: {value}"))?
        .iter()
        .map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("ready item has no id: {item}"))
        })
        .collect()
}

fn peer_turn(recorded: &RecordedTurn<'_>) -> Result<Turn, String> {
    let peer_id = required(
        &recorded.scenario.peer_id,
        "peer_id (host-resolved canonical routing id)",
    )?;
    let body = required(&recorded.scenario.peer_body, "peer_body")?;
    if body.trim_start().starts_with("[fixture:") {
        return Err("peer_body must not start with an operator scenario trigger".into());
    }
    if let Some(turn) = recorded.pending(&[(
        "send-peer",
        "send_message",
        json!({
            "peer_id": peer_id, "body": body, "handling_mode": "queue",
        }),
    )])? {
        return Ok(turn);
    }
    let result = recorded
        .result("send-peer")?
        .ok_or("missing send_message result")?;
    if result["status"] != "sent" || !result.get("receipt").is_some_and(Value::is_object) {
        return Err(format!(
            "send_message did not return a sent receipt: {result}"
        ));
    }
    Ok(Turn::Text(format!(
        "## Peer message submitted\n\nThe runtime returned this send receipt. Recipient processing is verified separately in the recipient conversation.\n\n```json\n{}\n```",
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?,
    )))
}

fn image_turn(recorded: &RecordedTurn<'_>) -> Result<Turn, String> {
    let provider = required(&recorded.scenario.image_provider, "image_provider")?;
    let prompt = required(&recorded.scenario.image_prompt, "image_prompt")?;
    if let Some(turn) = recorded.pending(&[(
        "generate-image",
        "generate_image",
        json!({
            "request": { "intent": "generate", "prompt": prompt, "provider": provider, "count": 1 },
        }),
    )])? {
        return Ok(turn);
    }
    let result = recorded
        .result("generate-image")?
        .ok_or("missing generate_image result")?;
    if result.pointer("/terminal/terminal").and_then(Value::as_str) != Some("generated") {
        return Err(format!(
            "image operation did not generate an image: {result}"
        ));
    }
    let images = result
        .get("images")
        .and_then(Value::as_array)
        .filter(|images| !images.is_empty())
        .ok_or_else(|| format!("generated result has no committed image: {result}"))?;
    for image in images {
        if image.get("image_id").and_then(Value::as_str).is_none()
            || image
                .pointer("/blob_ref/blob_id")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            || image
                .pointer("/blob_ref/media_type")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err(format!(
                "generated result has an incomplete committed image: {image}"
            ));
        }
    }
    // Auto-sized provider results can carry zero dimension metadata. The
    // browser acceptance test resolves the committed blob and decodes its
    // actual pixels; metadata alone cannot establish a valid image.
    Ok(Turn::Text(format!(
        "## Image operation complete\n\nThe runtime reported `generated` and committed {} image artifact(s). The tool's assistant image block carries the preview.\n\n```json\n{}\n```",
        images.len(),
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use meerkat_core::{AssistantBlock, BlockAssistantMessage, ToolResult, UserMessage};

    fn comms_notice(direction: &str, text: &str) -> Message {
        serde_json::from_value(json!({
            "role": "system_notice", "kind": "comms", "body": "Peer message",
            "blocks": [{
                "type": "comms", "direction": direction, "kind": "message",
                "peer": {"display_name": "console-acceptance/lead/peer", "id": "9c6b3d2f-21aa-4cb7-bd87-6a77b3d9c012"},
                "summary": "Peer message", "content": [{"type": "text", "text": text}],
            }],
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn barrier_blocks_before_first_tool_until_exact_release() {
        let barrier = Arc::new(ModelBarrier::default());
        let command = json!({"action": "arm", "plan": {
            "id": "overlap-1", "match_text": "peer barrier marker", "source": "Peer complete."
        }});
        assert_eq!(barrier.control(&command).unwrap()["phase"], "armed");
        let client = RecordingClient::new(
            Arc::new(Mutex::new(ModelPlan {
                source: "Ordinary reply".into(),
                delay_ms: 0,
                chunk_chars: 32,
                reasoning_blocks: Vec::new(),
                scenario: None,
            })),
            Arc::new(Mutex::new(Vec::new())),
            barrier.clone(),
        );
        let request = LlmRequest::new(
            "gpt-5.5",
            vec![
                Message::User(UserMessage::text("Old spawn request")),
                comms_notice("incoming", "Incoming peer barrier marker"),
            ],
        );
        let mut stream = client.stream(&request);
        let first = stream.next();
        tokio::pin!(first);
        assert!(matches!(
            futures::poll!(first.as_mut()),
            std::task::Poll::Pending
        ));
        let status = barrier
            .control(&json!({"action": "status", "id": "overlap-1"}))
            .unwrap();
        assert_eq!(status["phase"], "entered");
        assert_eq!(status["requests"], 1);
        assert!(
            barrier
                .control(&json!({"action": "release", "id": "wrong"}))
                .is_err()
        );
        assert!(matches!(
            futures::poll!(first.as_mut()),
            std::task::Poll::Pending
        ));
        assert_eq!(
            barrier
                .control(&json!({"action": "release", "id": "overlap-1"}))
                .unwrap()["phase"],
            "released"
        );
        assert!(
            matches!(first.await, Some(Ok(LlmEvent::ToolCallComplete { id, name, .. }))
            if id == "fixture-overlap-1-peer-ready" && name == "workgraph_ready")
        );
    }

    #[tokio::test]
    async fn barrier_after_tool_result_holds_only_the_matching_completed_turn() {
        let barrier = Arc::new(ModelBarrier::default());
        barrier
            .control(&json!({"action": "arm", "plan": {
                "id": "apps-hold", "match_text": "[fixture:apps-hold]",
                "source": "Held response complete.", "after_tool_result": "native-display"
            }}))
            .unwrap();
        let input = Message::User(UserMessage::text("[fixture:apps-hold] Show records."));
        assert!(
            barrier
                .matching_request(std::slice::from_ref(&input))
                .is_none()
        );
        let native_result = Message::tool_results(vec![ToolResult::new(
            "native-display".into(),
            "{}".into(),
            false,
        )]);
        let messages = vec![input.clone(), native_result.clone()];
        assert!(barrier.matching_request(&messages).is_some());
        assert!(
            barrier
                .matching_request(&[input.clone(), native_result.clone(), native_result,])
                .is_none()
        );
        assert!(
            barrier
                .matching_request(&[
                    input.clone(),
                    Message::tool_results(vec![ToolResult::new(
                        "native-display".into(),
                        "{}".into(),
                        true,
                    )]),
                ])
                .is_none()
        );
        let mut superseded = messages.clone();
        superseded.push(Message::User(UserMessage::text("A different request.")));
        assert!(barrier.matching_request(&superseded).is_none());

        let client = RecordingClient::new(
            Arc::new(Mutex::new(ModelPlan {
                source: "Ordinary reply".into(),
                delay_ms: 0,
                chunk_chars: 64,
                reasoning_blocks: Vec::new(),
                scenario: None,
            })),
            Arc::new(Mutex::new(Vec::new())),
            barrier.clone(),
        );
        let request = LlmRequest::new("gpt-5.5", messages);
        let mut stream = client.stream(&request);
        let first = stream.next();
        tokio::pin!(first);
        assert!(matches!(
            futures::poll!(first.as_mut()),
            std::task::Poll::Pending
        ));
        assert_eq!(
            barrier
                .control(&json!({"action": "status", "id": "apps-hold"}))
                .unwrap()["phase"],
            "entered"
        );
        barrier
            .control(&json!({"action": "release", "id": "apps-hold"}))
            .unwrap();
        assert!(
            matches!(first.await, Some(Ok(LlmEvent::TextDelta { delta, .. }))
            if delta == "Held response complete.")
        );
    }

    #[tokio::test]
    async fn cancelled_barrier_wait_is_not_reported_as_a_live_model_request() {
        let run = BarrierRun {
            plan: BarrierPlan {
                id: "cancelled".into(),
                match_text: "marker".into(),
                source: "Ready.".into(),
                after_tool_result: None,
            },
            progress: Mutex::new(BarrierProgress::default()),
            release: Notify::new(),
        };
        let mut waiting = Box::pin(run.wait_for_release());
        assert!(matches!(
            futures::poll!(waiting.as_mut()),
            std::task::Poll::Pending
        ));
        assert_eq!(run.snapshot()["waiting_requests"], 1);
        drop(waiting);
        assert_eq!(run.snapshot()["waiting_requests"], 0);
        assert_eq!(run.snapshot()["released"], false);
    }

    #[tokio::test]
    async fn barrier_release_is_not_lost_and_old_peer_message_cannot_capture_new_operator() {
        let barrier = ModelBarrier::default();
        barrier
            .control(&json!({"action": "arm", "plan": {
                "id": "overlap-2", "match_text": "peer barrier marker", "source": "Peer complete."
            }}))
            .unwrap();
        let peer = comms_notice("incoming", "Incoming peer barrier marker");
        let messages = vec![
            peer.clone(),
            Message::User(UserMessage::text("New operator input")),
        ];
        assert!(barrier.matching_request(&messages).is_none());
        let (run, _) = barrier
            .matching_request(std::slice::from_ref(&peer))
            .unwrap();
        barrier
            .control(&json!({"action": "release", "id": "overlap-2"}))
            .unwrap();
        assert!(matches!(
            futures::poll!(Box::pin(run.wait_for_release())),
            std::task::Poll::Ready(())
        ));
        let result = Message::tool_results(vec![ToolResult::new(
            "fixture-overlap-2-peer-ready".into(),
            "{\"items\":[]}".into(),
            false,
        )]);
        let continuation = vec![
            Message::User(UserMessage::text("Old spawn request")),
            peer,
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "fixture-overlap-2-peer-ready".into(),
                    name: "workgraph_ready".into(),
                    args: serde_json::value::to_raw_value(&json!({})).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            result,
        ];
        let (_, start) = barrier.matching_request(&continuation).unwrap();
        assert_eq!(start, 1);
        assert!(
            matches!(barrier_turn(&run.plan, &continuation[start..]), Ok(Turn::Text(text)) if text == "Peer complete.")
        );
    }

    #[test]
    fn barrier_uses_latest_typed_input_and_ignores_non_input_notices() {
        let barrier = ModelBarrier::default();
        barrier
            .control(&json!({"action": "arm", "plan": {
                "id": "overlap-3", "match_text": "peer barrier marker", "source": "Peer complete."
            }}))
            .unwrap();
        let peer = comms_notice("incoming", "Incoming peer barrier marker");
        for newer in [
            Message::User(UserMessage::text("New operator input")),
            comms_notice("incoming", "A different peer input"),
            comms_notice("incoming", ""),
        ] {
            assert!(barrier.matching_request(&[peer.clone(), newer]).is_none());
        }
        for notice in [
            comms_notice("outgoing", "Sent to a peer"),
            comms_notice("internal", "Runtime housekeeping"),
            Message::SystemNotice(meerkat_core::SystemNoticeMessage::new(
                meerkat_core::SystemNoticeKind::Generic,
                "Unrelated notice",
            )),
        ] {
            let messages = [peer.clone(), notice];
            assert_eq!(barrier.matching_request(&messages).unwrap().1, 0);
        }
        // Prose alone cannot claim that a runtime notice is incoming peer input.
        let marker_only_in_body = Message::SystemNotice(meerkat_core::SystemNoticeMessage::new(
            meerkat_core::SystemNoticeKind::Comms,
            "Incoming peer barrier marker",
        ));
        assert!(barrier.matching_request(&[marker_only_in_body]).is_none());
        for direction in ["outgoing", "internal"] {
            assert!(
                barrier
                    .matching_request(&[comms_notice(direction, "Incoming peer barrier marker")])
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn configured_reasoning_streams_each_exact_block_before_the_answer() {
        let source = "Check again again and check again.";
        let plan: ModelPlan = serde_json::from_value(json!({
            "source": "Review complete.", "chunk_chars": 6,
            "reasoning_blocks": [source, source],
        }))
        .unwrap();
        let client = RecordingClient::new(
            Arc::new(Mutex::new(plan)),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(ModelBarrier::default()),
        );
        let request = LlmRequest::new(
            "gpt-5.5",
            vec![Message::User(UserMessage::text("Review the candidate."))],
        );
        let mut stream = client.stream(&request);
        let mut current = String::new();
        let mut completed = Vec::new();
        let mut deltas = Vec::new();
        let mut answer = String::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                LlmEvent::ReasoningDelta { delta } => {
                    assert!(answer.is_empty(), "reasoning precedes the answer");
                    current.push_str(&delta);
                    deltas.push(delta);
                }
                LlmEvent::ReasoningComplete { text, .. } => {
                    assert_eq!(current, source);
                    assert_eq!(text, source);
                    completed.push(text);
                    current.clear();
                }
                LlmEvent::TextDelta { delta, .. } => {
                    assert_eq!(completed.len(), 2);
                    answer.push_str(&delta);
                }
                _ => {}
            }
        }
        assert_eq!(completed, vec![source, source]);
        assert_eq!(
            deltas
                .iter()
                .filter(|delta| delta.as_str() == "again ")
                .count(),
            4
        );
        assert_eq!(answer, "Review complete.");
        assert!(current.is_empty());
    }

    #[tokio::test]
    async fn bootstrap_emits_visible_completion_and_usage_before_done() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let client = RecordingClient::new(
            Arc::new(Mutex::new(ModelPlan {
                source: "Scenario source must not run during bootstrap.".into(),
                delay_ms: 0,
                chunk_chars: 256,
                reasoning_blocks: Vec::new(),
                scenario: None,
            })),
            requests.clone(),
            Arc::new(ModelBarrier::default()),
        );
        let request = LlmRequest::new(
            "gpt-5.5",
            vec![Message::User(UserMessage::text(
                "You have been spawned as 'fixture-member' (role: lead) in mob 'fixture'.",
            ))],
        );
        let events: Vec<_> = client.stream(&request).collect().await;
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                Ok(LlmEvent::TextDelta { delta, .. }) => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Ready.");
        assert!(events.iter().all(|event| !matches!(
            event,
            Ok(LlmEvent::ReasoningDelta { .. } | LlmEvent::ReasoningComplete { .. })
        )));
        assert!(matches!(
            events.get(events.len().saturating_sub(2)),
            Some(Ok(LlmEvent::UsageUpdate { .. }))
        ));
        assert!(matches!(
            events.last(),
            Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                    ..
                }
            }))
        ));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    fn scenario(kind: ScenarioKind) -> ScenarioPlan {
        ScenarioPlan {
            kind,
            run_id: "release-1".into(),
            owner_id: Some("router:main".into()),
            peer_id: Some("canonical-peer-id".into()),
            peer_body: Some("Please review the release candidate.".into()),
            image_provider: Some("openai".into()),
            image_prompt: Some("A small blue shipping crate.".into()),
        }
    }

    fn start(scenario: &ScenarioPlan) -> Vec<Message> {
        vec![Message::User(UserMessage::text(format!(
            "{} Run the acceptance scenario.",
            scenario.trigger()
        )))]
    }

    #[test]
    fn routine_scenario_requires_real_results_and_preserves_a_failed_read() {
        let scenario = scenario(ScenarioKind::Routine);
        let mut messages = start(&scenario);
        let initial = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(initial.len(), 2);
        assert_eq!(initial[0].name, "read_file");
        assert_eq!(initial[1].name, "list_files");
        for (step, text, is_error, next) in [
            ("notes", "  exact notes\n", false, None),
            ("files", "notes.txt\n", false, Some("missing")),
            (
                "missing",
                "Cannot read missing.txt: NotFound",
                true,
                Some("late"),
            ),
            ("late", "Late review\n", false, Some("ready")),
            ("ready", "{\"items\":[]}", false, None),
        ] {
            messages.push(Message::tool_results(vec![ToolResult::new(
                scenario.call_id(step),
                text.to_owned(),
                is_error,
            )]));
            if let Some(next) = next {
                let requested = calls(scenario_turn(&scenario, &messages).unwrap());
                assert_eq!(requested.len(), 1);
                assert_eq!(requested[0].id, scenario.call_id(next));
            }
        }
        assert!(
            matches!(scenario_turn(&scenario, &messages).unwrap(), Turn::Text(text) if text.starts_with("Routine workspace review complete."))
        );
        let mut wrong = start(&scenario);
        wrong.push(result(&scenario, "notes", json!("notes")));
        wrong.push(result(&scenario, "files", json!("files")));
        wrong.push(result(&scenario, "missing", json!("false success")));
        assert!(
            scenario_turn(&scenario, &wrong)
                .unwrap_err()
                .contains("unexpected runtime outcome")
        );
    }

    fn result(scenario: &ScenarioPlan, step: &str, value: Value) -> Message {
        Message::tool_results(vec![ToolResult::new(
            scenario.call_id(step),
            value.to_string(),
            false,
        )])
    }

    #[test]
    fn mcp_apps_script_stages_only_the_model_tool_and_preserves_catalog_refusal() {
        let scenario = scenario(ScenarioKind::McpApps);
        let mut messages = start(&scenario);
        let initial = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].name, "tool_catalog_load");
        assert_eq!(initial[0].args, json!({"names": ["display"]}));
        messages.push(result(
            &scenario,
            "load-display",
            json!({"catalog_exact": true, "accepted_names": ["display"], "noop_names": []}),
        ));
        let display = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(display.len(), 1);
        assert_eq!(display[0].name, "display");
        messages.push(result(&scenario, "display", json!({"count": 7})));
        assert!(matches!(scenario_turn(&scenario, &messages).unwrap(),
            Turn::Text(text) if text == "The MCP App is ready."));

        let mut refused = start(&scenario);
        refused.push(result(
            &scenario,
            "load-display",
            json!({"catalog_exact": true, "accepted_names": [], "noop_names": []}),
        ));
        assert!(
            scenario_turn(&scenario, &refused)
                .unwrap_err()
                .contains("native catalog did not stage display")
        );
    }

    #[test]
    fn mcp_apps_polling_script_requires_three_distinct_native_results() {
        let scenario = scenario(ScenarioKind::McpAppsPolling);
        let mut messages = start(&scenario);
        assert_eq!(
            calls(scenario_turn(&scenario, &messages).unwrap())[0].name,
            "tool_catalog_load"
        );
        messages.push(result(
            &scenario,
            "load-display",
            json!({
                "catalog_exact": true, "accepted_names": ["display"], "noop_names": []
            }),
        ));
        for view in ["display", "display-two", "display-three"] {
            let display = calls(scenario_turn(&scenario, &messages).unwrap());
            assert_eq!(display.len(), 1);
            assert_eq!(display[0].id, scenario.call_id(view));
            assert_eq!(display[0].name, "display");
            assert_eq!(display[0].args, json!({"query": "records", "viewId": view}));
            messages.push(result(&scenario, view, json!({"count": 7, "viewId": view})));
        }
        assert!(matches!(scenario_turn(&scenario, &messages).unwrap(),
            Turn::Text(text) if text == "The MCP App is ready."));
    }

    #[test]
    fn assistant_identity_script_requires_two_real_results_before_its_identical_final() {
        let scenario = scenario(ScenarioKind::AssistantIdentity);
        let mut messages = start(&scenario);
        for step in ["identity-first", "identity-second"] {
            let Turn::TextAndTools { text, calls } = scenario_turn(&scenario, &messages).unwrap()
            else {
                panic!("an unfinished identity check must request its actual runtime tool");
            };
            assert_eq!(text, ASSISTANT_IDENTITY_SOURCE);
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, scenario.call_id(step));
            assert_eq!(calls[0].name, "workgraph_ready");
            assert_eq!(calls[0].args, json!({"labels": [scenario.label()]}));
            messages.push(result(&scenario, step, json!({"items": []})));
        }
        assert!(matches!(scenario_turn(&scenario, &messages).unwrap(),
            Turn::Text(text) if text == ASSISTANT_IDENTITY_SOURCE));

        let mut malformed = start(&scenario);
        malformed.push(result(
            &scenario,
            "identity-first",
            json!({"unexpected": true}),
        ));
        assert!(
            scenario_turn(&scenario, &malformed)
                .unwrap_err()
                .contains("items")
        );
        let mut failed = start(&scenario);
        failed.push(Message::tool_results(vec![ToolResult::new(
            scenario.call_id("identity-first"),
            "runtime query refused".to_string(),
            true,
        )]));
        assert!(
            scenario_turn(&scenario, &failed)
                .unwrap_err()
                .contains("runtime query refused")
        );
    }

    fn calls(turn: Turn) -> Vec<PlannedCall> {
        match turn {
            Turn::Tools(calls) => calls,
            _ => panic!("expected a real tool-call turn"),
        }
    }

    fn created(scenario: &ScenarioPlan) -> Vec<Message> {
        let mut messages = start(scenario);
        for (step, id) in [
            ("create-review", "review-id"),
            ("create-render", "render-id"),
            ("create-publish", "publish-id"),
        ] {
            messages.push(result(
                scenario,
                step,
                json!({"item": {
                    "id": id, "revision": 1, "status": "open",
                }}),
            ));
        }
        messages
    }

    fn linked(scenario: &ScenarioPlan) -> Vec<Message> {
        let mut messages = created(scenario);
        messages.push(result(scenario, "link-review", json!({"edge": {}})));
        messages.push(result(scenario, "link-render", json!({"edge": {}})));
        messages.push(result(
            scenario,
            "ready-before",
            json!({"items": [
                {"id": "review-id"}, {"id": "render-id"},
            ]}),
        ));
        messages.push(result(
            scenario,
            "get-review",
            json!({"item": {
                "id": "review-id", "revision": 7, "status": "open",
            }}),
        ));
        messages.push(result(
            scenario,
            "get-render",
            json!({"item": {
                "id": "render-id", "revision": 11, "status": "open",
            }}),
        ));
        messages
    }

    #[test]
    fn independent_creates_and_links_use_observed_ids() {
        let scenario = scenario(ScenarioKind::Workgraph);
        let first = calls(scenario_turn(&scenario, &start(&scenario)).unwrap());
        assert_eq!(first.len(), 3);
        assert!(first.iter().all(|call| call.name == "workgraph_create"));
        let links = calls(scenario_turn(&scenario, &created(&scenario)).unwrap());
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].args["from_id"], "review-id");
        assert_eq!(links[0].args["to_id"], "publish-id");
        assert_eq!(links[1].args["from_id"], "render-id");
    }

    fn event_item_created(scenario: &ScenarioPlan) -> Vec<Message> {
        let mut messages = start(scenario);
        messages.push(result(
            scenario,
            "create-event-item",
            json!({"item": {
                "id": "observed-event-item", "revision": 7, "status": "open",
                "title": "Review WorkGraph activity",
            }}),
        ));
        messages
    }

    fn observed_events() -> Value {
        json!({"events": [
            {"seq": 23, "item_id": "observed-event-item", "kind": "item_created",
             "at": "2026-09-26T10:20:00Z", "payload": {"source": "first"}},
            {"seq": 5, "item_id": "observed-event-item", "kind": "item_updated",
             "at": "2026-09-26T10:21:00Z", "payload": {"source": "second"}},
        ]})
    }

    #[test]
    fn workgraph_events_plan_uses_real_result_ids_overlap_and_maximum_cursor() {
        let scenario: ScenarioPlan = serde_json::from_value(json!({
            "kind": "workgraph_events", "run_id": "events-1",
        }))
        .expect("browser plan needs no owner or peer settings");
        assert!(matches!(scenario.kind, ScenarioKind::WorkgraphEvents));
        let create = calls(scenario_turn(&scenario, &start(&scenario)).unwrap());
        assert_eq!(create.len(), 1);
        assert_eq!(create[0].id, scenario.call_id("create-event-item"));
        assert_eq!(create[0].name, "workgraph_create");
        assert_eq!(create[0].args["labels"], json!([scenario.label()]));

        let mut messages = event_item_created(&scenario);
        let first = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, scenario.call_id("events-first"));
        assert_eq!(first[0].name, "workgraph_events");
        assert_eq!(first[0].args, json!({"limit": 100}));

        let events = observed_events();
        messages.push(result(&scenario, "events-first", events.clone()));
        let repeat = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(repeat.len(), 1);
        assert_eq!(repeat[0].id, scenario.call_id("events-repeat"));
        assert_eq!(repeat[0].name, "workgraph_events");
        assert_eq!(repeat[0].args, first[0].args);
        messages.push(result(&scenario, "events-repeat", events));
        let empty = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].id, scenario.call_id("events-empty"));
        assert_eq!(empty[0].name, "workgraph_events");
        assert_eq!(empty[0].args, json!({"limit": 100, "after_seq": 23}));
        messages.push(result(&scenario, "events-empty", json!({"events": []})));
        let Turn::Text(text) = scenario_turn(&scenario, &messages).unwrap() else {
            panic!("completed polling must produce an observed summary");
        };
        assert!(text.starts_with("## WorkGraph event review complete\n"));
        assert!(text.contains("observed-event-item"));
    }

    #[test]
    fn workgraph_events_plan_rejects_unusable_or_foreign_first_poll() {
        let scenario = scenario(ScenarioKind::WorkgraphEvents);
        let mut invalid = vec![json!({}), json!({"events": []}), json!({"events": {}})];
        for replacement in [
            Value::Null,
            json!(-1),
            json!(1.25),
            json!(9_007_199_254_740_992_u64),
        ] {
            let mut value = observed_events();
            value["events"][0]["seq"] = replacement;
            invalid.push(value);
        }
        for (field, replacement) in [
            ("seq", json!(5)),
            ("item_id", json!("another-item")),
            ("item_id", Value::Null),
            ("kind", Value::Null),
            ("at", json!("not-a-time")),
        ] {
            let mut value = observed_events();
            value["events"][0][field] = replacement;
            invalid.push(value);
        }
        for value in invalid {
            let mut messages = event_item_created(&scenario);
            messages.push(result(&scenario, "events-first", value.clone()));
            assert!(
                scenario_turn(&scenario, &messages).is_err(),
                "invalid first poll must not advance the script: {value}"
            );
        }
    }

    #[test]
    fn workgraph_events_plan_refuses_changed_overlap_or_nonempty_final_poll() {
        let scenario = scenario(ScenarioKind::WorkgraphEvents);
        let events = observed_events();
        let mut messages = event_item_created(&scenario);
        messages.push(result(&scenario, "events-first", events.clone()));
        let mut changed = events.clone();
        changed["events"][0]["payload"] = json!({"source": "changed"});
        messages.push(result(&scenario, "events-repeat", changed));
        assert!(
            scenario_turn(&scenario, &messages)
                .unwrap_err()
                .contains("overlap")
        );
        messages.pop();
        messages.push(result(&scenario, "events-repeat", events.clone()));
        for invalid in [events, json!({}), json!({"events": [], "unexpected": true})] {
            messages.push(result(&scenario, "events-empty", invalid));
            assert!(
                scenario_turn(&scenario, &messages)
                    .unwrap_err()
                    .contains("empty")
            );
            messages.pop();
        }
    }

    #[test]
    fn mutations_use_current_get_and_claim_revisions() {
        let scenario = scenario(ScenarioKind::Workgraph);
        let mut messages = linked(&scenario);
        let claims = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(claims[0].args["expected_revision"], 7);
        assert_eq!(claims[1].args["expected_revision"], 11);
        assert_eq!(claims[0].args["owner"]["key"]["id"], "router:main");
        for (step, id, revision) in [
            ("claim-review", "review-id", 8),
            ("claim-render", "render-id", 12),
        ] {
            messages.push(result(
                &scenario,
                step,
                json!({"item": {
                    "id": id, "revision": revision, "status": "in_progress",
                }}),
            ));
        }
        let closes = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(closes[0].args["expected_revision"], 8);
        assert_eq!(closes[1].args["expected_revision"], 12);
    }

    #[test]
    fn readiness_violation_stops_before_claiming() {
        let scenario = scenario(ScenarioKind::Workgraph);
        let mut messages = linked(&scenario);
        messages.push(result(
            &scenario,
            "ready-before",
            json!({"items": [
                {"id": "review-id"}, {"id": "render-id"}, {"id": "publish-id"},
            ]}),
        ));
        let error = scenario_turn(&scenario, &messages).unwrap_err();
        assert!(error.contains("dependent item was ready before"));
    }

    #[test]
    fn tool_errors_and_missing_results_never_become_success_or_retries() {
        let scenario = scenario(ScenarioKind::Peer);
        let mut messages = start(&scenario);
        messages.push(Message::tool_results(vec![ToolResult::new(
            scenario.call_id("send-peer"),
            "peer_unreachable".into(),
            true,
        )]));
        assert!(
            scenario_turn(&scenario, &messages)
                .unwrap_err()
                .contains("peer_unreachable")
        );
        messages.pop();
        messages.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: scenario.call_id("send-peer"),
                name: "send_message".into(),
                args: serde_json::value::to_raw_value(&json!({})).unwrap(),
                meta: None,
            }],
            StopReason::ToolUse,
        )));
        assert!(
            scenario_turn(&scenario, &messages)
                .unwrap_err()
                .contains("without a recorded result")
        );
    }

    #[test]
    fn peer_receipt_is_observed_without_claiming_recipient_processing() {
        let scenario = scenario(ScenarioKind::Peer);
        let mut messages = start(&scenario);
        let send = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(send[0].name, "send_message");
        assert_eq!(send[0].args["peer_id"], "canonical-peer-id");
        messages.push(result(
            &scenario,
            "send-peer",
            json!({
                "status": "sent", "kind": "message", "receipt": {"id": "receipt-1"},
            }),
        ));
        let Turn::Text(text) = scenario_turn(&scenario, &messages).unwrap() else {
            panic!("expected receipt summary");
        };
        assert!(text.contains("receipt-1"));
        assert!(text.contains("Recipient processing is verified separately"));
    }

    #[test]
    fn fresh_user_boundary_and_peer_turns_do_not_reuse_another_turn() {
        let scenario = scenario(ScenarioKind::Peer);
        let mut messages = start(&scenario);
        messages.push(result(
            &scenario,
            "send-peer",
            json!({
                "status": "sent", "receipt": {"id": "old"},
            }),
        ));
        messages.push(Message::User(UserMessage::text("Ordinary follow-up")));
        assert!(active_messages(&scenario, &messages).is_none());
        messages.push(Message::User(UserMessage::text(scenario.trigger())));
        let active = active_messages(&scenario, &messages).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(calls(scenario_turn(&scenario, active).unwrap()).len(), 1);
        assert!(
            active_messages(
                &scenario,
                &[Message::User(UserMessage::text(
                    "A peer asks to review the release candidate.",
                ))]
            )
            .is_none()
        );
    }

    #[test]
    fn image_completion_requires_generated_terminal_and_committed_artifact() {
        let scenario = scenario(ScenarioKind::Image);
        let mut messages = start(&scenario);
        let call = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(call[0].name, "generate_image");
        assert_eq!(call[0].args["request"]["intent"], "generate");
        assert_eq!(
            call[0].args["request"]["prompt"],
            "A small blue shipping crate."
        );
        messages.push(result(
            &scenario,
            "generate-image",
            json!({
                "operation_id": "op-1", "terminal": {"terminal": "generated"}, "images": [],
            }),
        ));
        assert!(
            scenario_turn(&scenario, &messages)
                .unwrap_err()
                .contains("committed image")
        );
        messages.pop();
        messages.push(result(
            &scenario,
            "generate-image",
            json!({
                "operation_id": "op-1", "terminal": {"terminal": "denied"},
            }),
        ));
        assert!(
            scenario_turn(&scenario, &messages)
                .unwrap_err()
                .contains("did not generate")
        );
        messages.pop();
        messages.push(result(
            &scenario,
            "generate-image",
            json!({
                "operation_id": "op-1", "terminal": {"terminal": "generated"},
                "images": [{"image_id": "image-1", "width": 1024, "height": 1024,
                    "blob_ref": {"blob_id": "sha256:observed-test-blob", "media_type": "image/png"},
                }],
            }),
        ));
        let Turn::Text(text) = scenario_turn(&scenario, &messages).unwrap() else {
            panic!("expected observed image summary");
        };
        assert!(text.contains("sha256:observed-test-blob"));
        messages.pop();
        messages.push(result(
            &scenario,
            "generate-image",
            json!({
                "operation_id": "op-auto", "terminal": {"terminal": "generated"},
                "images": [{"image_id": "image-auto", "width": 0, "height": 0,
                    "media_type": "image/png",
                    "blob_ref": {"blob_id": "sha256:auto-sized", "media_type": "image/png"},
                }],
            }),
        ));
        assert!(matches!(
            scenario_turn(&scenario, &messages),
            Ok(Turn::Text(_))
        ));
    }

    #[test]
    fn complete_graph_checks_both_readiness_transition_and_final_snapshot() {
        let scenario = scenario(ScenarioKind::Workgraph);
        let mut messages = linked(&scenario);
        for (step, id, revision, status) in [
            ("claim-review", "review-id", 8, "in_progress"),
            ("claim-render", "render-id", 12, "in_progress"),
            ("close-review", "review-id", 9, "completed"),
            ("close-render", "render-id", 13, "completed"),
        ] {
            messages.push(result(
                &scenario,
                step,
                json!({"item": {
                    "id": id, "revision": revision, "status": status,
                }}),
            ));
        }
        messages.push(result(
            &scenario,
            "ready-after",
            json!({"items": [{"id": "publish-id"}]}),
        ));
        messages.push(result(
            &scenario,
            "get-publish",
            json!({"item": {
                "id": "publish-id", "revision": 15, "status": "open",
            }}),
        ));
        let claim = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(claim[0].args["expected_revision"], 15);
        messages.push(result(
            &scenario,
            "claim-publish",
            json!({"item": {
                "id": "publish-id", "revision": 16, "status": "in_progress",
            }}),
        ));
        let close = calls(scenario_turn(&scenario, &messages).unwrap());
        assert_eq!(close[0].args["expected_revision"], 16);
        messages.push(result(
            &scenario,
            "close-publish",
            json!({"item": {
                "id": "publish-id", "revision": 17, "status": "completed",
            }}),
        ));
        assert_eq!(
            calls(scenario_turn(&scenario, &messages).unwrap())[0].name,
            "workgraph_snapshot"
        );
        messages.push(result(
            &scenario,
            "snapshot",
            json!({"snapshot": {
                "items": [
                    {"id": "review-id", "status": "completed"},
                    {"id": "render-id", "status": "completed"},
                    {"id": "publish-id", "status": "completed"},
                ],
                "edges": [
                    {"kind": "blocks", "from_id": "review-id", "to_id": "publish-id"},
                    {"kind": "blocks", "from_id": "render-id", "to_id": "publish-id"},
                ],
            }}),
        ));
        assert!(matches!(
            scenario_turn(&scenario, &messages).unwrap(),
            Turn::Text(_)
        ));
        messages.push(result(
            &scenario,
            "snapshot",
            json!({"snapshot": {"items": [], "edges": []}}),
        ));
        assert!(
            scenario_turn(&scenario, &messages)
                .unwrap_err()
                .contains("does not confirm completed")
        );
    }
}
