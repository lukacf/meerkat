#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../test-fixtures/live_smoke/support.rs"]
mod live_smoke;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use meerkat::SessionService;
use meerkat::{LlmClient, encode_llm_client_override_for_service};
use meerkat_client::AnthropicClient;
use meerkat_core::lifecycle::InputId;
use meerkat_core::service::{
    CreateSessionRequest as SvcCreateSessionRequest, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::{ContextConfig, RealmConfig, RealmSelection, RuntimeBootstrap};
use meerkat_rest::{AppState, router};
use reqwest::Client as HttpClient;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep, timeout};
use tower::ServiceExt;

const REST_REALM_ID: &str = "rest-live-smoke";

fn live_client() -> Option<Arc<dyn LlmClient>> {
    let api_key = live_smoke::anthropic_api_key()?;
    Some(Arc::new(
        AnthropicClient::new(api_key).expect("Anthropic client should initialize"),
    ))
}

fn rest_bootstrap(root: &std::path::Path, instance_id: &str) -> RuntimeBootstrap {
    let project_root = root.join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("project root should initialize");
    RuntimeBootstrap {
        realm: RealmConfig {
            selection: RealmSelection::Explicit {
                realm_id: REST_REALM_ID.to_string(),
            },
            instance_id: Some(instance_id.to_string()),
            backend_hint: None,
            state_root: Some(root.join("realms")),
        },
        context: ContextConfig {
            context_root: Some(project_root),
            user_config_root: None,
        },
    }
}

async fn build_live_state(
    root: &std::path::Path,
    client: Arc<dyn LlmClient>,
    instance_id: &str,
    expose_paths: bool,
) -> AppState {
    let mut state =
        AppState::load_with_bootstrap_and_options(rest_bootstrap(root, instance_id), expose_paths)
            .await
            .expect("app state should load");
    state.llm_client_override = Some(client);
    state
}

async fn request_json(
    app: &axum::Router,
    method: Method,
    uri: impl Into<String>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri.into());
    let request = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        builder
            .body(Body::from(
                serde_json::to_vec(&body).expect("request body should serialize"),
            ))
            .expect("request should build")
    } else {
        builder.body(Body::empty()).expect("request should build")
    };

    let response = timeout(live_smoke::live_timeout(), app.clone().oneshot(request))
        .await
        .expect("request timed out")
        .expect("request should succeed");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body should read")
        .to_bytes();
    let value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).expect("response body should be json")
    };
    (status, value)
}

async fn request_text(
    app: &axum::Router,
    method: Method,
    uri: impl Into<String>,
) -> (StatusCode, String) {
    let request = Request::builder()
        .method(method)
        .uri(uri.into())
        .body(Body::empty())
        .expect("request should build");
    let response = timeout(live_smoke::live_timeout(), app.clone().oneshot(request))
        .await
        .expect("request timed out")
        .expect("request should succeed");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body should read")
        .to_bytes();
    let text = String::from_utf8(bytes.to_vec()).expect("response should be utf8");
    (status, text)
}

async fn create_deferred_session(state: &AppState, prompt: &str) -> String {
    let result = state
        .session_service
        .create_session(SvcCreateSessionRequest {
            model: state.default_model.to_string(),
            prompt: prompt.to_string(),
            system_prompt: None,
            max_tokens: Some(state.max_tokens),
            event_tx: None,
            host_mode: false,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            build: Some(SessionBuildOptions {
                llm_client_override: state
                    .llm_client_override
                    .clone()
                    .map(encode_llm_client_override_for_service),
                ..Default::default()
            }),
            labels: Some(BTreeMap::from([(
                "live_smoke".to_string(),
                "rest".to_string(),
            )])),
        })
        .await
        .expect("deferred session should create");
    result.session_id.to_string()
}

fn durable_prompt_input(text: &str) -> Value {
    json!({
        "input_type": "prompt",
        "header": {
            "id": InputId::new(),
            "timestamp": Utc::now(),
            "source": { "type": "operator" },
            "durability": "durable",
            "visibility": {
                "transcript_eligible": true,
                "operator_eligible": true
            }
        },
        "text": text
    })
}

fn durable_peer_progress_input(request_id: &str) -> Value {
    json!({
        "input_type": "peer",
        "header": {
            "id": InputId::new(),
            "timestamp": Utc::now(),
            "source": {
                "type": "peer",
                "peer_id": "rest-live-peer"
            },
            "durability": "durable",
            "visibility": {
                "transcript_eligible": true,
                "operator_eligible": true
            }
        },
        "convention": {
            "convention_type": "response_progress",
            "request_id": request_id,
            "phase": "accepted"
        },
        "body": "still working"
    })
}

async fn wait_for_input_state(
    app: &axum::Router,
    session_id: &str,
    input_id: &str,
    wanted: &[&str],
) -> Value {
    for _ in 0..40 {
        let (status, payload) = request_json(
            app,
            Method::GET,
            format!("/input/{session_id}/{input_id}"),
            None,
        )
        .await;
        if status == StatusCode::OK {
            let state = payload["current_state"].as_str().unwrap_or_default();
            if wanted.contains(&state) {
                return payload;
            }
        }
        sleep(Duration::from_millis(250)).await;
    }
    panic!("timed out waiting for input {input_id} to reach one of {wanted:?}");
}

async fn spawn_http_server(
    app: axum::Router,
) -> (std::net::SocketAddr, oneshot::Sender<()>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("listener addr");
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

async fn read_http_response_head(socket: &mut TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        let read = timeout(live_smoke::live_timeout(), socket.read(&mut buffer))
            .await
            .expect("http response head timed out")
            .expect("http response head should read");
        assert!(read > 0, "http response closed before headers completed");
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return bytes;
        }
    }
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_21_rest_runtime_accept_input_roundtrip() {
    let Some(client) = live_client() else {
        eprintln!("Skipping scenario 21: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("rest-s21");
    let state = build_live_state(root.path(), client, "rest-s21", false).await;
    let session_id = create_deferred_session(&state, "Hold the runtime for scenario 21.").await;
    let app = router(state);

    let (status, runtime_state) = request_json(
        &app,
        Method::GET,
        format!("/runtime/{session_id}/state"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runtime_state["state"], "idle");

    let (status, accepted) = request_json(
        &app,
        Method::POST,
        format!("/runtime/{session_id}/accept"),
        Some(durable_prompt_input(
            "Remember the marker REST-RUNTIME-21 and acknowledge briefly.",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["outcome_type"], "accepted");
    let input_id = accepted["input_id"]
        .as_str()
        .expect("accepted input should have id")
        .to_string();

    let (status, listed) =
        request_json(&app, Method::GET, format!("/input/{session_id}/list"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        listed["input_ids"].as_array().is_some_and(|ids| ids
            .iter()
            .any(|value| value.as_str() == Some(input_id.as_str()))),
        "accepted input should be visible in active input list"
    );

    let final_state = wait_for_input_state(
        &app,
        &session_id,
        &input_id,
        &["applied", "applied_pending_consumption", "consumed"],
    )
    .await;
    assert_eq!(final_state["input_id"], input_id);

    let (status, continued) = request_json(
        &app,
        Method::POST,
        format!("/sessions/{session_id}/messages"),
        Some(json!({
            "session_id": session_id,
            "prompt": "What marker were you asked to remember? Reply in one short sentence."
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = continued["text"].as_str().unwrap_or("").to_lowercase();
    assert!(
        text.contains("rest-runtime-21") || text.contains("rest runtime 21"),
        "runtime accept turn should be durable and recallable, got: {text}"
    );
}

#[tokio::test]
#[ignore = "integration-real: runtime control plane"]
async fn e2e_scenario_22_rest_runtime_reset_and_retire_drain_staged_inputs() {
    let Some(client) = live_client() else {
        eprintln!("Skipping scenario 22: missing ANTHROPIC_API_KEY");
        return;
    };

    {
        let root = live_smoke::LiveSmokeDir::new("rest-s22-reset");
        let state = build_live_state(root.path(), client.clone(), "rest-s22-reset", false).await;
        let reset_session_id =
            create_deferred_session(&state, "Hold idle state for reset scenario 22.").await;
        let app = router(state);

        let (status, accepted) = request_json(
            &app,
            Method::POST,
            format!("/runtime/{reset_session_id}/accept"),
            Some(durable_peer_progress_input("rest-reset-22")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let reset_input_id = accepted["input_id"].as_str().expect("input id");

        let (status, reset_payload) = request_json(
            &app,
            Method::POST,
            format!("/runtime/{reset_session_id}/reset"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            reset_payload["inputs_abandoned"]
                .as_u64()
                .unwrap_or_default()
                >= 1,
            "reset should abandon the staged runtime input"
        );

        let abandoned =
            wait_for_input_state(&app, &reset_session_id, reset_input_id, &["abandoned"]).await;
        assert_eq!(abandoned["current_state"], "abandoned");

        let (status, runtime_state) = request_json(
            &app,
            Method::GET,
            format!("/runtime/{reset_session_id}/state"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(runtime_state["state"], "idle");
    }

    let root = live_smoke::LiveSmokeDir::new("rest-s22-retire");
    let state = build_live_state(root.path(), client, "rest-s22-retire", false).await;
    let retire_session_id =
        create_deferred_session(&state, "Hold idle state for retire scenario 22.").await;
    let app = router(state);

    let (status, accepted) = request_json(
        &app,
        Method::POST,
        format!("/runtime/{retire_session_id}/accept"),
        Some(durable_prompt_input(
            "Remember rest-retire-22 and apply it before retirement completes.",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let retire_input_id = accepted["input_id"].as_str().expect("input id");

    let (status, retire_payload) = request_json(
        &app,
        Method::POST,
        format!("/runtime/{retire_session_id}/retire"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        retire_payload["inputs_abandoned"]
            .as_u64()
            .unwrap_or_default(),
        0,
        "retire should stop new accepts but drain already-accepted work"
    );

    let consumed =
        wait_for_input_state(&app, &retire_session_id, retire_input_id, &["consumed"]).await;
    assert_eq!(consumed["current_state"], "consumed");

    let (status, runtime_state) = request_json(
        &app,
        Method::GET,
        format!("/runtime/{retire_session_id}/state"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runtime_state["state"], "retired");

    let (status, rejected) = request_json(
        &app,
        Method::POST,
        format!("/runtime/{retire_session_id}/accept"),
        Some(durable_prompt_input(
            "This should be rejected while retired.",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rejected["outcome_type"].as_str(),
        Some("rejected"),
        "retired runtimes should reject fresh work"
    );

    let (status, reset_payload) = request_json(
        &app,
        Method::POST,
        format!("/runtime/{retire_session_id}/reset"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        reset_payload["inputs_abandoned"]
            .as_u64()
            .unwrap_or_default(),
        0,
        "reset after a drained retire should not need to abandon already-finished work"
    );

    let (status, runtime_state) = request_json(
        &app,
        Method::GET,
        format!("/runtime/{retire_session_id}/state"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        runtime_state["state"], "idle",
        "reset should make a retired runtime usable again"
    );

    let (status, accepted) = request_json(
        &app,
        Method::POST,
        format!("/runtime/{retire_session_id}/accept"),
        Some(durable_prompt_input(
            "Remember retired-reset-22 and reply briefly when asked.",
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let post_reset_input_id = accepted["input_id"].as_str().expect("post-reset input id");
    let post_reset_state =
        wait_for_input_state(&app, &retire_session_id, post_reset_input_id, &["consumed"]).await;
    assert_eq!(post_reset_state["current_state"], "consumed");

    let (status, continued) = request_json(
        &app,
        Method::POST,
        format!("/sessions/{retire_session_id}/messages"),
        Some(json!({
            "session_id": retire_session_id,
            "prompt": "What marker were you asked to remember after reset? Reply in one sentence."
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = continued["text"].as_str().unwrap_or("").to_lowercase();
    assert!(
        text.contains("retired-reset-22") || text.contains("retired reset 22"),
        "a reset runtime should accept fresh work after retirement, got: {text}"
    );
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_23_rest_sse_events_follow_continue_turn() {
    let Some(client) = live_client() else {
        eprintln!("Skipping scenario 23: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("rest-s23");
    let state = build_live_state(root.path(), client, "rest-s23", false).await;
    let app = router(state);
    let http = HttpClient::new();
    let (addr, shutdown_tx, server_handle) = spawn_http_server(app.clone()).await;

    let (status, created) = request_json(
        &app,
        Method::POST,
        "/sessions",
        Some(json!({
            "prompt": "My name is RestEventBot and my color is orange. Reply briefly.",
            "model": live_smoke::smoke_model()
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let session_id = created["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let mut sse = TcpStream::connect(addr)
        .await
        .expect("sse socket should connect");
    let request = format!(
        "GET /sessions/{session_id}/events HTTP/1.1\r\nhost: {addr}\r\naccept: text/event-stream\r\nconnection: keep-alive\r\n\r\n"
    );
    sse.write_all(request.as_bytes())
        .await
        .expect("sse request should write");
    sse.flush().await.expect("sse request should flush");

    let mut sse_bytes = read_http_response_head(&mut sse).await;
    let initial_text = String::from_utf8_lossy(&sse_bytes);
    assert!(
        initial_text.starts_with("HTTP/1.1 200") || initial_text.starts_with("HTTP/1.0 200"),
        "sse route should return 200, got: {initial_text}"
    );
    let follow_up = {
        let http = http.clone();
        let session_id = session_id.clone();
        tokio::spawn(async move {
            let response = http
                .post(format!("http://{addr}/sessions/{session_id}/messages"))
                .json(&json!({
                    "session_id": session_id,
                    "prompt": "What are my name and favorite color? Reply in one sentence."
                }))
                .send()
                .await
                .expect("follow-up request should succeed");
            let status = response.status();
            let payload = response
                .json::<Value>()
                .await
                .expect("follow-up body should be json");
            (status, payload)
        })
    };

    let (status, continued) = follow_up.await.expect("follow-up task should join");
    assert_eq!(status, reqwest::StatusCode::OK);
    let text = continued["text"].as_str().unwrap_or("").to_lowercase();
    assert!(
        (text.contains("resteventbot") || text.contains("rest event bot"))
            && text.contains("orange"),
        "follow-up turn should preserve context, got: {text}"
    );

    let mut saw_follow_up_event = initial_text.contains("session_loaded")
        || initial_text.contains("assistant_delta")
        || initial_text.contains("run_finished")
        || initial_text.contains("session_updated");
    for _ in 0..12 {
        if saw_follow_up_event {
            break;
        }
        let mut buffer = [0u8; 4096];
        let read = timeout(live_smoke::live_timeout(), sse.read(&mut buffer))
            .await
            .expect("sse chunk timed out")
            .expect("sse chunk should read");
        if read == 0 {
            break;
        }
        sse_bytes.extend_from_slice(&buffer[..read]);
        let body = String::from_utf8_lossy(&sse_bytes);
        if body.contains("assistant_delta")
            || body.contains("run_finished")
            || body.contains("session_updated")
        {
            saw_follow_up_event = true;
        }
    }
    assert!(
        saw_follow_up_event,
        "expected streamed session events in SSE body, got: {}",
        String::from_utf8_lossy(&sse_bytes)
    );
    drop(sse);
    let _ = shutdown_tx.send(());
    let _ = server_handle.await;
}

#[tokio::test]
#[ignore = "integration-real: diagnostics endpoints"]
async fn e2e_scenario_24_rest_config_capabilities_health_and_skills() {
    let Some(client) = live_client() else {
        eprintln!("Skipping scenario 24: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("rest-s24");
    let state = build_live_state(root.path(), client, "rest-s24", true).await;
    let app = router(state);

    let (status, health) = request_text(&app, Method::GET, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health.trim(), "ok");

    let (status, capabilities) = request_json(&app, Method::GET, "/capabilities", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(capabilities["capabilities"].is_array());

    let (status, config) = request_json(&app, Method::GET, "/config", None).await;
    assert_eq!(status, StatusCode::OK);
    let generation = config["generation"].as_u64().expect("config generation");
    assert!(config["resolved_paths"].is_object());

    let (status, patched) = request_json(
        &app,
        Method::PATCH,
        "/config",
        Some(json!({
            "patch": { "max_tokens": 384 },
            "expected_generation": generation
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["config"]["max_tokens"], 384);

    let (status, skills) = request_json(&app, Method::GET, "/skills", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        skills["skills"].is_array() || skills["entries"].is_array(),
        "skills endpoint should return an array payload"
    );
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_25_rest_reload_and_resume_on_same_realm_root() {
    let Some(client) = live_client() else {
        eprintln!("Skipping scenario 25: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("rest-s25");
    let state = build_live_state(root.path(), client.clone(), "rest-s25-a", false).await;
    let app = router(state.clone());

    let (status, created) = request_json(
        &app,
        Method::POST,
        "/sessions",
        Some(json!({
            "prompt": "My codename is RestReload30 and my lucky number is 3030. Reply briefly.",
            "model": live_smoke::smoke_model()
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let session_id = created["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    drop(app);
    drop(state);

    let state = build_live_state(root.path(), client, "rest-s25-b", false).await;
    let app = router(state);

    let (status, resumed) = request_json(
        &app,
        Method::POST,
        format!("/sessions/{session_id}/messages"),
        Some(json!({
            "session_id": session_id,
            "prompt": "What are my codename and lucky number? Reply in one sentence."
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = resumed["text"].as_str().unwrap_or("").to_lowercase();
    assert!(
        (text.contains("restreload30") || text.contains("rest reload 30")) && text.contains("3030"),
        "reloaded app should resume the persisted session, got: {text}"
    );
}
