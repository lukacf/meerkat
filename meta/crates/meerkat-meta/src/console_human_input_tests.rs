#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::skills::{SkillKey, SkillName, SkillRef, SourceUuid};
use meerkat_core::types::{HandlingMode, TranscriptUserRole};
use meerkat_core::{ContentInput, Message, Provider, SessionId};
use meerkat_mob::{MobDefinition, MobDeliveryIdentity, WorkOrigin, WorkSpec};
use serde_json::json;

use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleFrame, ConsoleFrameSourceKind, ConsoleFrameStatus,
    ConsoleInteractionAccepted, ConsoleSendRequest, ConsoleTimelineQuery, MetaConsoleAggregator,
};
use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryError, AgentMemoryPerTurnInjection, AgentMemoryProvider,
    AgentMemoryRecallRequest, AgentMemoryRecord, AgentMemoryRuntimeInjector,
};
use crate::identity_first::bridge::{HostHumanInputError, MobSessionBridge};
use crate::identity_first::orchestrator::restore_flow;
use crate::identity_first::*;
use crate::unified_runtime::{ConsoleEventStore, UnifiedRuntime, UnifiedRuntimeBuilder};

const WAIT: Duration = Duration::from_secs(30);
const HUMAN: &str = "CURRENT_VALUE is cobalt; remember my exact choice.";

#[derive(Clone)]
struct RecordingClient {
    requests: Arc<Mutex<Vec<LlmRequest>>>,
    gate: tokio::sync::watch::Sender<bool>,
    /// How many model requests this client has seen, to wait on one.
    seen: tokio::sync::watch::Sender<usize>,
}

impl RecordingClient {
    fn new() -> Self {
        Self {
            requests: Arc::default(),
            gate: tokio::sync::watch::channel(true).0,
            seen: tokio::sync::watch::channel(0).0,
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for RecordingClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        self.requests.lock().unwrap().push(request.clone());
        self.seen.send_modify(|count| *count += 1);
        let mut gate = self.gate.subscribe();
        Box::pin(async_stream::try_stream! {
            while !*gate.borrow_and_update() {
                gate.changed().await.unwrap();
            }
            yield LlmEvent::TextDelta { delta: "Your choice is cobalt.".to_string(), meta: None };
            yield LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI, &request.model, meerkat_core::Usage::default(),
                ),
            };
            yield LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            };
        })
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct Harness {
    unified: UnifiedRuntime,
    identity_runtime: Arc<IdentityRuntime>,
    bridge: Arc<MobSessionBridge>,
    aggregator: MetaConsoleAggregator,
    identity: AgentIdentity,
    runtime_id: AgentRuntimeId,
    session_id: SessionId,
    client: RecordingClient,
    _state: tempfile::TempDir,
}

impl Harness {
    async fn new(budget: Duration) -> Self {
        Self::with_kickoff(budget, false).await
    }

    async fn with_kickoff(budget: Duration, kickoff: bool) -> Self {
        Self::build(budget, kickoff, None).await
    }

    async fn with_meerkat_config(config: meerkat::Config) -> Self {
        Self::build(WAIT, false, Some(config)).await
    }

    async fn build(budget: Duration, kickoff: bool, config: Option<meerkat::Config>) -> Self {
        let state = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let client = RecordingClient::new();
        let builder = match config {
            Some(config) => UnifiedRuntimeBuilder::default().meerkat_config(config),
            None => UnifiedRuntimeBuilder::default(),
        };
        let unified = builder
            .definition(
                MobDefinition::from_toml(&format!(
                    r#"
[mob]
id = "console-human-{}"
[profiles.human]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "autonomous_host"
[profiles.human.tools]
comms = true
"#,
                    uuid::Uuid::new_v4()
                ))
                .unwrap(),
            )
            .persistent_state(state.path())
            .comms(true)
            .default_llm_client(Arc::new(client.clone()))
            .build()
            .await
            .unwrap();
        let bridge = Arc::new(
            MobSessionBridge::with_session_service(
                unified.mob_handle(),
                unified.mob_runtime().session_service().cloned().unwrap(),
            )
            .with_actor_admission_budget(budget),
        );
        let identity_runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "human-test".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge.clone()),
                default_timeout: None,
            })
            .with_runtime_services(AgentRuntimeServices::new(unified.mob_handle())),
        );
        let identity = AgentIdentity::parse("human").unwrap();
        let spec = DurableAgentSpec {
            identity: identity.clone(),
            profile: "human".into(),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: kickoff.then(|| "fixture kickoff".into()),
            runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
            backend: None,
            binding: None,
            placement: None,
        };
        restore_flow(&identity_runtime, &[spec], None, None)
            .await
            .unwrap();
        let status = identity_runtime.status(&identity).await.unwrap();
        if kickoff {
            // Read the kickoff phase from the roster (a plain actor read), not
            // through member_status: a member-status read joins the member's
            // status observation, and back-to-back observations can hold the
            // kickoff turn off (meerkat #1226). Roster reads never do.
            let member_id = crate::member_comms_id::mob_member_id(identity.as_str());
            tokio::time::timeout(WAIT, async {
                loop {
                    let entry = unified.mob_handle().get_member(&member_id).await.unwrap();
                    if entry
                        .as_ref()
                        .and_then(|entry| entry.kickoff.as_ref())
                        .is_some_and(|kickoff| {
                            kickoff.phase == meerkat_mob::MobMemberKickoffPhase::Started
                        })
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        } else if unified
            .mob_handle()
            .get_member(&crate::member_comms_id::mob_member_id(identity.as_str()))
            .await
            .unwrap()
            .is_some_and(|entry| entry.kickoff.is_some())
        {
            // The member is an autonomous host, so Core runs its spawn kickoff
            // (Core's own spawn prompt) on its own schedule, and that turn can
            // reach the model at any point of a row. Wait for its model request,
            // then start the row with an empty request log: a row's checks see
            // only what the row itself caused.
            let mut seen = client.seen.subscribe();
            tokio::time::timeout(WAIT, seen.wait_for(|count| *count > 0))
                .await
                .expect("the member's spawn kickoff reached the model")
                .expect("the recording client is alive");
            client.requests.lock().unwrap().clear();
        }
        let aggregator = MetaConsoleAggregator::in_memory();
        aggregator.register_runtime_handles_with_policy(
            "human",
            "",
            unified.mob_runtime().clone(),
            Some(identity_runtime.clone()),
            ConsoleEventStore::new(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        Self {
            unified,
            identity_runtime,
            bridge,
            aggregator,
            identity,
            runtime_id: status.agent_runtime_id.unwrap(),
            session_id: status.session_id.unwrap(),
            client,
            _state: state,
        }
    }

    fn request(&self, key: &str, content: &str) -> ConsoleSendRequest {
        ConsoleSendRequest {
            identity: self.identity.to_string(),
            content: json!(content),
            origin: "console".to_string(),
            idempotency_key: key.to_string(),
            handling_mode: None,
            origin_kind: None,
            skill_refs: Vec::new(),
        }
    }

    async fn send(
        &self,
        request: ConsoleSendRequest,
    ) -> Result<ConsoleInteractionAccepted, crate::console_aggregator::ConsoleSendError> {
        super::console_send_identity_first(
            &self.aggregator,
            self.identity_runtime.clone(),
            None,
            request,
        )
        .await
    }

    async fn wait_status(
        &self,
        accepted: &ConsoleInteractionAccepted,
        expected: ConsoleFrameStatus,
    ) {
        tokio::time::timeout(WAIT, async {
            loop {
                let page = self
                    .aggregator
                    .query_timeline(ConsoleTimelineQuery {
                        identity: Some(accepted.identity.clone()),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
                if page
                    .frames
                    .iter()
                    .any(|frame| frame.id == accepted.input_frame_id && frame.status == expected)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("console dispatch status");
    }

    async fn finish_human(&self, accepted: &ConsoleInteractionAccepted, content: &str) {
        self.finish_human_with_mode(accepted, content, HandlingMode::Queue)
            .await;
    }

    async fn finish_human_with_mode(
        &self,
        accepted: &ConsoleInteractionAccepted,
        content: &str,
        mode: HandlingMode,
    ) {
        self.finish_human_with_context(accepted, content, mode, Vec::new())
            .await;
    }

    async fn finish_human_with_context(
        &self,
        accepted: &ConsoleInteractionAccepted,
        content: &str,
        mode: HandlingMode,
        injected_context: Vec<ContentInput>,
    ) {
        self.wait_status(accepted, ConsoleFrameStatus::Delivered)
            .await;
        let handle = self.unified.mob_handle();
        let member = handle
            .get_member(&crate::member_comms_id::mob_member_id(&accepted.identity))
            .await
            .unwrap()
            .unwrap();
        let interaction = accepted.interaction_id.parse::<uuid::Uuid>().unwrap();
        let spec = WorkSpec::new(content, WorkOrigin::Internal)
            .with_injected_context(injected_context)
            .with_interaction_id(meerkat_core::interaction::InteractionId(interaction));
        let turn = handle
            .start_host_human_input_bounded(
                member.agent_runtime_id,
                member.fence_token,
                spec,
                mode,
                MobDeliveryIdentity::new(&accepted.input_frame_id, interaction.to_string())
                    .unwrap(),
                std::time::Instant::now() + WAIT,
            )
            .await
            .unwrap();
        tokio::time::timeout(WAIT, turn.wait())
            .await
            .unwrap()
            .unwrap();
    }

    /// Observe the exact console turn to completion by replaying its
    /// delivery identity with the SAME selected skills (a changed selection
    /// would be an idempotency conflict, not a replay).
    async fn finish_human_with_skills(
        &self,
        accepted: &ConsoleInteractionAccepted,
        content: &str,
        skills: Vec<SkillKey>,
    ) {
        self.wait_status(accepted, ConsoleFrameStatus::Delivered)
            .await;
        let handle = self.unified.mob_handle();
        let member = handle
            .get_member(&crate::member_comms_id::mob_member_id(&accepted.identity))
            .await
            .unwrap()
            .unwrap();
        let interaction = accepted.interaction_id.parse::<uuid::Uuid>().unwrap();
        let spec = WorkSpec::new(content, WorkOrigin::Internal)
            .with_interaction_id(meerkat_core::interaction::InteractionId(interaction));
        let turn = handle
            .start_host_human_input_with_options_bounded(
                member.agent_runtime_id,
                member.fence_token,
                spec,
                HandlingMode::Queue,
                meerkat_mob::MemberTurnOptions::new().with_skill_references(skills),
                MobDeliveryIdentity::new(&accepted.input_frame_id, interaction.to_string())
                    .unwrap(),
                std::time::Instant::now() + WAIT,
            )
            .await
            .unwrap();
        tokio::time::timeout(WAIT, turn.wait())
            .await
            .unwrap()
            .unwrap();
    }

    /// The ids and statuses of the console inputs reserved for `identity`:
    /// the `user_input` frames its sends made. A timeline read can start a
    /// session-history backfill, which projects the member's committed user
    /// messages (its spawn kickoff among them) as `user_input` frames of
    /// their own, landing at any later read; those are no console input.
    async fn reserved_inputs(&self, identity: &str) -> Vec<(String, ConsoleFrameStatus)> {
        self.aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some(identity.to_string()),
                ..Default::default()
            })
            .await
            .unwrap()
            .frames
            .into_iter()
            .filter(|frame| {
                frame.kind == "user_input" && frame.source.kind == ConsoleFrameSourceKind::Send
            })
            .map(|frame| (frame.id, frame.status))
            .collect()
    }

    async fn history(&self) -> Vec<Message> {
        self.unified
            .mob_runtime()
            .read_session_history(&self.session_id.to_string(), 0, None)
            .await
            .unwrap()
            .messages
    }

    async fn stop(self) {
        // Teardown stops the mob with the plain Stop, bounded by the mob
        // handle's own command guard, as the console aggregator suites do.
        // The gateway's idle-host flow-run budget
        // (`unified_runtime::mob_stop::MOB_STOP_FLOW_SETTLE_BUDGET`, with its
        // own paused-time deadline and refusal tests) is not a bound for a
        // test's teardown under a loaded host.
        let stopped = self.unified.mob_handle().stop().await;
        // Even when the Stop failed: no further delivery is applied to this
        // mob. This call is synchronous; it does not join delivery work that
        // may already be running.
        self.unified.mob_runtime().stop_runtime_delivery();
        if let Err(error) = stopped {
            panic!("mob stop failed: {error}");
        }
    }
}

// The event feed retains both admission and committed transcript evidence.
// A conversation renderer joins their exact interaction; raw-row count is not
// a count of authored human inputs. Check both sources without hiding either.
fn assert_canonical_human_frame(
    frame: &ConsoleFrame,
    accepted: &ConsoleInteractionAccepted,
    session_id: &SessionId,
    canonical: &[Message],
) {
    let matching: Vec<_> = canonical
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            matches!(message,
            Message::User(user)
                if user.transcript_role == TranscriptUserRole::Conversational
                    && user.identity.interaction_id.as_ref().is_some_and(|id|
                        id.0.to_string() == accepted.interaction_id))
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "one committed human input per interaction"
    );
    let (offset, message) = matching[0];
    let Message::User(user) = message else {
        unreachable!()
    };
    assert_eq!(user.text_content(), HUMAN);
    assert_eq!(frame.kind, "user_input");
    assert_eq!(frame.source.kind, ConsoleFrameSourceKind::SessionHistory);
    assert_eq!(
        frame.session_id.as_deref(),
        Some(session_id.to_string().as_str())
    );
    assert_eq!(
        frame.interaction_id.as_deref(),
        Some(accepted.interaction_id.as_str())
    );
    assert_eq!(
        frame.run_id,
        user.identity.run_id.as_ref().map(|id| id.0.to_string())
    );
    assert!(
        frame.run_id.is_some(),
        "committed human input retains its runtime run"
    );
    assert_eq!(
        frame.source.source_cursor,
        Some(format!("{session_id}:{offset}"))
    );
    assert_eq!(
        frame.payload["message"],
        serde_json::to_value(message).unwrap()
    );
    assert_eq!(
        frame.payload["content"],
        serde_json::to_value(&user.content).unwrap()
    );
}

fn assert_human_input_source_pair(
    frames: &[ConsoleFrame],
    accepted: &ConsoleInteractionAccepted,
    session_id: &SessionId,
    canonical: &[Message],
) -> ConsoleFrame {
    let inputs: Vec<_> = frames
        .iter()
        .filter(|frame| frame.kind == "user_input")
        .collect();
    assert_eq!(
        inputs.len(),
        2,
        "exactly one admission and one canonical source: {frames:#?}"
    );
    let reservations: Vec<_> = inputs
        .iter()
        .copied()
        .filter(|frame| frame.source.kind == ConsoleFrameSourceKind::Send)
        .collect();
    assert_eq!(reservations.len(), 1);
    let reserved = reservations[0];
    assert_eq!(reserved.id, accepted.input_frame_id);
    assert_eq!(reserved.cursor, accepted.cursor);
    assert_eq!(
        reserved.session_id.as_deref(),
        Some(session_id.to_string().as_str())
    );
    assert_eq!(
        reserved.interaction_id.as_deref(),
        Some(accepted.interaction_id.as_str())
    );
    assert_eq!(reserved.payload["content"], HUMAN);
    let history: Vec<_> = inputs
        .iter()
        .copied()
        .filter(|frame| frame.source.kind == ConsoleFrameSourceKind::SessionHistory)
        .collect();
    assert_eq!(history.len(), 1);
    assert_canonical_human_frame(history[0], accepted, session_id, canonical);
    history[0].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn console_human_is_canonical_once_and_generic_identity_work_stays_external_event() {
    let h = Harness::new(WAIT).await;
    let request = h.request("canonical", HUMAN);
    let accepted = h.send(request.clone()).await.unwrap();
    h.finish_human(&accepted, HUMAN).await;
    let history = h.history().await;
    let interaction =
        meerkat_core::interaction::InteractionId(accepted.interaction_id.parse().unwrap());
    let users: Vec<_> = history
        .iter()
        .filter_map(|message| match message {
            Message::User(user) if user.identity.interaction_id == Some(interaction) => Some(user),
            _ => None,
        })
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].text_content(), HUMAN);
    assert_eq!(users[0].transcript_role, TranscriptUserRole::Conversational);
    assert!(history.iter().any(|message| matches!(message,
        Message::BlockAssistant(answer) if answer.identity.interaction_id == Some(interaction))));

    let calls = h.client.requests.lock().unwrap().len();
    let replay = h.send(request.clone()).await.unwrap();
    assert_eq!(replay.input_frame_id, accepted.input_frame_id);
    assert_eq!(h.client.requests.lock().unwrap().len(), calls);
    let mut changed = request;
    changed.content = json!("changed user intent");
    assert!(matches!(
        h.send(changed).await,
        Err(crate::console_aggregator::ConsoleSendError::IdempotencyConflict(_))
    ));

    h.aggregator.refresh_session_history().await.unwrap();
    let page = h
        .aggregator
        .query_timeline(ConsoleTimelineQuery {
            identity: Some(h.identity.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_human_input_source_pair(&page.frames, &accepted, &h.session_id, &history);

    h.identity_runtime
        .send(
            &h.identity,
            &ContentInput::Text("GENERIC_SYSTEM_WORK".to_string()),
        )
        .await
        .unwrap();
    let generic = tokio::time::timeout(WAIT, async {
        loop {
            let messages = h.history().await;
            if messages.iter().any(|message| {
                matches!(message, Message::SystemNotice(_))
                    && serde_json::to_string(message)
                        .unwrap()
                        .contains("GENERIC_SYSTEM_WORK")
            }) {
                break messages;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(generic.iter().any(|message| {
        matches!(message, Message::SystemNotice(_))
            && serde_json::to_string(message)
                .unwrap()
                .contains("GENERIC_SYSTEM_WORK")
    }));
    assert!(!generic.iter().any(|message|
        matches!(message, Message::User(user) if user.text_content() == "GENERIC_SYSTEM_WORK")));
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn console_human_recall_stays_canonical_context_not_refreshed_or_paged_human_input() {
    const MEMORY: &str = "Recalled reference: the cobalt sample is stored in locker seven.";
    struct RecallProvider;
    #[async_trait::async_trait]
    impl AgentMemoryProvider for RecallProvider {
        async fn recall(
            &self,
            _request: AgentMemoryRecallRequest,
        ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
            Ok(vec![AgentMemoryRecord {
                memory_id: "cobalt-location".to_string(),
                title: "Cobalt reference".to_string(),
                body: MEMORY.to_string(),
                tags: Vec::new(),
                created_at_ms: 1,
                updated_at_ms: 1,
            }])
        }
    }
    let h = Harness::new(WAIT).await;
    h.identity_runtime
        .set_agent_memory(Some(AgentMemoryRuntimeInjector::new(
            Arc::new(RecallProvider),
            AgentMemoryConfig {
                per_turn_injection: AgentMemoryPerTurnInjection::Budgeted,
                ..Default::default()
            },
        )))
        .await;
    let accepted = h.send(h.request("recall-projection", HUMAN)).await.unwrap();
    let interaction =
        meerkat_core::interaction::InteractionId(accepted.interaction_id.parse().unwrap());
    // Read the exact prepared slot from the real session, not another recall:
    // the completion-bearing replay must match the originally admitted input.
    let injected = tokio::time::timeout(WAIT, async {
        loop {
            let injected: Vec<_> = h
                .history()
                .await
                .into_iter()
                .filter_map(|message| match message {
                    Message::User(user)
                        if user.identity.interaction_id == Some(interaction)
                            && user.transcript_role == TranscriptUserRole::InjectedContext =>
                    {
                        Some(ContentInput::Text(user.text_content()))
                    }
                    _ => None,
                })
                .collect();
            if !injected.is_empty() {
                break injected;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("configured recall must reach the canonical injected-context slot");
    assert!(serde_json::to_string(&injected).unwrap().contains(MEMORY));
    h.finish_human_with_context(&accepted, HUMAN, HandlingMode::Queue, injected)
        .await;

    let canonical = h.history().await;
    let mut retained_canonical = None;
    for _ in 0..2 {
        h.aggregator.refresh_session_history().await.unwrap();
        let full = h
            .aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some(h.identity.to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        let source =
            assert_human_input_source_pair(&full.frames, &accepted, &h.session_id, &canonical);
        if let Some(previous) = &retained_canonical {
            assert_eq!(
                &source, previous,
                "refresh cannot duplicate or rewrite the canonical row"
            );
        } else {
            retained_canonical = Some(source);
        }
        assert!(
            !serde_json::to_string(&full.frames)
                .unwrap()
                .contains(MEMORY)
        );
    }
    let retained_canonical = retained_canonical.unwrap();
    let mut cursor = accepted.cursor.clone();
    let mut reached_end = false;
    let mut paged_inputs = Vec::new();
    for _ in 0..20 {
        let page = h
            .aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some(h.identity.to_string()),
                after: Some(cursor.clone()),
                limit: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        for frame in page
            .frames
            .iter()
            .filter(|frame| frame.kind == "user_input")
        {
            assert_canonical_human_frame(frame, &accepted, &h.session_id, &canonical);
            assert_eq!(frame, &retained_canonical);
            paged_inputs.push(frame.clone());
        }
        assert!(
            !serde_json::to_string(&page.frames)
                .unwrap()
                .contains(MEMORY)
        );
        let Some(last) = page.frames.last() else {
            reached_end = true;
            break;
        };
        cursor = last.cursor.clone();
    }
    assert!(
        reached_end,
        "bounded pagination must exhaust the transcript"
    );

    assert_eq!(
        paged_inputs,
        vec![retained_canonical],
        "paging retains the committed human row exactly once, without another admission or recall row"
    );

    // A fresh observer has no optimistic reservation to hide behind.
    let history_only = MetaConsoleAggregator::in_memory();
    history_only.register_runtime_handles_with_policy(
        "human",
        "",
        h.unified.mob_runtime().clone(),
        Some(h.identity_runtime.clone()),
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    history_only.refresh_session_history().await.unwrap();
    let page = history_only
        .query_timeline(ConsoleTimelineQuery {
            identity: Some(h.identity.to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        page.frames
            .iter()
            .filter(|frame| frame.kind == "user_input")
            .count(),
        1
    );
    assert_canonical_human_frame(
        page.frames
            .iter()
            .find(|frame| frame.kind == "user_input")
            .unwrap(),
        &accepted,
        &h.session_id,
        &canonical,
    );
    assert!(
        !serde_json::to_string(&page.frames)
            .unwrap()
            .contains(MEMORY)
    );

    let canonical = h.history().await;
    assert!(canonical.iter().any(|message| matches!(message,
        Message::User(user) if user.transcript_role == TranscriptUserRole::InjectedContext
            && user.identity.interaction_id == Some(interaction)
            && user.text_content().contains(MEMORY))));
    assert_eq!(canonical.iter().filter(|message| matches!(message,
        Message::User(user) if user.transcript_role == TranscriptUserRole::Conversational
            && user.identity.interaction_id == Some(interaction) && user.text_content() == HUMAN
    )).count(), 1);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn console_human_timeout_replay_preserves_failed_receipt_without_repreparing() {
    let h = Harness::new(Duration::ZERO).await;
    let request = h.request("unknown", HUMAN);
    let accepted = h.send(request.clone()).await.unwrap();
    h.wait_status(&accepted, ConsoleFrameStatus::DeliveryFailed)
        .await;
    let replay = h.send(request).await.unwrap_err();
    let data = super::console_send_error_data(&replay).unwrap();
    assert_eq!(data["kind"], "host_human_input_replay_unavailable");
    assert_eq!(data["input_frame_id"], accepted.input_frame_id);
    assert_eq!(data["execution_fate"], "unknown");
    assert_eq!(data["retry_submitted"], false);
    assert!(h.client.requests.lock().unwrap().is_empty());
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn console_human_lost_admission_observation_does_not_reexecute_committed_work() {
    let h = Harness::new(WAIT).await;
    let request = h.request("lost-response", HUMAN);
    let accepted = h.send(request.clone()).await.unwrap();
    h.finish_human(&accepted, HUMAN).await;
    let calls = h.client.requests.lock().unwrap().len();
    // The runtime really committed. Only the console's observation is lost;
    // mark its existing receipt failed as the timeout path does.
    h.aggregator
        .mark_interaction_delivery_failed(&accepted.input_frame_id)
        .await
        .unwrap();
    let replay = h.send(request).await.unwrap_err();
    assert_eq!(
        super::console_send_error_data(&replay).unwrap()["kind"],
        "host_human_input_replay_unavailable",
    );
    assert_eq!(h.client.requests.lock().unwrap().len(), calls);
    assert_eq!(
        h.history()
            .await
            .iter()
            .filter(
                |message| matches!(message, Message::User(user) if user.text_content() == HUMAN)
            )
            .count(),
        1
    );
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn console_human_bridge_slots_strict_replay_and_stale_session_refusal() {
    let h = Harness::new(WAIT).await;
    let interaction = uuid::Uuid::new_v4();
    let mut delivery = BridgeDelivery::new(HUMAN.into(), HandlingMode::Queue);
    delivery.interaction_id = Some(interaction.to_string());
    delivery.delivery_identity =
        Some(MobDeliveryIdentity::new("slots", interaction.to_string()).unwrap());
    delivery.system_prompt = Some("PER_TURN_SYSTEM".to_string());
    delivery.injected_context = vec!["AMBIENT_MEMORY".into()];
    h.bridge
        .deliver_host_human_input(&h.runtime_id, &h.session_id, delivery.clone())
        .await
        .unwrap();
    let member = h
        .unified
        .mob_handle()
        .get_member(&crate::member_comms_id::mob_member_id(h.identity.as_str()))
        .await
        .unwrap()
        .unwrap();
    let spec = WorkSpec::new(HUMAN, WorkOrigin::Internal)
        .with_system_prompt("PER_TURN_SYSTEM")
        .with_injected_context(vec!["AMBIENT_MEMORY".into()])
        .with_interaction_id(meerkat_core::interaction::InteractionId(interaction));
    let turn = h
        .unified
        .mob_handle()
        .start_host_human_input_bounded(
            member.agent_runtime_id,
            member.fence_token,
            spec,
            HandlingMode::Queue,
            delivery.delivery_identity.clone().unwrap(),
            std::time::Instant::now() + WAIT,
        )
        .await
        .unwrap();
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .unwrap()
        .unwrap();
    let history = h.history().await;
    assert!(history.iter().any(|message| matches!(message,
        Message::User(user) if user.transcript_role == TranscriptUserRole::InjectedContext
            && user.text_content() == "AMBIENT_MEMORY")));
    assert!(history.iter().any(|message| matches!(message,
        Message::System(system) if system.content == "PER_TURN_SYSTEM")));
    let calls_after_turn = h.client.requests.lock().unwrap().len();
    h.bridge
        .deliver_host_human_input(&h.runtime_id, &h.session_id, delivery.clone())
        .await
        .unwrap();
    delivery.injected_context = vec!["CHANGED_MEMORY".into()];
    assert!(matches!(
        h.bridge.deliver_host_human_input(&h.runtime_id, &h.session_id, delivery.clone()).await,
        Err(BridgeError::HostHumanInput(HostHumanInputError::Mob(error)))
            if matches!(*error, meerkat_mob::MobError::WorkInputIdempotencyConflict { .. })
    ));
    assert!(matches!(
        h.bridge
            .deliver_host_human_input(&h.runtime_id, &SessionId::new(), delivery.clone())
            .await,
        Err(BridgeError::HostHumanInput(
            HostHumanInputError::BindingChanged
        ))
    ));
    delivery.handling_mode = HandlingMode::Steer;
    let interaction = uuid::Uuid::new_v4();
    delivery.interaction_id = Some(interaction.to_string());
    delivery.delivery_identity =
        Some(MobDeliveryIdentity::new("steer-slots", interaction.to_string()).unwrap());
    let refusal = h
        .bridge
        .deliver_host_human_input(&h.runtime_id, &h.session_id, delivery.clone())
        .await;
    assert!(
        matches!(
                &refusal,
                Err(BridgeError::HostHumanInput(HostHumanInputError::Mob(error)))
                    if matches!(error.as_ref(), meerkat_mob::MobError::InjectedContextUndeliverable { .. })
        ),
        "{refusal:?}"
    );
    delivery.injected_context.clear();
    let refusal = h
        .bridge
        .deliver_host_human_input(&h.runtime_id, &h.session_id, delivery)
        .await;
    assert!(
        matches!(
        &refusal,
        Err(BridgeError::HostHumanInput(HostHumanInputError::Mob(error)))
                if matches!(error.as_ref(), meerkat_mob::MobError::UnsupportedForMode { .. })
        ),
        "{refusal:?}"
    );
    assert_eq!(h.client.requests.lock().unwrap().len(), calls_after_turn);
    assert_eq!(
        h.history()
            .await
            .iter()
            .filter(
                |message| matches!(message, Message::User(user) if user.text_content() == HUMAN)
            )
            .count(),
        1
    );
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn console_human_member_only_worker_uses_the_same_canonical_user_seam() {
    let h = Harness::new(WAIT).await;
    h.unified
        .mob_handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
            "human".to_string(),
            "worker".to_string(),
            None,
            None,
            None,
        ))
        .await
        .unwrap();
    let request = ConsoleSendRequest {
        identity: "worker".to_string(),
        ..h.request("worker", HUMAN)
    };
    let accepted = h.aggregator.send(request.clone()).await.unwrap();
    h.finish_human(&accepted, HUMAN).await;
    let messages = h
        .unified
        .mob_runtime()
        .read_session_history(accepted.session_id.as_deref().unwrap(), 0, None)
        .await
        .unwrap()
        .messages;
    assert_eq!(
        messages
            .iter()
            .filter(
                |message| matches!(message, Message::User(user) if user.text_content() == HUMAN)
            )
            .count(),
        1
    );
    let replay = h.aggregator.send(request).await.unwrap();
    assert_eq!(replay.input_frame_id, accepted.input_frame_id);
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn console_human_active_steer_stays_steer_and_commits_once() {
    let h = Harness::with_kickoff(WAIT, true).await;
    let calls_before = h.client.requests.lock().unwrap().len();
    h.client.gate.send_replace(false);
    let active = h
        .send(h.request("active", "active queue work"))
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        while h.client.requests.lock().unwrap().len() <= calls_before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let request = ConsoleSendRequest {
        handling_mode: Some("steer".to_string()),
        origin_kind: None,
        ..h.request("steer", HUMAN)
    };
    let steer = tokio::time::timeout(WAIT, h.send(request.clone()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !*h.client.gate.borrow(),
        "admission cannot wait for the active LLM"
    );
    h.client.gate.send_replace(true);
    h.finish_human(&active, "active queue work").await;
    h.finish_human_with_mode(&steer, HUMAN, HandlingMode::Steer)
        .await;
    assert_eq!(
        h.history()
            .await
            .iter()
            .filter(
                |message| matches!(message, Message::User(user) if user.text_content() == HUMAN)
            )
            .count(),
        1
    );
    assert_eq!(
        h.send(request).await.unwrap().input_frame_id,
        steer.input_frame_id
    );
    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn console_human_reset_refuses_old_prepared_session_and_old_worker_fence() {
    let h = Harness::new(WAIT).await;
    let request = h.request("before-reset", HUMAN);
    let accepted = h.send(request.clone()).await.unwrap();
    h.finish_human(&accepted, HUMAN).await;
    let old_member = h
        .unified
        .mob_handle()
        .list_members()
        .await
        .into_iter()
        .find(|member| member.agent_identity.as_str() == h.identity.as_str())
        .unwrap();
    let successor = h.identity_runtime.reset(&h.identity).await.unwrap();
    let successor_session = successor
        .session_id()
        .cloned()
        .expect("the successor is bound to a session");
    assert_ne!(successor_session, h.session_id);
    let replay = h.send(request).await.unwrap();
    assert_eq!(
        replay.session_id.as_deref(),
        Some(h.session_id.to_string().as_str())
    );
    let mut delivery = BridgeDelivery::new(HUMAN.into(), HandlingMode::Queue);
    delivery.interaction_id = Some(accepted.interaction_id.clone());
    delivery.delivery_identity =
        Some(MobDeliveryIdentity::new(&accepted.input_frame_id, &accepted.interaction_id).unwrap());
    assert!(matches!(
        h.bridge
            .deliver_host_human_input(&h.runtime_id, &h.session_id, delivery)
            .await,
        Err(BridgeError::HostHumanInput(
            HostHumanInputError::BindingChanged
        ))
    ));
    assert!(
        crate::mob_handle_runtime::send_console_human_on_mob(
            &h.unified.mob_handle(),
            &old_member,
            HUMAN.into(),
            &[],
            HandlingMode::Queue,
            &accepted,
        )
        .await
        .is_err()
    );
    let replacement_history = h
        .unified
        .mob_runtime()
        .read_session_history(&successor_session.to_string(), 0, None)
        .await
        .unwrap();
    assert!(
        !replacement_history
            .messages
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == HUMAN))
    );
    h.stop().await;
}

/// A send accepted in the identity's session whose dispatch never ran (the
/// gateway stopped in between), and an identity that moved to a new session
/// before the replay: the replay dispatches it again only into the session
/// it recorded, which refuses it. It ends `delivery_failed`, and neither
/// session receives it.
#[tokio::test(flavor = "multi_thread")]
async fn console_human_redispatch_into_a_moved_session_fails_and_delivers_nothing() {
    const MOVED: &str = "MOVED-SESSION a send interrupted before its admission";
    let h = Harness::new(WAIT).await;
    let request = h.request("moved-session", MOVED);
    let accepted = match h
        .aggregator
        .reserve_identity_first_interaction_scoped(
            request.clone(),
            Some(h.session_id.to_string().as_str()),
            &crate::console_aggregator::ConsoleSendScope::Anonymous,
        )
        .await
        .unwrap()
    {
        // The claim drops here, as it does when the process stops.
        crate::console_aggregator::ScopedReservation::Fresh(accepted, _claim) => Some(accepted),
        _ => None,
    }
    .expect("a fresh reservation");
    assert_eq!(
        accepted.session_id.as_deref(),
        Some(h.session_id.to_string().as_str())
    );
    let successor = h.identity_runtime.reset(&h.identity).await.unwrap();
    let successor_session = successor
        .session_id()
        .cloned()
        .expect("the successor is bound to a session");
    assert_ne!(successor_session, h.session_id);

    let replay = h.send(request).await.unwrap();
    assert_eq!(replay.input_frame_id, accepted.input_frame_id);
    let settled = tokio::time::timeout(WAIT, async {
        loop {
            let page = h
                .aggregator
                .query_timeline(ConsoleTimelineQuery {
                    identity: Some(accepted.identity.clone()),
                    ..Default::default()
                })
                .await
                .unwrap();
            if let Some(status) = page
                .frames
                .iter()
                .find(|frame| frame.id == accepted.input_frame_id)
                .map(|frame| frame.status)
                .filter(|status| {
                    matches!(
                        status,
                        ConsoleFrameStatus::Delivered | ConsoleFrameStatus::DeliveryFailed
                    )
                })
            {
                break status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the replayed send settles");
    assert_eq!(
        settled,
        ConsoleFrameStatus::DeliveryFailed,
        "a send recorded in a session the identity left is refused there, never retargeted"
    );
    let holds_the_send = |history: &meerkat_core::service::SessionHistoryPage| {
        history
            .messages
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == MOVED))
    };
    // A retargeted send would land in the successor: its history must read,
    // so this cannot pass on a read error.
    let successor_history = h
        .unified
        .mob_runtime()
        .read_session_history(&successor_session.to_string(), 0, None)
        .await
        .expect("the successor session's history reads");
    assert!(
        !holds_the_send(&successor_history),
        "the successor session received the send"
    );
    // The session the send recorded refused it (delivery_failed above). The
    // reset retired that session, so its history is checked when it still
    // reads.
    if let Ok(history) = h
        .unified
        .mob_runtime()
        .read_session_history(&h.session_id.to_string(), 0, None)
        .await
    {
        assert!(
            !holds_the_send(&history),
            "the recorded session received the send"
        );
    }
    assert!(
        h.client
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| !format!("{request:?}").contains("MOVED-SESSION")),
        "the model never saw the send"
    );
    h.stop().await;
}

const SKILL_BODY: &str = "Answer in the chosen house style: numbered, terse, no hedging.";
const OTHER_SKILL_BODY: &str = "Answer in the other house style: long prose with caveats.";

/// A meerkat config whose skill engine serves two filesystem repositories,
/// each holding a skill named `house-style` with a different body. Returns
/// the config and the (chosen, other) source-pinned keys.
fn house_style_skill_config(root: &std::path::Path) -> (meerkat::Config, SkillKey, SkillKey) {
    let name = SkillName::parse("house-style").unwrap();
    let mut config = meerkat::Config::default();
    let mut keys = Vec::new();
    for (index, (source, body)) in [
        ("5b0c7c2e-4c61-4f2d-9d0e-2f5a8c6b1e11", SKILL_BODY),
        ("9e4d2a10-7b3c-4c8e-a1f5-6d2b0c9e8f77", OTHER_SKILL_BODY),
    ]
    .into_iter()
    .enumerate()
    {
        let source_uuid = SourceUuid::parse(source).unwrap();
        let repository = root.join(format!("skills-{index}"));
        let skill_dir = repository.join(name.as_str());
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: fixture skill\n---\n{body}\n"),
        )
        .unwrap();
        config
            .skills
            .repositories
            .push(meerkat_core::skills_config::SkillRepositoryConfig {
                name: format!("fixture-skills-{index}"),
                source_uuid: source_uuid.clone(),
                transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                    path: repository.to_string_lossy().into_owned(),
                },
            });
        keys.push(SkillKey::new(source_uuid, name.clone()));
    }
    let other = keys.pop().unwrap();
    let chosen = keys.pop().unwrap();
    (config, chosen, other)
}

/// The durable `SkillContext` rows committed for one console interaction.
fn skill_context_rows(messages: &[Message], interaction_id: &str) -> Vec<(SkillKey, String)> {
    let interaction =
        meerkat_core::interaction::InteractionId(interaction_id.parse::<uuid::Uuid>().unwrap());
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) if user.identity.interaction_id == Some(interaction) => Some(user),
            _ => None,
        })
        .flat_map(|user| user.content.iter())
        .filter_map(|block| match block {
            meerkat_core::types::ContentBlock::SkillContext { skill_key, text } => {
                Some((skill_key.clone(), text.clone()))
            }
            _ => None,
        })
        .collect()
}

/// ConsoleSend carries a source-pinned selection to the exact direct target.
/// Two sources host a skill with the same name; the key picks exactly one.
/// The member resolves it natively (typed `SkillsResolved`, durable
/// `SkillContext`, the selected body and only that body in the provider
/// request). The response is then lost and the send replayed: the replay
/// returns the original acceptance with no second provider call. A resend
/// under the same request key with a changed selection is a fingerprint
/// conflict.
#[tokio::test(flavor = "multi_thread")]
async fn console_send_selected_skills_resolve_natively_and_replay_exactly() {
    use futures::StreamExt;

    let skills = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let (config, chosen, other) = house_style_skill_config(skills.path());
    let h = Harness::with_meerkat_config(config).await;
    let mut events = h
        .unified
        .mob_handle()
        .subscribe_agent_events(&crate::member_comms_id::mob_member_id(h.identity.as_str()))
        .await
        .unwrap();
    let request = ConsoleSendRequest {
        skill_refs: vec![SkillRef::Structured(chosen.clone())],
        ..h.request("selected-skill", HUMAN)
    };
    let accepted = h.send(request.clone()).await.unwrap();
    h.finish_human_with_skills(&accepted, HUMAN, vec![chosen.clone()])
        .await;

    let mut resolved = Vec::new();
    tokio::time::timeout(WAIT, async {
        while let Some(envelope) = events.next().await {
            match envelope.payload {
                meerkat_core::AgentEvent::SkillsResolved { skills, .. } => resolved.push(skills),
                meerkat_core::AgentEvent::SkillResolutionFailed { reason, .. } => {
                    panic!("selected skill must resolve, got {reason}")
                }
                // The member's own bootstrap turn may still be draining
                // from this subscription; the selected turn ends at the
                // first completion after its activation.
                meerkat_core::AgentEvent::RunCompleted { .. } if !resolved.is_empty() => break,
                _ => {}
            }
        }
    })
    .await
    .expect("the selected turn's events arrive");
    assert_eq!(
        resolved,
        vec![vec![chosen.clone()]],
        "one native activation"
    );

    let rows = skill_context_rows(&h.history().await, &accepted.interaction_id);
    assert_eq!(rows.len(), 1, "exactly one durable SkillContext");
    assert_eq!(rows[0].0, chosen);
    assert!(rows[0].1.contains(SKILL_BODY), "native rendered skill body");
    let requests = h.client.requests.lock().unwrap().clone();
    let bodies = requests
        .iter()
        .map(|request| serde_json::to_string(&request.messages).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        bodies
            .iter()
            .filter(|body| body.contains(SKILL_BODY))
            .count(),
        1,
        "exactly one provider call carries the selected body"
    );
    assert!(
        bodies.iter().all(|body| !body.contains(OTHER_SKILL_BODY)),
        "the same-name skill is never selected"
    );
    let calls_after_turn = requests.len();

    // A history refresh projects the member's committed user messages (its
    // spawn kickoff among them) as user_input frames of their own, which can
    // land at any read. The replay is checked against the console's own
    // reservation: the one input this row's send reserved.
    h.aggregator.refresh_session_history().await.unwrap();
    tokio::time::timeout(WAIT, h.aggregator.history_backfill_converged())
        .await
        .expect("the console's history backfills converge");
    let frames_before_replay = h.reserved_inputs(&accepted.identity).await;
    assert_eq!(
        frames_before_replay,
        vec![(
            accepted.input_frame_id.clone(),
            ConsoleFrameStatus::Delivered
        )],
        "the send reserved one console input"
    );

    // The response is lost; the caller replays the same send.
    let replay = h.send(request.clone()).await.unwrap();
    assert_eq!(replay.input_frame_id, accepted.input_frame_id);
    assert_eq!(replay.interaction_id, accepted.interaction_id);

    let changed = ConsoleSendRequest {
        skill_refs: vec![SkillRef::Structured(other)],
        ..request
    };
    assert!(matches!(
        h.send(changed).await,
        Err(crate::console_aggregator::ConsoleSendError::IdempotencyConflict(_))
    ));

    // Neither the replay nor the refused resend reserved or dispatched a new
    // console input, and the original turn is still the only one: observing
    // its completion again through the same delivery identity resolves to
    // the committed turn without another provider call or SkillContext.
    assert_eq!(
        h.reserved_inputs(&accepted.identity).await,
        frames_before_replay,
        "no new console input was reserved or dispatched"
    );
    h.finish_human_with_skills(&accepted, HUMAN, vec![chosen.clone()])
        .await;
    assert_eq!(
        skill_context_rows(&h.history().await, &accepted.interaction_id).len(),
        1
    );
    assert_eq!(h.client.requests.lock().unwrap().len(), calls_after_turn);
    h.stop().await;
}

/// The member-only lane (a worker without an identity-first record) carries
/// the same selection through the same fenced host-human seam.
#[tokio::test(flavor = "multi_thread")]
async fn console_send_selected_skills_reach_a_member_only_worker() {
    let skills = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let (config, key, _) = house_style_skill_config(skills.path());
    let h = Harness::with_meerkat_config(config).await;
    h.unified
        .mob_handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
            "human".to_string(),
            "worker".to_string(),
            None,
            None,
            None,
        ))
        .await
        .unwrap();
    let request = ConsoleSendRequest {
        identity: "worker".to_string(),
        skill_refs: vec![SkillRef::Structured(key.clone())],
        ..h.request("worker-skill", HUMAN)
    };
    let accepted = h.aggregator.send(request.clone()).await.unwrap();
    h.finish_human_with_skills(&accepted, HUMAN, vec![key.clone()])
        .await;
    let messages = h
        .unified
        .mob_runtime()
        .read_session_history(accepted.session_id.as_deref().unwrap(), 0, None)
        .await
        .unwrap()
        .messages;
    let rows = skill_context_rows(&messages, &accepted.interaction_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, key);
    assert!(rows[0].1.contains(SKILL_BODY));
    let frames_before_replay = h.reserved_inputs(&accepted.identity).await;
    let replay = h.aggregator.send(request).await.unwrap();
    assert_eq!(replay.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        h.reserved_inputs(&accepted.identity).await,
        frames_before_replay,
        "the replay reserved nothing new"
    );
    // Nor did it dispatch anything: observing the turn again through its
    // delivery identity resolves to the committed one, and the worker's
    // history still holds exactly one SkillContext for the interaction.
    h.finish_human_with_skills(&accepted, HUMAN, vec![key.clone()])
        .await;
    let messages = h
        .unified
        .mob_runtime()
        .read_session_history(accepted.session_id.as_deref().unwrap(), 0, None)
        .await
        .unwrap()
        .messages;
    assert_eq!(
        skill_context_rows(&messages, &accepted.interaction_id).len(),
        1,
        "the replay dispatched nothing new"
    );
    h.stop().await;
}

#[test]
fn console_send_request_skill_refs_are_typed_and_optional() {
    let bare: ConsoleSendRequest = serde_json::from_value(json!({
        "identity": "human",
        "content": "hello",
        "origin": "console",
        "idempotency_key": "k",
    }))
    .unwrap();
    assert!(bare.skill_refs.is_empty());
    assert!(
        serde_json::to_value(&bare)
            .unwrap()
            .get("skill_refs")
            .is_none()
    );

    let typed: ConsoleSendRequest = serde_json::from_value(json!({
        "identity": "human",
        "content": "hello",
        "origin": "console",
        "idempotency_key": "k",
        "skill_refs": [{
            "kind": "structured",
            "source_uuid": "5b0c7c2e-4c61-4f2d-9d0e-2f5a8c6b1e11",
            "skill_name": "house-style",
        }],
    }))
    .unwrap();
    assert_eq!(
        typed.selected_skill_keys(),
        vec![SkillKey::new(
            SourceUuid::parse("5b0c7c2e-4c61-4f2d-9d0e-2f5a8c6b1e11").unwrap(),
            SkillName::parse("house-style").unwrap(),
        )]
    );

    assert!(
        serde_json::from_value::<ConsoleSendRequest>(json!({
            "identity": "human",
            "content": "hello",
            "origin": "console",
            "idempotency_key": "k",
            "skill_refs": ["house-style"],
        }))
        .is_err(),
        "untyped legacy skill strings are refused, not folded"
    );
}

fn dormant_spec(identity: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(identity).unwrap(),
        profile: "human".into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
        backend: None,
        binding: None,
        placement: None,
    }
}

fn selected_skill() -> SkillRef {
    SkillRef::Structured(SkillKey::new(
        SourceUuid::parse("5b0c7c2e-4c61-4f2d-9d0e-2f5a8c6b1e11").unwrap(),
        SkillName::parse("house-style").unwrap(),
    ))
}

/// An identity-first target without a native host-human skill carrier (an
/// external backend, or a remote placement) refuses a nonempty selection
/// typed BEFORE the interaction is reserved: no reservation, no frame.
#[tokio::test(flavor = "multi_thread")]
async fn console_send_selected_skills_refused_before_reservation_for_external_or_placed_identity() {
    let h = Harness::new(WAIT).await;
    let mut external = dormant_spec("external-target");
    external.backend = Some(meerkat_mob::MobBackendKind::External);
    let mut placed = dormant_spec("placed-target");
    placed.placement = Some(meerkat_contracts::WireHostRef("remote-host".to_string()));
    for spec in [external, placed] {
        let identity = spec.identity.to_string();
        h.identity_runtime.register(spec).await;
        let request = ConsoleSendRequest {
            identity: identity.clone(),
            skill_refs: vec![selected_skill()],
            ..h.request("refused-selection", HUMAN)
        };
        let refusal = h.send(request).await;
        assert!(
            matches!(
                &refusal,
                Err(crate::console_aggregator::ConsoleSendError::InvalidRequest(
                    _
                ))
            ),
            "{identity}: {refusal:?}"
        );
        let frames = h
            .aggregator
            .query_timeline(ConsoleTimelineQuery {
                identity: Some(identity.clone()),
                ..Default::default()
            })
            .await
            .unwrap()
            .frames;
        assert!(
            frames.is_empty(),
            "{identity}: nothing was reserved: {frames:?}"
        );
    }
    h.stop().await;
}

/// On the member-only lane, meerkat's host-human admission is the refusal
/// point for targets the roster cannot classify cheaply (remote placement,
/// external backends). A typed admission refusal of a selection-carrying
/// send surfaces typed on the console: the input frame is `delivery_failed`
/// and the failure frame carries `data.kind = "host_human_input_unsupported"`.
/// The refusal here is meerkat's refusal of a host-human Steer during an
/// unresolved kickoff, which travels the same admission and mapping path.
#[tokio::test(flavor = "multi_thread")]
async fn console_send_selected_skills_member_lane_admission_refusal_surfaces_typed() {
    let h = Harness::new(WAIT).await;
    let calls_before = h.client.requests.lock().unwrap().len();
    h.client.gate.send_replace(false);
    h.unified
        .mob_handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
            "human".to_string(),
            "worker".to_string(),
            Some("worker kickoff".into()),
            None,
            None,
        ))
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        while h.client.requests.lock().unwrap().len() <= calls_before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the worker kickoff reaches the provider");
    let request = ConsoleSendRequest {
        identity: "worker".to_string(),
        handling_mode: Some("steer".to_string()),
        skill_refs: vec![selected_skill()],
        ..h.request("member-lane-refusal", HUMAN)
    };
    let accepted = h.aggregator.send(request).await.unwrap();
    h.wait_status(&accepted, ConsoleFrameStatus::DeliveryFailed)
        .await;
    let frames = h
        .aggregator
        .query_timeline(ConsoleTimelineQuery {
            identity: Some(accepted.identity.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .frames;
    let failure = frames
        .iter()
        .find(|frame| {
            frame.kind == "message_delivery_failed"
                && frame.parent_frame_id.as_deref() == Some(accepted.input_frame_id.as_str())
        })
        .expect("a typed delivery failure frame");
    assert_eq!(
        failure.payload["data"]["kind"], "host_human_input_unsupported",
        "{:?}",
        failure.payload
    );
    h.client.gate.send_replace(true);
    h.stop().await;
}

/// A governed host's Console send: refused before anything is reserved for
/// a caller who is not a verified human, and otherwise carried only to the
/// host's binder for that human (never to the ungoverned human-input path);
/// a member with no governed admission refuses it, and the model sees
/// nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_console_send_reaches_only_the_hosts_binder_for_the_verified_human() {
    use crate::console_governed_ingress::ConsoleGovernedIngress;
    use crate::runtime::AuthenticatedConsoleHuman;

    struct Unreached;

    #[async_trait::async_trait]
    impl GovernedInputBinder for Unreached {
        async fn bind(
            &self,
            _context: &GovernedBindContext,
            _input: meerkat_runtime::Input,
        ) -> Result<BoundGovernedInput, String> {
            Err("this member has no governed admission to bind for".to_string())
        }

        fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {}
    }

    struct Host(Mutex<Vec<(String, String)>>);

    impl ConsoleGovernedIngress for Host {
        fn governs(&self, _identity: &AgentIdentity) -> bool {
            true
        }

        fn binder_for(
            &self,
            identity: &AgentIdentity,
            human: &AuthenticatedConsoleHuman,
        ) -> Result<Arc<dyn GovernedInputBinder>, String> {
            self.0
                .lock()
                .unwrap()
                .push((identity.to_string(), human.subject().to_string()));
            Ok(Arc::new(Unreached))
        }
    }

    let h = Harness::new(WAIT).await;
    let host = Arc::new(Host(Mutex::default()));
    let send = |key: &'static str, human: Option<AuthenticatedConsoleHuman>| {
        let governance = super::ConsoleSendGovernance {
            ingress: Some(host.clone()),
            human,
            ..Default::default()
        };
        let request = h.request(key, HUMAN);
        let aggregator = h.aggregator.clone();
        let runtime = h.identity_runtime.clone();
        async move {
            super::console_send_identity_first_governed(
                &aggregator,
                runtime,
                None,
                request,
                &governance,
            )
            .await
        }
    };

    let anonymous = send("governed-anonymous", None).await;
    assert!(
        matches!(
            anonymous,
            Err(crate::console_aggregator::ConsoleSendError::UnqualifiedPrincipal(_))
        ),
        "{anonymous:?}"
    );
    assert!(host.0.lock().unwrap().is_empty(), "the host is never asked");
    let reserved = h
        .aggregator
        .query_timeline(ConsoleTimelineQuery {
            identity: Some(h.identity.to_string()),
            ..Default::default()
        })
        .await
        .unwrap()
        .frames;
    assert!(
        reserved.iter().all(|frame| frame.kind != "user_input"),
        "nothing was reserved for the refused send: {reserved:?}"
    );

    let accepted = send(
        "governed-human",
        Some(AuthenticatedConsoleHuman::for_test(
            "https://issuer.example.test",
            "oidc-sub-7",
        )),
    )
    .await
    .expect("accepted for the verified human");
    h.wait_status(&accepted, ConsoleFrameStatus::DeliveryFailed)
        .await;
    assert_eq!(
        *host.0.lock().unwrap(),
        [(h.identity.to_string(), "oidc-sub-7".to_string())],
        "the host built exactly one binder, for this identity and human"
    );
    assert!(
        h.client.requests.lock().unwrap().is_empty(),
        "no ungoverned fallback reached the model"
    );
    h.stop().await;
}

/// A verified Console human whose retained token still verifies, so the
/// Console's own recheck at bind passes and the host's binder is reached.
pub(super) fn current_console_human() -> crate::runtime::AuthenticatedConsoleHuman {
    let state = super::console_human_admission_tests::state();
    super::console_human_admission_tests::authenticate(
        &state,
        json!({
            "sub": "oidc-sub-7",
            "email": super::console_human_admission_tests::BOB,
            "provider": "google_oauth",
        }),
    )
    .expect("the console authenticates the human")
    .human
    .expect("a verified human")
}

/// A host that pins each Console send to the original it reviewed, by the
/// send's client key: it records every per-send context, refuses a send
/// whose key it reviewed nothing for, and otherwise returns a binder for
/// that send. The binder must still check the bound session, the human and
/// the content at bind; this harness's bridge has no controller pin source,
/// so a governed delivery fails at the controller pin, before any bind and
/// before anything is submitted.
struct PinnedHost {
    reviewed_key: &'static str,
    contexts: Mutex<
        Vec<(
            String,
            String,
            crate::console_governed_ingress::ConsoleSendContext,
        )>,
    >,
}

impl PinnedHost {
    fn new(reviewed_key: &'static str) -> Arc<Self> {
        Arc::new(Self {
            reviewed_key,
            contexts: Mutex::default(),
        })
    }
}

impl crate::console_governed_ingress::ConsoleGovernedIngress for PinnedHost {
    fn governs(&self, _identity: &AgentIdentity) -> bool {
        true
    }

    fn binder_for(
        &self,
        _identity: &AgentIdentity,
        _human: &crate::runtime::AuthenticatedConsoleHuman,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        Err("this host binds a Console send only with its per-send context".to_string())
    }

    fn binder_for_send(
        &self,
        identity: &AgentIdentity,
        human: &crate::runtime::AuthenticatedConsoleHuman,
        send: &crate::console_governed_ingress::ConsoleSendContext,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        self.contexts.lock().unwrap().push((
            identity.to_string(),
            human.subject().to_string(),
            send.clone(),
        ));
        if send.idempotency_key != self.reviewed_key {
            return Err(format!(
                "{} is not the reviewed original",
                send.idempotency_key
            ));
        }
        Ok(Arc::new(PinnedBinder))
    }
}

struct PinnedBinder;

#[async_trait::async_trait]
impl GovernedInputBinder for PinnedBinder {
    async fn bind(
        &self,
        _context: &GovernedBindContext,
        _input: meerkat_runtime::Input,
    ) -> Result<BoundGovernedInput, String> {
        Err("this fixture member has no governed admission".to_string())
    }

    fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {}
}

/// Send through the Console's governed send path (the one `/console/send`
/// runs after its gate) as the verified human, with `host` installed.
async fn send_pinned(
    h: &Harness,
    host: &Arc<PinnedHost>,
    request: ConsoleSendRequest,
) -> Result<ConsoleInteractionAccepted, crate::console_aggregator::ConsoleSendError> {
    let governance = super::ConsoleSendGovernance {
        ingress: Some(host.clone()),
        human: Some(current_console_human()),
        ..Default::default()
    };
    super::console_send_governed_with_identity_first_fallback(
        &h.aggregator,
        h.identity_runtime.clone(),
        None,
        request,
        &governance,
    )
    .await
}

/// The host's per-send hook receives exactly the request's client key and
/// origin, once per send, before anything is reserved; an exact retry under
/// the same key reaches the hook again with the same key and replays the
/// original's outcome. Nothing reaches the model.
#[tokio::test(flavor = "multi_thread")]
async fn console_human_send_context_reaches_the_hosts_binder_with_the_request_key() {
    let h = Harness::new(WAIT).await;
    let host = PinnedHost::new("reviewed-original");

    let accepted = send_pinned(&h, &host, h.request("reviewed-original", HUMAN))
        .await
        .expect("accepted for the verified human");
    h.wait_status(&accepted, ConsoleFrameStatus::DeliveryFailed)
        .await;
    let context = (
        h.identity.to_string(),
        "oidc-sub-7".to_string(),
        crate::console_governed_ingress::ConsoleSendContext {
            idempotency_key: "reviewed-original".to_string(),
            origin: "console".to_string(),
        },
    );
    assert_eq!(
        *host.contexts.lock().unwrap(),
        std::slice::from_ref(&context),
        "the hook saw exactly the request's key and origin, once"
    );

    let retry = send_pinned(&h, &host, h.request("reviewed-original", HUMAN)).await;
    assert!(
        matches!(
            &retry,
            Err(crate::console_aggregator::ConsoleSendError::RuntimeOperation(error))
                if matches!(
                    error.as_ref(),
                    IdentityRuntimeError::HostHumanInput(
                        HostHumanInputError::ReplayUnavailable { interaction_id, input_frame_id }
                    ) if *interaction_id == accepted.interaction_id
                        && *input_frame_id == accepted.input_frame_id
                )
        ),
        "the retry replays the failed original: {retry:?}"
    );
    assert_eq!(
        *host.contexts.lock().unwrap(),
        [context.clone(), context],
        "the retry reached the hook with the same key"
    );
    assert!(h.client.requests.lock().unwrap().is_empty());
    h.stop().await;
}

/// A host that reviewed a different original refuses this send by its key
/// in the per-send hook: the send is refused before anything is reserved,
/// and nothing is admitted (the model and the member's transcript see
/// nothing).
#[tokio::test(flavor = "multi_thread")]
async fn console_human_send_pinned_to_another_original_is_refused_and_not_admitted() {
    let h = Harness::new(WAIT).await;
    let host = PinnedHost::new("reviewed-original");

    let refused = send_pinned(&h, &host, h.request("another-send", HUMAN)).await;
    assert!(
        matches!(
            &refused,
            Err(crate::console_aggregator::ConsoleSendError::GovernedRefused(reason))
                if reason == "another-send is not the reviewed original"
        ),
        "{refused:?}"
    );
    assert_eq!(
        host.contexts
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, send)| send.idempotency_key.clone())
            .collect::<Vec<_>>(),
        ["another-send"]
    );
    assert!(
        h.reserved_inputs(h.identity.as_str()).await.is_empty(),
        "nothing was reserved for the refused send"
    );
    assert!(
        h.client.requests.lock().unwrap().is_empty(),
        "nothing reached the model"
    );
    assert!(
        !h.history()
            .await
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == HUMAN)),
        "nothing was admitted to the member's transcript"
    );
    h.stop().await;
}

// --- image checks against self-hosted models ----------------------------------

/// Two self-hosted models served from one server the members never call
/// (they run on `TestClient`): the host config declares one `vision = true`
/// and leaves the other without the flag.
const SEEING_MODEL: &str = "seeing-fixture-model";
const BLIND_MODEL: &str = "blind-fixture-model";

/// A 1x1 PNG, base64.
const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn self_hosted_realm_binding(realm: &str) -> String {
    format!(
        r#"
[realm."{realm}"]
default_binding = "loop"

[realm."{realm}".backend.loop]
provider = "self_hosted"
backend_kind = "self_hosted"

[realm."{realm}".auth.loop]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{realm}".binding.loop]
backend_profile = "loop"
auth_profile = "loop"
default_model = "{SEEING_MODEL}"
provider_default = true
"#
    )
}

fn self_hosted_vision_config(mob_id: &str) -> meerkat::Config {
    let mut text = format!(
        r#"
[self_hosted]
default_model = "{SEEING_MODEL}"

[self_hosted.servers.loop]
base_url = "http://127.0.0.1:9"

[self_hosted.models."{SEEING_MODEL}"]
server = "loop"
remote_model = "{SEEING_MODEL}"
display_name = "Seeing fixture model"
family = "{SEEING_MODEL}"
vision = true

[self_hosted.models."{BLIND_MODEL}"]
server = "loop"
remote_model = "{BLIND_MODEL}"
display_name = "Blind fixture model"
family = "{BLIND_MODEL}"
"#
    );
    text.push_str(&self_hosted_realm_binding("global"));
    text.push_str(&self_hosted_realm_binding(&format!("mob.{mob_id}")));
    toml::from_str(&text).unwrap()
}

fn self_hosted_vision_definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.seeing]
model = "{SEEING_MODEL}"
provider = "self_hosted"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.seeing.tools]
comms = true

[profiles.blind]
model = "{BLIND_MODEL}"
provider = "self_hosted"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.blind.tools]
comms = true
"#
    ))
    .unwrap()
}

fn image_turn() -> ContentInput {
    ContentInput::Blocks(vec![
        meerkat_core::ContentBlock::Text {
            text: "what is in this image?".to_string(),
        },
        meerkat_core::ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: meerkat_core::types::ImageData::Inline {
                data: ONE_PIXEL_PNG.to_string(),
            },
        },
    ])
}

/// A runtime over the self-hosted host config, with a member-only worker of
/// each model: `seeing-worker` and `blind-worker`.
async fn self_hosted_vision_workers(state: &std::path::Path) -> UnifiedRuntime {
    let mob_id = format!("self-hosted-vision-{}", uuid::Uuid::new_v4());
    let unified = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(self_hosted_vision_definition(&mob_id))
            .meerkat_config(self_hosted_vision_config(&mob_id))
            .persistent_state(state)
            .comms(true)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .unwrap();
    for (profile, member) in [("seeing", "seeing-worker"), ("blind", "blind-worker")] {
        unified
            .mob_handle()
            .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
                profile.to_string(),
                member.to_string(),
                None,
                None,
                None,
            ))
            .await
            .unwrap();
    }
    unified
}

/// Core admits an image turn for a self-hosted model whose host config
/// declares `vision = true`. Meta's identity bridge refused it, because its
/// check asked only the static model catalog, which lists no self-hosted
/// model. A self-hosted model without the flag stays refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_self_hosted_vision_model_takes_an_image_through_the_identity_bridge() {
    let state = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let mob_id = format!("self-hosted-vision-{}", uuid::Uuid::new_v4());
    let spec = |identity: &str, profile: &str| DurableAgentSpec {
        identity: AgentIdentity::parse(identity).unwrap(),
        profile: profile.into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        backend: None,
        binding: None,
        placement: None,
    };
    let unified = Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(self_hosted_vision_definition(&mob_id))
            .meerkat_config(self_hosted_vision_config(&mob_id))
            .persistent_state(state.path())
            .roster_provider(Arc::new(MutableRosterProvider::new(vec![
                spec("agent:seeing", "seeing"),
                spec("agent:blind", "blind"),
            ])))
            .identity_runtime_instance_id("self-hosted-vision-bridge")
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .unwrap();
    let identity_runtime = unified.identity_runtime().unwrap();

    let blind = identity_runtime
        .send(&AgentIdentity::parse("agent:blind").unwrap(), &image_turn())
        .await;
    let seeing = identity_runtime
        .send(
            &AgentIdentity::parse("agent:seeing").unwrap(),
            &image_turn(),
        )
        .await;
    assert!(
        blind.as_ref().is_err_and(|error| {
            format!("{error:?}").contains("target member model cannot accept image input")
        }),
        "a self-hosted model without the flag is refused images: {blind:?}"
    );
    assert!(
        seeing.is_ok(),
        "a vision = true self-hosted model takes the image turn: {seeing:?}"
    );
    unified.shutdown().await;
}

/// The console's member send checks images with the same source: the
/// `vision = true` self-hosted worker takes the image, the other is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_self_hosted_vision_model_takes_an_image_through_the_console_send() {
    let state = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let unified = self_hosted_vision_workers(state.path()).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "vision",
        "",
        unified.mob_runtime().clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let request = |identity: &str| ConsoleSendRequest {
        identity: identity.to_string(),
        content: serde_json::to_value(image_turn()).unwrap(),
        origin: "console".to_string(),
        idempotency_key: format!("image-to-{identity}"),
        handling_mode: None,
        origin_kind: None,
        skill_refs: Vec::new(),
    };

    let blind = aggregator.send(request("blind-worker")).await;
    let seeing = aggregator.send(request("seeing-worker")).await;
    assert!(
        matches!(
            &blind,
            Err(crate::console_aggregator::ConsoleSendError::InvalidContent(message))
                if message.contains("target member model cannot accept image input")
        ),
        "a self-hosted model without the flag is refused images: {blind:?}"
    );
    assert!(
        seeing.is_ok(),
        "a vision = true self-hosted model takes the image: {seeing:?}"
    );
    let stopped = unified.mob_handle().stop().await;
    unified.mob_runtime().stop_runtime_delivery();
    stopped.unwrap();
}

/// The console's member rows (the snapshot walk and `meta/list_members`)
/// show image input for the `vision = true` self-hosted model, so the console
/// offers what the gate admits.
#[tokio::test(flavor = "multi_thread")]
async fn console_member_rows_show_a_self_hosted_model_vision_flag() {
    let state = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let unified = self_hosted_vision_workers(state.path()).await;
    let expected = BTreeMap::from([
        ("blind-worker".to_string(), false),
        ("seeing-worker".to_string(), true),
    ]);

    let read_model = super::collect_console_snapshot_read_model(
        unified.mob_runtime(),
        None,
        &super::WalkedHook::default(),
    )
    .await;
    let walked = read_model
        .primary_members
        .iter()
        .map(|member| {
            (
                member.agent_identity.clone(),
                member.model_capabilities.image_input,
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(walked, expected, "the console snapshot's member rows");

    let listed = Box::pin(super::handle_console_runtime_rpc(
        unified.mob_runtime(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        crate::rpc::JsonRpcRequest {
            jsonrpc: crate::rpc::JSONRPC_VERSION.to_string(),
            id: Some(json!(1)),
            method: "meta/list_members".to_string(),
            params: json!({}),
        },
        true,
    ))
    .await;
    let listed = listed["result"]
        .as_array()
        .unwrap_or_else(|| panic!("meta/list_members: {listed:#?}"))
        .iter()
        .map(|row| {
            (
                row["agent_identity"].as_str().unwrap().to_string(),
                row["model_capabilities"]["image_input"].as_bool().unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(listed, expected, "the meta/list_members rows");
    let stopped = unified.mob_handle().stop().await;
    unified.mob_runtime().stop_runtime_delivery();
    stopped.unwrap();
}

/// `meta/send_message` to a member checks images with the same source: the
/// `vision = true` self-hosted worker takes the image, the other is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_self_hosted_vision_model_takes_an_image_through_meta_send_message() {
    let state = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let unified = self_hosted_vision_workers(state.path()).await;
    let send = |member_id: &str| {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "meta/send_message",
            "params": {
                "member_id": member_id,
                "content": serde_json::to_value(image_turn()).unwrap(),
            },
        })
        .to_string()
    };

    let blind: serde_json::Value = serde_json::from_str(
        &crate::rpc::handle_unified_rpc_json(&unified, &send("blind-worker"), WAIT, None, None)
            .await,
    )
    .unwrap();
    let seeing: serde_json::Value = serde_json::from_str(
        &crate::rpc::handle_unified_rpc_json(&unified, &send("seeing-worker"), WAIT, None, None)
            .await,
    )
    .unwrap();
    assert!(
        blind["error"]["message"].as_str().is_some_and(
            |message| message.contains("target member model cannot accept image input")
        ),
        "a self-hosted model without the flag is refused images: {blind:#}"
    );
    assert!(
        seeing["error"].is_null() && !seeing["result"].is_null(),
        "a vision = true self-hosted model takes the image: {seeing:#}"
    );
    let stopped = unified.mob_handle().stop().await;
    unified.mob_runtime().stop_runtime_delivery();
    stopped.unwrap();
}
