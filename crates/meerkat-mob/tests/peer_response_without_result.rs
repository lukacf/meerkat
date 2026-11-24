//! A peer response that carries no result must not stop the session that
//! receives it.
//!
//! Two autonomous-host members of one mob over the persistent session
//! service and a SQLite realm. A scripted model lists its peers and sends a
//! typed request; the other member's scripted model answers `send_response`
//! with one status and, in every row but the pin, no result. The requester's
//! runtime consumes that response in a run. A prompt put on the requester
//! afterwards must run, and the mob must still shut down or be destroyed.
//!
//! Before the fix the run committed, the completion's finalizing write was
//! refused for good, the runtime loop left with its registration kept, and
//! every later input of that session was accepted and never ran, while the
//! member still read active.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meerkat::{AgentFactory, Config, FactoryAgentBuilder, PersistentSessionService};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobHandle, MobId, MobRuntimeMode, MobState,
    MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{Input, MeerkatMachine, PromptInput};

const STEP: Duration = Duration::from_mins(1);
const ANSWER_BOUND: Duration = Duration::from_secs(40);
const EXCHANGE_BOUND: Duration = Duration::from_secs(10);
const ASK_MARKER: &str = "ASK_PEER";
const REPLY_MARKER: &str = "Reply with send_response with arguments ";
const LATER_PROMPT: &str = "a prompt after the peer exchange";

/// The responder's answer to a peer request: `send_response` with this
/// status and, when given, this typed result.
#[derive(Clone)]
struct Reply {
    status: &'static str,
    result: Option<serde_json::Value>,
}

/// One scripted model for both members. What it does is decided by the
/// request: a prompt carrying [`ASK_MARKER`] lists the peers and sends a
/// request; a peer request is answered as [`Reply`] says; everything else
/// ends the turn.
struct ScriptedClient {
    reply: Reply,
    replies_sent: AtomicUsize,
    turns_ended_after_a_tool_result: AtomicUsize,
    later_prompt_turns: AtomicUsize,
    notes: Mutex<Vec<String>>,
}

impl ScriptedClient {
    fn new(reply: Reply) -> Self {
        Self {
            reply,
            replies_sent: AtomicUsize::new(0),
            turns_ended_after_a_tool_result: AtomicUsize::new(0),
            later_prompt_turns: AtomicUsize::new(0),
            notes: Mutex::new(Vec::new()),
        }
    }

    fn note(&self, line: String) {
        self.notes.lock().unwrap().push(line);
    }

    fn tool_call(
        request: &LlmRequest,
        id: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Vec<LlmEvent> {
        vec![
            LlmEvent::ToolCallComplete {
                id: id.to_string(),
                name: name.to_string(),
                args,
                meta: None,
            },
            Self::usage(request),
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
            },
        ]
    }

    fn text(request: &LlmRequest, text: &str) -> Vec<LlmEvent> {
        vec![
            LlmEvent::TextDelta {
                delta: text.to_string(),
                meta: None,
            },
            Self::usage(request),
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            },
        ]
    }

    fn usage(request: &LlmRequest) -> LlmEvent {
        LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::OpenAI,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        }
    }
}

/// The first `peer_id` string in `value`, preferring an object whose text
/// names `prefer`.
fn find_peer_id(value: &serde_json::Value, prefer: &str) -> Option<String> {
    fn walk(
        value: &serde_json::Value,
        prefer: &str,
        preferred: &mut Option<String>,
        any: &mut Option<String>,
    ) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::String(peer_id)) = map.get("peer_id") {
                    if any.is_none() {
                        *any = Some(peer_id.clone());
                    }
                    if preferred.is_none() && value.to_string().contains(prefer) {
                        *preferred = Some(peer_id.clone());
                    }
                }
                for child in map.values() {
                    walk(child, prefer, preferred, any);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    walk(child, prefer, preferred, any);
                }
            }
            _ => {}
        }
    }
    let mut preferred = None;
    let mut any = None;
    walk(value, prefer, &mut preferred, &mut any);
    preferred.or(any)
}

/// Every string leaf of `value`.
fn string_leaves(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => out.push(text.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|item| string_leaves(item, out)),
        serde_json::Value::Object(map) => map.values().for_each(|item| string_leaves(item, out)),
        _ => {}
    }
}

/// The JSON object that starts at the first `{` of `text`.
fn leading_json_object(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let mut stream =
        serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
    stream.next()?.ok()
}

#[async_trait::async_trait]
impl LlmClient for ScriptedClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        // The newest text of any message that carries a peer request's reply
        // affordance, unless a reply was already sent after it. The request
        // is not rendered as a user message, so every string leaf is read.
        let mut unanswered_request: Option<String> = None;
        // The last user message, and the tool results that follow it.
        let mut last_user = String::new();
        let mut results_after: Vec<(String, String)> = Vec::new();
        for message in &request.messages {
            if let Message::ToolResults { results, .. } = message
                && results
                    .iter()
                    .any(|result| result.tool_use_id == "scripted-reply")
            {
                unanswered_request = None;
            }
            if let Ok(value) = serde_json::to_value(message) {
                let mut leaves = Vec::new();
                string_leaves(&value, &mut leaves);
                if let Some(text) = leaves.into_iter().find(|leaf| leaf.contains(REPLY_MARKER)) {
                    unanswered_request = Some(text);
                }
            }
            match message {
                Message::User(user) => {
                    last_user = user.text_content();
                    results_after.clear();
                }
                Message::ToolResults { results, .. } => {
                    for result in results {
                        results_after.push((result.tool_use_id.clone(), result.text_content()));
                    }
                }
                _ => {}
            }
        }
        let events = if let Some((id, text)) = results_after.last() {
            let peer_id = (id == "scripted-peers")
                .then(|| serde_json::from_str::<serde_json::Value>(text).ok())
                .flatten()
                .and_then(|value| find_peer_id(&value, "responder"));
            match peer_id {
                Some(peer_id) => Self::tool_call(
                    request,
                    "scripted-request",
                    "send_request",
                    serde_json::json!({
                        "peer_id": peer_id,
                        "intent": "checksum_token",
                        "params": {"subject": "status"},
                        "handling_mode": "queue",
                    }),
                ),
                None => {
                    self.note(format!(
                        "tool result {id}: {}",
                        text.chars().take(200).collect::<String>()
                    ));
                    self.turns_ended_after_a_tool_result
                        .fetch_add(1, Ordering::SeqCst);
                    Self::text(request, "done")
                }
            }
        } else if last_user.contains(ASK_MARKER) {
            Self::tool_call(request, "scripted-peers", "peers", serde_json::json!({}))
        } else if let Some(request_text) = unanswered_request.as_deref()
            && let Some(at) = request_text.find(REPLY_MARKER)
            && let Some(serde_json::Value::Object(mut args)) =
                leading_json_object(&request_text[at + REPLY_MARKER.len()..])
        {
            args.insert(
                "status".to_string(),
                serde_json::Value::String(self.reply.status.to_string()),
            );
            args.remove("result");
            if let Some(result) = &self.reply.result {
                args.insert("result".to_string(), result.clone());
            }
            self.replies_sent.fetch_add(1, Ordering::SeqCst);
            self.note(format!(
                "reply: {}",
                serde_json::Value::Object(args.clone())
            ));
            Self::tool_call(
                request,
                "scripted-reply",
                "send_response",
                serde_json::Value::Object(args),
            )
        } else {
            if last_user.contains(LATER_PROMPT) {
                self.later_prompt_turns.fetch_add(1, Ordering::SeqCst);
            }
            Self::text(request, "ok")
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

async fn build_service(
    root: &Path,
    client: Arc<ScriptedClient>,
) -> (
    Arc<PersistentSessionService<FactoryAgentBuilder>>,
    Arc<MeerkatMachine>,
) {
    let (_manifest, persistence) = meerkat::open_realm_persistence_in(
        root,
        "peer-response-without-result",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let project = root.join("project");
    std::fs::create_dir_all(&project).expect("project dir");
    let factory = AgentFactory::new(root.join("sessions"))
        .runtime_root(root.join("realm"))
        .project_root(&project)
        .comms(true);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(client);
    meerkat::surface::build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        8,
        persistence,
        root.join("config-state.json"),
    )
    .expect("compose the runtime-backed service")
}

fn profile() -> ProfileBinding {
    ProfileBinding::Inline(Box::new(Profile {
        model_fallback: None,
        model: "gpt-5.5".to_string(),
        provider: None,
        self_hosted_server_id: None,
        auth_binding: None,
        image_generation_provider: None,
        auto_compact_threshold: None,
        resume_overrides: Vec::new(),
        skills: vec![],
        tools: ToolConfig {
            comms: true,
            ..Default::default()
        },
        peer_description: "team member".to_string(),
        external_addressable: true,
        backend: None,
        runtime_mode: MobRuntimeMode::AutonomousHost,
        max_inline_peer_notifications: None,
        output_schema: None,
        provider_params: None,
        durable: false,
    }))
}

fn mob_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(ProfileName::from("requester"), profile());
    profiles.insert(ProfileName::from("responder"), profile());
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "peer-response-without-result-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

/// How a row ends its mob.
enum End {
    Shutdown,
    Destroy,
}

/// Two wired autonomous members, their kickoff turns over.
struct Pair {
    _root: tempfile::TempDir,
    client: Arc<ScriptedClient>,
    machine: Arc<MeerkatMachine>,
    handle: MobHandle,
    requester: AgentIdentity,
}

impl Pair {
    async fn start(reply: Reply) -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let client = Arc::new(ScriptedClient::new(reply));
        let (service, machine) = build_service(root.path(), Arc::clone(&client)).await;
        let storage = MobStorage::persistent(root.path().join("mob.db")).expect("mob storage");
        let handle = MobBuilder::new(mob_definition(), storage)
            .with_session_service(service)
            .with_runtime_adapter(Arc::clone(&machine))
            .with_default_llm_client(client.clone())
            .create()
            .await
            .expect("create mob");
        let requester = AgentIdentity::from("requester-1");
        let responder = AgentIdentity::from("responder-1");
        for (profile, identity) in [("requester", &requester), ("responder", &responder)] {
            tokio::time::timeout(
                STEP,
                handle.spawn_spec(SpawnMemberSpec::new(profile, identity.clone())),
            )
            .await
            .expect("the spawn completes")
            .expect("spawn the member");
        }
        tokio::time::timeout(STEP, handle.wire(requester.clone(), responder))
            .await
            .expect("the wiring completes")
            .expect("wire the requester to the responder");
        // The two kickoff turns run and end. Nothing the rows can observe
        // marks their end, so this stays a fixed settle.
        tokio::time::sleep(Duration::from_secs(3)).await;
        Self {
            _root: root,
            client,
            machine,
            handle,
            requester,
        }
    }

    /// Put a prompt on the requester's session and require that its run
    /// completes with a result within the bound.
    async fn prompt_runs(&self, text: &str) {
        let session_id = self
            .handle
            .resolve_bridge_session_id(&self.requester)
            .await
            .expect("the requester is session-backed");
        let (_outcome, completion) = self
            .machine
            .accept_input_with_completion(
                &session_id,
                Input::Prompt(PromptInput::new(text.to_string(), None)),
            )
            .await
            .expect("the prompt is admitted");
        let completion = completion.expect("a prompt has a completion");
        let outcome = tokio::time::timeout(ANSWER_BOUND, completion.wait()).await;
        let notes = self.client.notes.lock().unwrap().join("\n");
        let outcome = outcome.unwrap_or_else(|_| {
            panic!(
                "the prompt {text:?} was accepted and never ran within {ANSWER_BOUND:?}\n{notes}"
            )
        });
        assert!(
            matches!(outcome, Ok(CompletionOutcome::Completed(_))),
            "the prompt {text:?} ended as {outcome:?}\n{notes}"
        );
    }

    /// Wait, at most [`EXCHANGE_BOUND`], until `turns` scripted turns ended
    /// after a tool result.
    async fn turns_ended_after_a_tool_result(&self, turns: usize) {
        let deadline = Instant::now() + EXCHANGE_BOUND;
        while self
            .client
            .turns_ended_after_a_tool_result
            .load(Ordering::SeqCst)
            < turns
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The mob is running; then it shuts down, or is destroyed, completely.
    async fn ends_healthy(self, end: End) {
        assert_eq!(
            self.handle.status().await.expect("mob status"),
            MobState::Running
        );
        match end {
            End::Shutdown => {
                tokio::time::timeout(STEP, self.handle.shutdown())
                    .await
                    .expect("the shutdown returns")
                    .expect("the mob shuts down, every session unregistered");
            }
            End::Destroy => {
                tokio::time::timeout(STEP, self.handle.destroy())
                    .await
                    .expect("the destroy returns")
                    .expect("the mob is destroyed, every member's session disposed of");
            }
        }
    }
}

/// The requester asks the responder, the responder answers as `reply` says,
/// the requester's runtime consumes the answer, a later prompt to the
/// requester runs, and the mob ends as `end` says.
async fn a_reply_does_not_stop_the_requester(reply: Reply, end: End) {
    let pair = Pair::start(reply).await;
    pair.prompt_runs(&format!("{ASK_MARKER}: ask the responder"))
        .await;
    // The request goes out, the responder answers, and the answer reaches
    // the requester's runtime, which consumes it in a run of its own. Three
    // scripted turns end after a tool result: the requester's ask, the
    // responder's reply, and the requester's run of the answer.
    pair.turns_ended_after_a_tool_result(3).await;
    assert_eq!(
        pair.client.replies_sent.load(Ordering::SeqCst),
        1,
        "the responder answered the request once: {:?}",
        pair.client.notes.lock().unwrap()
    );
    pair.prompt_runs(LATER_PROMPT).await;
    assert_eq!(pair.client.later_prompt_turns.load(Ordering::SeqCst), 1);
    pair.ends_healthy(end).await;
}

fn without_a_result(status: &'static str) -> Reply {
    Reply {
        status,
        result: None,
    }
}

/// The requester keeps running after a progress notice without a result,
/// and the mob that holds it is destroyed completely.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_accepted_reply_without_a_result_does_not_stop_the_requester() {
    a_reply_does_not_stop_the_requester(without_a_result("accepted"), End::Destroy).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_reply_without_a_result_does_not_stop_the_requester() {
    a_reply_does_not_stop_the_requester(without_a_result("completed"), End::Shutdown).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_reply_without_a_result_does_not_stop_the_requester() {
    a_reply_does_not_stop_the_requester(without_a_result("failed"), End::Shutdown).await;
}

/// Pin: a reply that carries its typed result never stopped the requester.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_accepted_reply_with_a_result_does_not_stop_the_requester() {
    a_reply_does_not_stop_the_requester(
        Reply {
            status: "accepted",
            result: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "status",
                "token": "accepted",
            })),
        },
        End::Shutdown,
    )
    .await;
}
