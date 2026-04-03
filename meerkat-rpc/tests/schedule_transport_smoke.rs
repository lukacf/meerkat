#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use futures::stream;
use meerkat::{
    AgentFactory, Config, ConfigRuntime, CreateScheduleRequest, HelperOptionsSpec,
    IntervalTriggerSpec, LlmClient, MemoryScheduleStore, MisfirePolicy, MissingTargetPolicy,
    MobTargetBinding, OverlapPolicy, PersistenceBundle, Schedule, ScheduledSessionAction,
    SessionMaterializationSpec, SessionTargetBinding, TargetBinding, TriggerSpec,
};
use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent};
use meerkat_core::{BlobStore, MemoryConfigStore, SessionId, StopReason};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_store::MemoryBlobStore;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};

const RPC_TIMEOUT: Duration = Duration::from_secs(20);

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "scheduled smoke ok".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            }),
        ]))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

async fn spawn_tcp_rpc_server() -> (
    tokio::net::tcp::OwnedWriteHalf,
    BufReader<tokio::net::tcp::OwnedReadHalf>,
    tokio::task::JoinHandle<Result<(), meerkat_rpc::server::ServerError>>,
) {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let blob_store: Arc<dyn BlobStore> = Arc::new(MemoryBlobStore::new());
    let schedule_store: Arc<dyn meerkat::RealmScheduleStore> = Arc::new(MemoryScheduleStore::new());
    let mut runtime = SessionRuntime::new(
        factory,
        config,
        10,
        PersistenceBundle::new_with_schedule_store(store, None, blob_store, schedule_store),
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));
    runtime.default_llm_client = Some(Arc::new(MockLlmClient));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config_state.json"),
    )));
    let runtime = Arc::new(runtime);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("listener should have addr");

    let server_handle = tokio::spawn(async move {
        let _temp = temp;
        let (stream, _) = listener.accept().await.expect("server should accept");
        let (read_half, write_half) = stream.into_split();
        let reader = BufReader::new(read_half);
        let mut server = RpcServer::new(reader, write_half, runtime, config_store);
        server.run().await
    });

    let client_stream = TcpStream::connect(addr)
        .await
        .expect("client should connect");
    let (read_half, write_half) = client_stream.into_split();
    (write_half, BufReader::new(read_half), server_handle)
}

async fn send_request(writer: &mut tokio::net::tcp::OwnedWriteHalf, request: &Value) {
    let line = format!(
        "{}\n",
        serde_json::to_string(request).expect("request should serialize")
    );
    timeout(RPC_TIMEOUT, writer.write_all(line.as_bytes()))
        .await
        .expect("request write should complete")
        .expect("request write should succeed");
    timeout(RPC_TIMEOUT, writer.flush())
        .await
        .expect("request flush should complete")
        .expect("request flush should succeed");
}

async fn read_response(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Value {
    loop {
        let mut line = String::new();
        timeout(RPC_TIMEOUT, reader.read_line(&mut line))
            .await
            .expect("response read should complete")
            .expect("response read should succeed");
        assert!(!line.is_empty(), "expected json-rpc response, got eof");
        let value: Value = serde_json::from_str(&line).expect("response should be valid json");
        if value.get("id").is_some() {
            return value;
        }
    }
}

async fn rpc_call(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    send_request(
        writer,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }),
    )
    .await;
    let response = read_response(reader).await;
    assert!(response["error"].is_null(), "{method} failed: {response}");
    response
}

async fn wait_for_occurrences(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    schedule_id: &str,
    completed_count: usize,
    starting_id: &mut u64,
) -> Vec<meerkat::Occurrence> {
    let mut last_seen = Vec::new();
    for _ in 0..80 {
        let response = rpc_call(
            writer,
            reader,
            *starting_id,
            "schedule/occurrences",
            json!({ "schedule_id": schedule_id }),
        )
        .await;
        *starting_id += 1;
        let occurrences: Vec<meerkat::Occurrence> =
            serde_json::from_value(response["result"]["occurrences"].clone())
                .expect("occurrences should decode");
        last_seen = occurrences.clone();
        if occurrences
            .iter()
            .filter(|occurrence| occurrence.phase == meerkat::OccurrencePhase::Completed)
            .count()
            >= completed_count
        {
            return occurrences;
        }
        sleep(Duration::from_millis(250)).await;
    }
    panic!(
        "timed out waiting for schedule {schedule_id} to complete {completed_count} occurrences; last seen: {:?}",
        last_seen
            .iter()
            .map(|occurrence| (
                occurrence.occurrence_ordinal.0,
                occurrence.phase,
                occurrence.failure_class,
                occurrence.failure_detail.clone(),
                occurrence.claimed_by.clone(),
                occurrence.last_receipt.as_ref().map(|receipt| (
                    receipt.stage,
                    receipt.materialized_session_id.clone(),
                    receipt.detail.clone(),
                    receipt.failure_class,
                )),
            ))
            .collect::<Vec<_>>()
    );
}

fn exact_session_prompt_target(session_id: &str, marker: &str) -> TargetBinding {
    TargetBinding::Session(SessionTargetBinding::ExactSession {
        session_id: SessionId::parse(session_id).expect("session id should parse"),
        action: ScheduledSessionAction::Prompt {
            prompt: format!("Remember the scheduled marker {marker}.").into(),
            system_prompt: None,
            render_metadata: None,
            skill_references: Vec::new(),
            additional_instructions: Vec::new(),
        },
    })
}

fn materialize_prompt_target(model: &str, marker: &str) -> TargetBinding {
    TargetBinding::Session(SessionTargetBinding::MaterializeOnDemandSession {
        create: SessionMaterializationSpec {
            model: model.to_string(),
            system_prompt: None,
            max_tokens: None,
            provider: None,
            output_schema_json: None,
            structured_output_retries: 0,
            provider_params: None,
            comms_name: None,
            peer_meta: None,
            labels: BTreeMap::new(),
            preload_skills: Vec::new(),
            additional_instructions: Vec::new(),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            keep_alive: false,
            app_context: None,
        },
        action: ScheduledSessionAction::Prompt {
            prompt: format!("Remember the scheduled marker {marker}.").into(),
            system_prompt: None,
            render_metadata: None,
            skill_references: Vec::new(),
            additional_instructions: Vec::new(),
        },
        bound_session_id: None,
    })
}

#[tokio::test]
async fn rpc_schedule_tools_cover_full_schedule_lifecycle_over_transport() {
    let (mut writer, mut reader, server_handle) = spawn_tcp_rpc_server().await;
    let mut next_id = 1;

    rpc_call(&mut writer, &mut reader, next_id, "initialize", json!({})).await;
    next_id += 1;

    let tools_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/tools",
        json!({}),
    )
    .await;
    next_id += 1;
    let tools = tools_response["result"]["tools"]
        .as_array()
        .expect("schedule/tools should return tools");
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "meerkat_schedule_create"),
        "schedule tools should include create"
    );

    let request = CreateScheduleRequest {
        name: Some("rpc-tool-schedule".into()),
        description: Some("rpc schedule tool smoke".into()),
        trigger: TriggerSpec::Interval(IntervalTriggerSpec {
            start_at_utc: Utc::now() + ChronoDuration::minutes(5),
            every_seconds: 300,
            end_at_utc: None,
        }),
        target: exact_session_prompt_target(&SessionId::new().to_string(), "RPC-TOOLS"),
        misfire_policy: MisfirePolicy::Skip,
        overlap_policy: OverlapPolicy::SkipIfRunning,
        missing_target_policy: MissingTargetPolicy::MarkMisfired,
        labels: BTreeMap::new(),
        planning_horizon_days: Some(1),
        planning_horizon_occurrences: Some(2),
    };

    let created = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/call",
        json!({
            "name": "meerkat_schedule_create",
            "arguments": serde_json::to_value(request).expect("tool request should serialize"),
        }),
    )
    .await;
    next_id += 1;
    let schedule_id = created["result"]["schedule_id"]
        .as_str()
        .expect("tool create should return schedule_id")
        .to_string();

    let fetched = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/call",
        json!({
            "name": "meerkat_schedule_get",
            "arguments": { "schedule_id": schedule_id.clone() }
        }),
    )
    .await;
    next_id += 1;
    assert_eq!(
        fetched["result"]["schedule_id"].as_str(),
        Some(schedule_id.as_str())
    );

    let listed = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/call",
        json!({
            "name": "meerkat_schedule_list",
            "arguments": {}
        }),
    )
    .await;
    next_id += 1;
    assert!(
        listed["result"]["schedules"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["schedule_id"] == schedule_id)),
        "tool list should include created schedule"
    );

    let occurrences = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/call",
        json!({
            "name": "meerkat_schedule_occurrences",
            "arguments": { "schedule_id": schedule_id.clone() }
        }),
    )
    .await;
    next_id += 1;
    assert!(
        occurrences["result"]["occurrences"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "tool occurrences should return planned rows"
    );

    let paused = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/call",
        json!({
            "name": "meerkat_schedule_pause",
            "arguments": { "schedule_id": schedule_id.clone() }
        }),
    )
    .await;
    next_id += 1;
    assert_eq!(paused["result"]["phase"].as_str(), Some("paused"));

    let resumed = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/call",
        json!({
            "name": "meerkat_schedule_resume",
            "arguments": { "schedule_id": schedule_id.clone() }
        }),
    )
    .await;
    next_id += 1;
    assert_eq!(resumed["result"]["phase"].as_str(), Some("active"));

    let deleted = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/call",
        json!({
            "name": "meerkat_schedule_delete",
            "arguments": { "schedule_id": schedule_id.clone() }
        }),
    )
    .await;
    assert_eq!(deleted["result"]["phase"].as_str(), Some("deleted"));

    drop(writer);
    drop(reader);
    server_handle
        .await
        .expect("server task should join")
        .expect("server should exit cleanly");
}

#[tokio::test]
async fn rpc_schedule_exact_session_prompt_is_delivered_end_to_end() {
    let (mut writer, mut reader, server_handle) = spawn_tcp_rpc_server().await;
    let mut next_id = 1;

    rpc_call(&mut writer, &mut reader, next_id, "initialize", json!({})).await;
    next_id += 1;

    let create_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "session/create",
        json!({
            "prompt": "Seed a session for exact schedule delivery.",
        }),
    )
    .await;
    next_id += 1;
    let session_id = create_response["result"]["session_id"]
        .as_str()
        .expect("session create should return session_id")
        .to_string();

    let marker = "RPC-SCHEDULE-EXACT";
    let schedule_request = CreateScheduleRequest {
        name: Some("rpc-exact".into()),
        description: Some("rpc exact-session delivery smoke".into()),
        trigger: TriggerSpec::Once {
            due_at_utc: Utc::now() - ChronoDuration::seconds(1),
        },
        target: exact_session_prompt_target(&session_id, marker),
        misfire_policy: MisfirePolicy::Skip,
        overlap_policy: OverlapPolicy::SkipIfRunning,
        missing_target_policy: MissingTargetPolicy::MarkMisfired,
        labels: BTreeMap::new(),
        planning_horizon_days: Some(1),
        planning_horizon_occurrences: Some(1),
    };
    let create_schedule_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/create",
        serde_json::to_value(schedule_request).expect("schedule request should serialize"),
    )
    .await;
    next_id += 1;
    let schedule: Schedule = serde_json::from_value(create_schedule_response["result"].clone())
        .expect("schedule create should decode");

    let occurrences = wait_for_occurrences(
        &mut writer,
        &mut reader,
        &schedule.schedule_id.to_string(),
        1,
        &mut next_id,
    )
    .await;
    let completed: Vec<_> = occurrences
        .iter()
        .filter(|occurrence| occurrence.phase == meerkat::OccurrencePhase::Completed)
        .collect();
    assert_eq!(completed.len(), 1, "expected one completed occurrence");
    assert_eq!(
        completed[0]
            .last_receipt
            .as_ref()
            .map(|receipt| receipt.stage),
        Some(meerkat::DeliveryReceiptStage::Completed)
    );

    let history_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "session/history",
        json!({ "session_id": session_id }),
    )
    .await;
    let history_text =
        serde_json::to_string(&history_response["result"]).expect("history should serialize");
    assert!(
        history_text.contains(marker),
        "scheduled marker should appear in session history: {history_text}"
    );
    assert!(
        history_response["result"]["message_count"]
            .as_u64()
            .unwrap_or_default()
            >= 4,
        "exact-session delivery should append a prompt/response pair: {}",
        history_response["result"]
    );

    drop(writer);
    drop(reader);
    server_handle
        .await
        .expect("server task should join")
        .expect("server should exit cleanly");
}

#[tokio::test]
async fn rpc_schedule_materialized_session_binds_and_reuses_over_multiple_occurrences() {
    let (mut writer, mut reader, server_handle) = spawn_tcp_rpc_server().await;
    let mut next_id = 1;

    rpc_call(&mut writer, &mut reader, next_id, "initialize", json!({})).await;
    next_id += 1;

    let marker = "RPC-SCHEDULE-MATERIALIZE";
    let interval_start = Utc::now() + ChronoDuration::seconds(1);
    let create_schedule_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/create",
        serde_json::to_value(CreateScheduleRequest {
            name: Some("rpc-materialize".into()),
            description: Some("rpc materialize-on-demand delivery smoke".into()),
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: interval_start,
                every_seconds: 1,
                end_at_utc: Some(interval_start + ChronoDuration::seconds(1)),
            }),
            target: materialize_prompt_target("mock-model", marker),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(2),
        })
        .expect("schedule request should serialize"),
    )
    .await;
    next_id += 1;
    let schedule: Schedule = serde_json::from_value(create_schedule_response["result"].clone())
        .expect("schedule create should decode");

    let occurrences = wait_for_occurrences(
        &mut writer,
        &mut reader,
        &schedule.schedule_id.to_string(),
        2,
        &mut next_id,
    )
    .await;
    let completed: Vec<_> = occurrences
        .iter()
        .filter(|occurrence| occurrence.phase == meerkat::OccurrencePhase::Completed)
        .collect();
    assert_eq!(completed.len(), 2, "expected two completed occurrences");

    let get_schedule_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "schedule/get",
        json!({ "schedule_id": schedule.schedule_id }),
    )
    .await;
    next_id += 1;
    let bound_schedule: Schedule = serde_json::from_value(get_schedule_response["result"].clone())
        .expect("schedule get should decode");
    let bound_session_id = match bound_schedule.target {
        TargetBinding::Session(SessionTargetBinding::MaterializeOnDemandSession {
            bound_session_id: Some(session_id),
            ..
        }) => session_id,
        other => panic!("expected bound materialized session target, got {other:?}"),
    };

    for occurrence in &completed {
        let materialized = occurrence
            .last_receipt
            .as_ref()
            .and_then(|receipt| receipt.materialized_session_id.as_ref());
        assert_eq!(
            materialized,
            Some(&bound_session_id),
            "completed occurrence should record the bound session id"
        );
    }

    let history_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "session/history",
        json!({ "session_id": bound_session_id }),
    )
    .await;
    let history_text =
        serde_json::to_string(&history_response["result"]).expect("history should serialize");
    assert!(
        history_text.contains(marker),
        "materialized session history should include scheduled prompt marker: {history_text}"
    );
    assert!(
        history_response["result"]["message_count"]
            .as_u64()
            .unwrap_or_default()
            >= 4,
        "two scheduled prompts should reuse one materialized session: {}",
        history_response["result"]
    );

    drop(writer);
    drop(reader);
    server_handle
        .await
        .expect("server task should join")
        .expect("server should exit cleanly");
}

#[cfg(feature = "mob")]
#[tokio::test]
async fn rpc_schedule_mob_helper_action_is_delivered_end_to_end() {
    let (mut writer, mut reader, server_handle) = spawn_tcp_rpc_server().await;
    let mut next_id = 1;

    rpc_call(&mut writer, &mut reader, next_id, "initialize", json!({})).await;
    next_id += 1;

    let create_mob_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "mob/create",
        json!({
            "definition": {
                "id": "scheduled_mob",
                "profiles": {
                    "worker": {
                        "model": "gpt-5.2",
                        "tools": { "comms": true }
                    }
                }
            }
        }),
    )
    .await;
    next_id += 1;
    let mob_id = create_mob_response["result"]["mob_id"]
        .as_str()
        .expect("mob/create should return mob_id")
        .to_string();

    let marker = "RPC-SCHEDULE-MOB-HELPER";
    let schedule: Schedule = serde_json::from_value(
        rpc_call(
            &mut writer,
            &mut reader,
            next_id,
            "schedule/create",
            serde_json::to_value(CreateScheduleRequest {
                name: Some("rpc-mob-helper".into()),
                description: Some("rpc scheduled mob helper delivery smoke".into()),
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - ChronoDuration::seconds(1),
                },
                target: TargetBinding::Mob(MobTargetBinding::SpawnHelper {
                    mob_id: mob_id.clone(),
                    member_id: "helper-1".into(),
                    prompt: format!("Say the marker {marker} and stop."),
                    options: HelperOptionsSpec {
                        profile_name: Some("worker".into()),
                        ..Default::default()
                    },
                }),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .expect("schedule request should serialize"),
        )
        .await["result"]
            .clone(),
    )
    .expect("schedule create should decode");
    next_id += 1;

    let occurrences = wait_for_occurrences(
        &mut writer,
        &mut reader,
        &schedule.schedule_id.to_string(),
        1,
        &mut next_id,
    )
    .await;
    let completed: Vec<_> = occurrences
        .iter()
        .filter(|occurrence| occurrence.phase == meerkat::OccurrencePhase::Completed)
        .collect();
    assert_eq!(completed.len(), 1, "expected one completed occurrence");

    let events_response = rpc_call(
        &mut writer,
        &mut reader,
        next_id,
        "mob/events",
        json!({ "mob_id": mob_id, "after_cursor": 0, "limit": 100 }),
    )
    .await;
    let history_text =
        serde_json::to_string(&events_response["result"]).expect("events should serialize");
    assert!(
        history_text.contains("helper-1"),
        "scheduled mob helper action should appear in mob events: {history_text}"
    );

    drop(writer);
    drop(reader);
    server_handle
        .await
        .expect("server task should join")
        .expect("server should exit cleanly");
}
