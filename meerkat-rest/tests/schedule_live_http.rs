#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use futures::stream;
use meerkat::{
    CreateScheduleRequest, IntervalTriggerSpec, LlmClient, MisfirePolicy, MissingTargetPolicy,
    OverlapPolicy, Schedule, ScheduledSessionAction, SessionMaterializationSpec,
    SessionTargetBinding, TargetBinding, TriggerSpec,
};
#[cfg(feature = "mob")]
use meerkat::{HelperOptionsSpec, MobTargetBinding};
use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent};
use meerkat_core::{
    ContextConfig, RealmConfig, RealmSelection, RuntimeBootstrap, SessionId, StopReason,
};
use meerkat_rest::{AppState, router};
use reqwest::Client;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep, timeout};

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

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

fn bootstrap(root: &std::path::Path, realm_id: &str, instance_id: &str) -> RuntimeBootstrap {
    let project_root = root.join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("project root should initialize");
    RuntimeBootstrap {
        realm: RealmConfig {
            selection: RealmSelection::Explicit {
                realm_id: realm_id.to_string(),
            },
            instance_id: Some(instance_id.to_string()),
            backend_hint: Some("sqlite".to_string()),
            state_root: Some(root.join("realms")),
        },
        context: ContextConfig {
            context_root: Some(project_root),
            user_config_root: None,
        },
    }
}

async fn build_state(root: &std::path::Path, realm_id: &str, instance_id: &str) -> AppState {
    let mut state =
        AppState::load_with_bootstrap_and_options(bootstrap(root, realm_id, instance_id), false)
            .await
            .expect("app state should load");
    state.llm_client_override = Some(Arc::new(MockLlmClient));
    state
        .ensure_schedule_host_started()
        .await
        .expect("schedule host should start");
    state
}

async fn spawn_http_server(
    app: axum::Router,
) -> (std::net::SocketAddr, oneshot::Sender<()>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("listener should have addr");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server should serve");
    });
    (addr, shutdown_tx, handle)
}

async fn create_session(client: &Client, base_url: &str, model: &str, prompt: &str) -> String {
    let response = timeout(
        HTTP_TIMEOUT,
        client
            .post(format!("{base_url}/sessions"))
            .json(&json!({
                "prompt": prompt,
                "model": model,
            }))
            .send(),
    )
    .await
    .expect("session create should complete")
    .expect("session create request should succeed")
    .error_for_status()
    .expect("session create status should be ok");
    let payload: Value = response
        .json()
        .await
        .expect("session create body should be json");
    payload["session_id"]
        .as_str()
        .expect("session create should return session_id")
        .to_string()
}

async fn create_schedule(
    client: &Client,
    base_url: &str,
    request: &CreateScheduleRequest,
) -> Schedule {
    let response = timeout(
        HTTP_TIMEOUT,
        client
            .post(format!("{base_url}/schedules"))
            .json(request)
            .send(),
    )
    .await
    .expect("schedule create should complete")
    .expect("schedule create request should succeed")
    .error_for_status()
    .expect("schedule create status should be ok");
    response
        .json::<Schedule>()
        .await
        .expect("schedule create should decode")
}

async fn schedule_tool_call(
    client: &Client,
    base_url: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let response = timeout(
        HTTP_TIMEOUT,
        client
            .post(format!("{base_url}/schedule/call"))
            .json(&json!({
                "name": name,
                "arguments": arguments,
            }))
            .send(),
    )
    .await
    .expect("schedule tool call should complete")
    .expect("schedule tool call request should succeed")
    .error_for_status()
    .expect("schedule tool call status should be ok");
    response
        .json::<Value>()
        .await
        .expect("schedule tool call should decode")
}

async fn get_schedule(client: &Client, base_url: &str, schedule_id: &str) -> Schedule {
    let response = timeout(
        HTTP_TIMEOUT,
        client
            .get(format!("{base_url}/schedules/{schedule_id}"))
            .send(),
    )
    .await
    .expect("schedule get should complete")
    .expect("schedule get request should succeed")
    .error_for_status()
    .expect("schedule get status should be ok");
    response
        .json::<Schedule>()
        .await
        .expect("schedule get should decode")
}

async fn wait_for_occurrences(
    client: &Client,
    base_url: &str,
    schedule_id: &str,
    completed_count: usize,
) -> Vec<meerkat::Occurrence> {
    for _ in 0..80 {
        let response = timeout(
            HTTP_TIMEOUT,
            client
                .get(format!("{base_url}/schedules/{schedule_id}/occurrences"))
                .send(),
        )
        .await
        .expect("occurrence list should complete")
        .expect("occurrence list request should succeed")
        .error_for_status()
        .expect("occurrence list status should be ok");
        let payload: Value = response
            .json()
            .await
            .expect("occurrence list should be json");
        let occurrences: Vec<meerkat::Occurrence> =
            serde_json::from_value(payload["occurrences"].clone())
                .expect("occurrences should decode");
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
        "timed out waiting for schedule {schedule_id} to complete {completed_count} occurrences"
    );
}

async fn get_history(client: &Client, base_url: &str, session_id: &str) -> Value {
    let response = timeout(
        HTTP_TIMEOUT,
        client
            .get(format!("{base_url}/sessions/{session_id}/history"))
            .send(),
    )
    .await
    .expect("history request should complete")
    .expect("history request should succeed")
    .error_for_status()
    .expect("history status should be ok");
    response
        .json()
        .await
        .expect("history response should be json")
}

#[cfg(feature = "mob")]
async fn mob_call(client: &Client, base_url: &str, name: &str, arguments: Value) -> Value {
    let response = timeout(
        HTTP_TIMEOUT,
        client
            .post(format!("{base_url}/mob/call"))
            .json(&json!({
                "name": name,
                "arguments": arguments,
            }))
            .send(),
    )
    .await
    .expect("mob call should complete")
    .expect("mob call request should succeed")
    .error_for_status()
    .expect("mob call status should be ok");
    response
        .json()
        .await
        .expect("mob call response should be json")
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

#[cfg(feature = "mob")]
fn mob_helper_target(mob_id: &str, member_id: &str, marker: &str) -> TargetBinding {
    TargetBinding::Mob(MobTargetBinding::SpawnHelper {
        mob_id: mob_id.to_string(),
        member_id: member_id.to_string(),
        prompt: format!("Say the marker {marker} and stop."),
        options: HelperOptionsSpec {
            profile_name: Some("worker".into()),
            ..Default::default()
        },
    })
}

#[tokio::test]
async fn rest_schedule_exact_session_prompt_is_delivered_end_to_end() {
    let root = TempDir::new().expect("tempdir should create");
    let state = build_state(root.path(), "rest-schedule-exact", "rest-schedule-exact").await;
    let model = state.default_model.to_string();
    let shutdown_state = state.clone();
    let app = router(state);
    let (addr, shutdown_tx, server_handle) = spawn_http_server(app).await;
    let base_url = format!("http://{addr}");
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("http client should build");

    let session_id = create_session(
        &client,
        &base_url,
        &model,
        "Seed a session for exact schedule delivery.",
    )
    .await;

    let marker = "REST-SCHEDULE-EXACT";
    let created = create_schedule(
        &client,
        &base_url,
        &CreateScheduleRequest {
            name: Some("rest-exact".into()),
            description: Some("rest exact-session delivery smoke".into()),
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
        },
    )
    .await;

    let occurrences =
        wait_for_occurrences(&client, &base_url, &created.schedule_id.to_string(), 1).await;
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

    let history = get_history(&client, &base_url, &session_id).await;
    let history_text = serde_json::to_string(&history).expect("history should serialize");
    assert!(
        history_text.contains(marker),
        "scheduled marker should appear in session history: {history_text}"
    );
    assert!(
        history["message_count"].as_u64().unwrap_or_default() >= 4,
        "exact-session delivery should append a prompt/response pair: {history}"
    );

    shutdown_tx.send(()).expect("shutdown should send");
    server_handle.await.expect("server task should join");
    shutdown_state.shutdown_schedule_host().await;
    shutdown_state
        .request_executor
        .shutdown_and_abort_stragglers()
        .await;
}

#[tokio::test]
async fn rest_schedule_tools_cover_full_schedule_lifecycle_over_http() {
    let root = TempDir::new().expect("tempdir should create");
    let state = build_state(root.path(), "rest-schedule-tools", "rest-schedule-tools").await;
    let shutdown_state = state.clone();
    let app = router(state);
    let (addr, shutdown_tx, server_handle) = spawn_http_server(app).await;
    let base_url = format!("http://{addr}");
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("http client should build");

    let tools_response = timeout(
        HTTP_TIMEOUT,
        client.get(format!("{base_url}/schedule/tools")).send(),
    )
    .await
    .expect("schedule tools request should complete")
    .expect("schedule tools request should succeed")
    .error_for_status()
    .expect("schedule tools status should be ok")
    .json::<Value>()
    .await
    .expect("schedule tools should decode");
    assert!(
        tools_response["tools"].as_array().is_some_and(|tools| tools
            .iter()
            .any(|tool| tool["name"] == "meerkat_schedule_create")),
        "schedule tools should include create"
    );

    let request = CreateScheduleRequest {
        name: Some("rest-tool-schedule".into()),
        description: Some("rest schedule tool smoke".into()),
        trigger: TriggerSpec::Interval(IntervalTriggerSpec {
            start_at_utc: Utc::now() + ChronoDuration::minutes(5),
            every_seconds: 300,
            end_at_utc: None,
        }),
        target: exact_session_prompt_target(&SessionId::new().to_string(), "REST-TOOLS"),
        misfire_policy: MisfirePolicy::Skip,
        overlap_policy: OverlapPolicy::SkipIfRunning,
        missing_target_policy: MissingTargetPolicy::MarkMisfired,
        labels: BTreeMap::new(),
        planning_horizon_days: Some(1),
        planning_horizon_occurrences: Some(2),
    };

    let created = schedule_tool_call(
        &client,
        &base_url,
        "meerkat_schedule_create",
        serde_json::to_value(request).expect("tool request should serialize"),
    )
    .await;
    let schedule_id = created["schedule_id"]
        .as_str()
        .expect("tool create should return schedule_id")
        .to_string();

    let fetched = schedule_tool_call(
        &client,
        &base_url,
        "meerkat_schedule_get",
        json!({ "schedule_id": schedule_id.clone() }),
    )
    .await;
    assert_eq!(fetched["schedule_id"].as_str(), Some(schedule_id.as_str()));

    let listed = schedule_tool_call(&client, &base_url, "meerkat_schedule_list", json!({})).await;
    assert!(
        listed["schedules"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["schedule_id"] == schedule_id)),
        "tool list should include created schedule"
    );

    let occurrences = schedule_tool_call(
        &client,
        &base_url,
        "meerkat_schedule_occurrences",
        json!({ "schedule_id": schedule_id.clone() }),
    )
    .await;
    assert!(
        occurrences["occurrences"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "tool occurrences should return planned rows"
    );

    let paused = schedule_tool_call(
        &client,
        &base_url,
        "meerkat_schedule_pause",
        json!({ "schedule_id": schedule_id.clone() }),
    )
    .await;
    assert_eq!(paused["phase"].as_str(), Some("paused"));

    let resumed = schedule_tool_call(
        &client,
        &base_url,
        "meerkat_schedule_resume",
        json!({ "schedule_id": schedule_id.clone() }),
    )
    .await;
    assert_eq!(resumed["phase"].as_str(), Some("active"));

    let deleted = schedule_tool_call(
        &client,
        &base_url,
        "meerkat_schedule_delete",
        json!({ "schedule_id": schedule_id.clone() }),
    )
    .await;
    assert_eq!(deleted["phase"].as_str(), Some("deleted"));

    shutdown_tx.send(()).expect("shutdown should send");
    server_handle.await.expect("server task should join");
    shutdown_state.shutdown_schedule_host().await;
    shutdown_state
        .request_executor
        .shutdown_and_abort_stragglers()
        .await;
}

#[tokio::test]
async fn rest_schedule_materialized_session_binds_and_reuses_over_multiple_occurrences() {
    let root = TempDir::new().expect("tempdir should create");
    let state = build_state(
        root.path(),
        "rest-schedule-materialize",
        "rest-schedule-materialize",
    )
    .await;
    let model = state.default_model.to_string();
    let shutdown_state = state.clone();
    let app = router(state);
    let (addr, shutdown_tx, server_handle) = spawn_http_server(app).await;
    let base_url = format!("http://{addr}");
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("http client should build");

    let marker = "REST-SCHEDULE-MATERIALIZE";
    let interval_start = Utc::now() + ChronoDuration::seconds(1);
    let created = create_schedule(
        &client,
        &base_url,
        &CreateScheduleRequest {
            name: Some("rest-materialize".into()),
            description: Some("rest materialize-on-demand delivery smoke".into()),
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: interval_start,
                every_seconds: 1,
                end_at_utc: Some(interval_start + ChronoDuration::seconds(1)),
            }),
            target: materialize_prompt_target(&model, marker),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(2),
        },
    )
    .await;

    let occurrences =
        wait_for_occurrences(&client, &base_url, &created.schedule_id.to_string(), 2).await;
    let completed: Vec<_> = occurrences
        .iter()
        .filter(|occurrence| occurrence.phase == meerkat::OccurrencePhase::Completed)
        .collect();
    assert_eq!(completed.len(), 2, "expected two completed occurrences");

    let schedule = get_schedule(&client, &base_url, &created.schedule_id.to_string()).await;
    let bound_session_id = match schedule.target {
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

    let history = get_history(&client, &base_url, &bound_session_id.to_string()).await;
    let history_text = serde_json::to_string(&history).expect("history should serialize");
    assert!(
        history_text.contains(marker),
        "materialized session history should include scheduled prompt marker: {history_text}"
    );
    assert!(
        history["message_count"].as_u64().unwrap_or_default() >= 4,
        "two scheduled prompts should reuse one materialized session: {history}"
    );

    shutdown_tx.send(()).expect("shutdown should send");
    server_handle.await.expect("server task should join");
    shutdown_state.shutdown_schedule_host().await;
    shutdown_state
        .request_executor
        .shutdown_and_abort_stragglers()
        .await;
}

#[cfg(feature = "mob")]
#[tokio::test]
async fn rest_schedule_mob_helper_action_is_delivered_end_to_end() {
    let root = TempDir::new().expect("tempdir should create");
    let state = build_state(root.path(), "rest-schedule-mob", "rest-schedule-mob").await;
    let shutdown_state = state.clone();
    let app = router(state);
    let (addr, shutdown_tx, server_handle) = spawn_http_server(app).await;
    let base_url = format!("http://{addr}");
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("http client should build");

    let create_mob_response = mob_call(
        &client,
        &base_url,
        "mob_create",
        json!({
            "definition": {
                "id": "scheduled_rest_mob",
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
    let mob_id = create_mob_response["mob_id"]
        .as_str()
        .expect("mob_create should return mob_id")
        .to_string();

    let marker = "REST-SCHEDULE-MOB-HELPER";
    let created = create_schedule(
        &client,
        &base_url,
        &CreateScheduleRequest {
            name: Some("rest-mob-helper".into()),
            description: Some("rest scheduled mob helper delivery smoke".into()),
            trigger: TriggerSpec::Once {
                due_at_utc: Utc::now() - ChronoDuration::seconds(1),
            },
            target: mob_helper_target(&mob_id, "helper-1", marker),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        },
    )
    .await;

    let occurrences =
        wait_for_occurrences(&client, &base_url, &created.schedule_id.to_string(), 1).await;
    let completed: Vec<_> = occurrences
        .iter()
        .filter(|occurrence| occurrence.phase == meerkat::OccurrencePhase::Completed)
        .collect();
    assert_eq!(completed.len(), 1, "expected one completed occurrence");

    let events_response = mob_call(
        &client,
        &base_url,
        "mob_events",
        json!({
            "mob_id": mob_id,
            "after_cursor": 0,
            "limit": 100,
        }),
    )
    .await;
    let history_text =
        serde_json::to_string(&events_response).expect("mob events should serialize");
    assert!(
        history_text.contains("helper-1"),
        "scheduled mob helper action should appear in mob events: {history_text}"
    );

    shutdown_tx.send(()).expect("shutdown should send");
    server_handle.await.expect("server task should join");
    shutdown_state.shutdown_schedule_host().await;
    shutdown_state
        .request_executor
        .shutdown_and_abort_stragglers()
        .await;
}
