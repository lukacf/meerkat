#![cfg(all(not(target_arch = "wasm32"), feature = "live"))]

use axum::extract::WebSocketUpgrade;
use axum::extract::ws::Message;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::routing::get;
use axum::{Json, Router};
use futures::SinkExt;
use oai_rt_rs::live::{
    ClientEvent, ClientOptions, Command, CreateRequest, LiveClient, SessionConfig,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Observation = (Uri, HeaderMap, Vec<Value>);

#[derive(Default)]
struct TraceCapture {
    records: Mutex<Vec<String>>,
    failed: AtomicBool,
    spans: AtomicU64,
}

struct TraceSubscriber(Arc<TraceCapture>);

#[derive(Default)]
struct TraceFields(String);

impl tracing::field::Visit for TraceFields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

impl TraceSubscriber {
    fn retain(&self, target: &str, fields: TraceFields) {
        let Ok(mut records) = self.0.records.lock() else {
            self.0.failed.store(true, Ordering::SeqCst);
            return;
        };
        if records.len() >= 4096 || fields.0.len() > 16384 {
            self.0.failed.store(true, Ordering::SeqCst);
            return;
        }
        records.push(format!("{target}: {}", fields.0));
    }
}

impl tracing::Subscriber for TraceSubscriber {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut fields = TraceFields::default();
        attributes.record(&mut fields);
        self.retain(attributes.metadata().target(), fields);
        tracing::span::Id::from_u64(self.0.spans.fetch_add(1, Ordering::SeqCst) + 1)
    }

    fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut fields = TraceFields::default();
        values.record(&mut fields);
        self.retain("span_update", fields);
    }

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = TraceFields::default();
        event.record(&mut fields);
        self.retain(event.metadata().target(), fields);
    }

    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn capture_http_trace() -> Result<&'static Arc<TraceCapture>, Box<dyn std::error::Error>> {
    static CAPTURE: OnceLock<Result<Arc<TraceCapture>, String>> = OnceLock::new();
    CAPTURE
        .get_or_init(|| {
            let capture = Arc::new(TraceCapture::default());
            tracing::subscriber::set_global_default(TraceSubscriber(capture.clone()))
                .map_err(|error| error.to_string())?;
            Ok(capture)
        })
        .as_ref()
        .map_err(|error| error.clone().into())
}

fn assert_trace_redaction(capture: &TraceCapture) -> TestResult {
    assert!(!capture.failed.load(Ordering::SeqCst));
    let records = capture
        .records
        .lock()
        .map_err(|_| "trace capture poisoned")?;
    assert!(
        records
            .iter()
            .any(|record| { record.starts_with("hyper") || record.starts_with("reqwest") }),
        "the authenticated HTTP implementation must actually emit captured trace"
    );
    assert!(
        !records
            .iter()
            .any(|record| { record.contains("public-live-fixture-not-a-secret") }),
        "authenticated TRACE must not disclose the fixture credential"
    );
    Ok(())
}

fn started() -> Value {
    json!({
        "type":"session.started","event_id":"started",
        "session":{"id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1000}
    })
}

async fn socket_contract(
    mut socket: axum::extract::ws::WebSocket,
    primary: bool,
    uri: Uri,
    headers: HeaderMap,
    observed: mpsc::Sender<Result<Observation, String>>,
) {
    let result = async {
        let mut commands = Vec::new();
        if primary {
            let Some(Ok(Message::Text(text))) = socket.recv().await else {
                return Err("primary connection omitted session.start".to_owned());
            };
            commands.push(serde_json::from_str(&text).map_err(|error| error.to_string())?);
        }
        socket
            .send(Message::Text(started().to_string().into()))
            .await
            .map_err(|error| error.to_string())?;
        let Some(Ok(Message::Text(text))) = socket.recv().await else {
            return Err("connection omitted requested close".to_owned());
        };
        commands.push(serde_json::from_str(&text).map_err(|error| error.to_string())?);
        Ok((uri, headers, commands))
    }
    .await;
    if observed.send(result).await.is_err() {
        // A failed test has already dropped the receiver; close its socket.
        let _ = socket.close().await;
    }
}

#[tokio::test]
async fn actual_primary_upgrade_and_sideband_attach_use_public_paths_and_exact_first_frames()
-> TestResult {
    let trace = capture_http_trace()?;
    let (observed, mut receive) = mpsc::channel(2);
    let primary_observed = observed.clone();
    let app = Router::new()
        .route(
            "/v1/live/sessions",
            get(move |uri: Uri, headers: HeaderMap, ws: WebSocketUpgrade| {
                let observed = primary_observed.clone();
                async move {
                    ws.on_upgrade(move |socket| {
                        socket_contract(socket, true, uri, headers, observed)
                    })
                }
            }),
        )
        .route(
            "/v1/live/sessions/{id}/attach",
            get(move |uri: Uri, headers: HeaderMap, ws: WebSocketUpgrade| {
                let observed = observed.clone();
                async move {
                    ws.on_upgrade(move |socket| {
                        socket_contract(socket, false, uri, headers, observed)
                    })
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown, stopping) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopping.await;
            })
            .await
    });
    let client = LiveClient::with_options(
        "public-live-fixture-not-a-secret",
        ClientOptions {
            base_url: format!("http://{address}/v1/").parse()?,
            request_timeout: Duration::from_secs(5),
            ..ClientOptions::default()
        },
    )?;
    let config: SessionConfig = serde_json::from_value(json!({"model":"gpt-live-1"}))?;
    let primary = client.connect(config).await?;
    primary
        .sender()
        .send(ClientEvent::new(Command::Close))
        .await?;
    let (uri, headers, frames) = tokio::time::timeout(Duration::from_secs(5), receive.recv())
        .await?
        .ok_or("primary observation")??;
    assert_eq!(uri.path(), "/v1/live/sessions");
    assert!(uri.query().is_none());
    assert_public_headers(&headers);
    assert_eq!(
        frames,
        vec![
            json!({"type":"session.start","session":{"model":"gpt-live-1"}}),
            json!({"type":"session.close"})
        ]
    );
    let sideband = client.attach("live_fixture").await?;
    sideband
        .sender()
        .send(ClientEvent::new(Command::Close))
        .await?;
    let (uri, headers, frames) = tokio::time::timeout(Duration::from_secs(5), receive.recv())
        .await?
        .ok_or("sideband observation")??;
    assert_eq!(uri.path(), "/v1/live/sessions/live_fixture/attach");
    assert!(uri.query().is_none());
    assert_public_headers(&headers);
    assert_eq!(frames, vec![json!({"type":"session.close"})]);
    drop((primary, sideband));
    shutdown.send(()).map_err(|()| "server already stopped")?;
    server.await??;
    assert_trace_redaction(trace)?;
    Ok(())
}

fn assert_public_headers(headers: &HeaderMap) {
    assert!(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            == Some("Bearer public-live-fixture-not-a-secret")
    );
    for forbidden in ["openai-beta", "openai-alpha", "chatgpt-account-id"] {
        assert!(!headers.contains_key(forbidden));
    }
}

#[tokio::test]
async fn actual_rtc_creation_requires_201_minimal_answer_and_never_retries_200() -> TestResult {
    let trace = capture_http_trace()?;
    for status in [StatusCode::CREATED, StatusCode::OK] {
        let (observed, mut receive) = mpsc::channel(2);
        let app = Router::new().route(
            "/v1/live/sessions",
            axum::routing::post(
                move |uri: Uri, headers: HeaderMap, Json(body): Json<Value>| {
                    let observed = observed.clone();
                    async move {
                        observed
                            .send((uri, headers, body))
                            .await
                            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                        Ok::<_, StatusCode>((
                            status,
                            Json(json!({
                                "session":{"id":"live_fixture"},
                                "transport":{"type":"webrtc","sdp":"answer-fixture"}
                            })),
                        ))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (shutdown, stopping) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopping.await;
                })
                .await
        });
        let client = LiveClient::with_options(
            "public-live-fixture-not-a-secret",
            ClientOptions {
                base_url: format!("http://{address}/v1/").parse()?,
                request_timeout: Duration::from_secs(5),
                ..ClientOptions::default()
            },
        )?;
        let body = json!({
            "session":{"model":"gpt-live-1"},
            "transport":{"type":"webrtc","sdp":"offer-fixture"}
        });
        let request: CreateRequest = serde_json::from_value(body.clone())?;
        let result = client.create_webrtc(&request).await;
        if status == StatusCode::CREATED {
            let created = result?;
            assert_eq!(created.session.id, "live_fixture");
            assert_eq!(created.transport.sdp(), "answer-fixture");
        } else {
            assert!(result.is_err());
        }
        let (uri, headers, received) = receive.try_recv()?;
        assert_eq!(uri.path(), "/v1/live/sessions");
        assert!(uri.query().is_none());
        assert_public_headers(&headers);
        assert_eq!(received, body);
        assert!(receive.try_recv().is_err(), "no automatic retry");
        shutdown.send(()).map_err(|()| "server already stopped")?;
        server.await??;
    }
    assert_trace_redaction(trace)?;
    Ok(())
}
