//! A single-session, owned HTTP framing fixture. All MCP handling is rmcp.
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
};
use futures::{StreamExt, channel::mpsc};
use mcp_test_server::FormTestServer;
use rmcp::{
    ServiceExt,
    model::{ClientJsonRpcMessage, RequestId, ServerJsonRpcMessage},
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mode {
    Sse,
    Streamable,
}
struct HttpState {
    mode: Mode,
    incoming: mpsc::UnboundedSender<ClientJsonRpcMessage>,
    events: Mutex<Option<mpsc::UnboundedReceiver<ServerJsonRpcMessage>>>,
    pending: Mutex<HashMap<RequestId, oneshot::Sender<ServerJsonRpcMessage>>>,
    required_bearer: Option<&'static str>,
    requests: AtomicUsize,
    rejected_auth: AtomicUsize,
}

async fn receive(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(message): Json<ClientJsonRpcMessage>,
) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    if let Some(expected) = state.required_bearer
        && headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(expected)
    {
        state.rejected_auth.fetch_add(1, Ordering::SeqCst);
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", "Bearer realm=\"form-fixture\"")],
        )
            .into_response();
    }
    // Match the protocol envelope only. rmcp owns every method and response.
    let reply = if state.mode == Mode::Streamable {
        if let ClientJsonRpcMessage::Request(request) = &message {
            let (tx, rx) = oneshot::channel();
            let mut pending = state.pending.lock().unwrap();
            if pending.contains_key(&request.id) {
                return StatusCode::CONFLICT.into_response();
            }
            pending.insert(request.id.clone(), tx);
            Some((request.id.clone(), rx))
        } else {
            None
        }
    } else {
        None
    };
    if state.incoming.unbounded_send(message).is_err() {
        if let Some((id, _)) = reply {
            state.pending.lock().unwrap().remove(&id);
        }
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    match reply {
        Some((_, rx)) => {
            // Publish the POST stream before awaiting the native response.
            // rmcp's client awaits POST framing in its send loop; withholding
            // headers until the final result prevents that loop from forwarding
            // the server callback arriving on the standalone GET stream.
            let initial = futures::stream::iter([Ok::<_, oneshot::error::RecvError>(
                Event::default().comment("request accepted"),
            )]);
            let reply = futures::stream::once(async move {
                rx.await.map(|message| {
                    Event::default()
                        .event("message")
                        .data(serde_json::to_string(&message).unwrap())
                })
            });
            (
                [("mcp-session-id", "form-fixture-session")],
                Sse::new(initial.chain(reply)),
            )
                .into_response()
        }
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn events(State(state): State<Arc<HttpState>>) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    let Some(rx) = state.events.lock().unwrap().take() else {
        return StatusCode::CONFLICT.into_response();
    };
    let endpoint =
        (state.mode == Mode::Sse).then(|| Event::default().event("endpoint").data("/messages"));
    let initial = futures::stream::iter(endpoint.into_iter().map(Ok::<_, Infallible>));
    let messages = rx.map(|message| {
        Ok::<_, Infallible>(
            Event::default()
                .event("message")
                .data(serde_json::to_string(&message).unwrap()),
        )
    });
    Sse::new(initial.chain(messages)).into_response()
}

async fn delete_session(State(state): State<Arc<HttpState>>) -> StatusCode {
    state.incoming.close_channel();
    StatusCode::OK
}

#[derive(Debug)]
pub(super) struct Shutdown {
    pub initialized: bool,
    pub joined: Vec<&'static str>,
    pub errors: Vec<String>,
}

pub(super) struct Endpoint {
    pub url: String,
    state: Arc<HttpState>,
    stop_service: Option<oneshot::Sender<()>>,
    service: Option<JoinHandle<Result<bool, String>>>,
    pump: Option<JoinHandle<()>>,
    stop_http: Option<oneshot::Sender<()>>,
    http: Option<JoinHandle<Result<(), std::io::Error>>>,
}

impl Endpoint {
    pub async fn start(mode: Mode, required_bearer: Option<&'static str>) -> std::io::Result<Self> {
        Self::start_at(mode, required_bearer, "127.0.0.1:0".parse().unwrap()).await
    }

    pub async fn start_at(
        mode: Mode,
        required_bearer: Option<&'static str>,
        address: std::net::SocketAddr,
    ) -> std::io::Result<Self> {
        // Both fallible preflight operations finish before creating task owners.
        let listener = tokio::net::TcpListener::bind(address).await?;
        let url = format!("http://{}/mcp", listener.local_addr()?);
        let (incoming, input) = mpsc::unbounded();
        let (output, mut outgoing) = mpsc::unbounded();
        let (event_tx, event_rx) = mpsc::unbounded();
        let state = Arc::new(HttpState {
            mode,
            incoming,
            events: Mutex::new(Some(event_rx)),
            pending: Mutex::new(HashMap::new()),
            required_bearer,
            requests: AtomicUsize::new(0),
            rejected_auth: AtomicUsize::new(0),
        });
        let pump_state = Arc::clone(&state);
        let pump = tokio::spawn(async move {
            while let Some(message) = outgoing.next().await {
                let id = match &message {
                    ServerJsonRpcMessage::Response(response) => Some(response.id.clone()),
                    ServerJsonRpcMessage::Error(error) => error.id.clone(),
                    _ => None,
                };
                if mode == Mode::Streamable && id.is_some() {
                    if let Some(tx) =
                        id.and_then(|id| pump_state.pending.lock().unwrap().remove(&id))
                    {
                        let _ = tx.send(message);
                    }
                } else {
                    let _ = event_tx.unbounded_send(message);
                }
            }
            // Wake any HTTP request owner whose protocol peer closed.
            pump_state.pending.lock().unwrap().clear();
        });
        let (stop_service, stop) = oneshot::channel();
        let service = tokio::spawn(async move {
            match FormTestServer::default().serve((output, input)).await {
                Ok(service) => {
                    let _ = stop.await;
                    match service.cancel().await.map_err(|e| e.to_string())? {
                        rmcp::service::QuitReason::Closed
                        | rmcp::service::QuitReason::Cancelled => Ok(true),
                        reason => Err(format!("fixture service failed: {reason:?}")),
                    }
                }
                // Factory refusal may intentionally leave handshake unentered.
                // The test checks its request count independently.
                Err(_) => Ok(false),
            }
        });
        let router = Router::new()
            .route("/mcp", get(events).post(receive).delete(delete_session))
            .route("/messages", post(receive))
            .with_state(Arc::clone(&state));
        let (stop_http, stop) = oneshot::channel();
        let http = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stop.await;
                })
                .await
        });
        Ok(Self {
            url,
            state,
            stop_service: Some(stop_service),
            service: Some(service),
            pump: Some(pump),
            stop_http: Some(stop_http),
            http: Some(http),
        })
    }

    pub fn requests(&self) -> usize {
        self.state.requests.load(Ordering::SeqCst)
    }
    pub fn rejected_auth(&self) -> usize {
        self.state.rejected_auth.load(Ordering::SeqCst)
    }

    pub fn abort_service_for_test(&self) {
        self.service
            .as_ref()
            .expect("fixture owns the service task")
            .abort();
    }

    pub async fn shutdown(&mut self) -> Shutdown {
        let mut report = Shutdown {
            initialized: false,
            joined: vec![],
            errors: vec![],
        };
        self.state.incoming.close_channel();
        if let Some(stop) = self.stop_service.take() {
            let _ = stop.send(());
        }
        if let Some(stop) = self.stop_http.take() {
            let _ = stop.send(());
        }
        if let Some(service) = self.service.take() {
            let result = service.await;
            report.joined.push("service");
            match result {
                Ok(Ok(initialized)) => report.initialized = initialized,
                Ok(Err(error)) => report.errors.push(format!("service: {error}")),
                Err(error) => report.errors.push(format!("service task: {error}")),
            }
        }
        // Output closes on service retirement, including task failure. Observe
        // the pump join even after a failed service join, then release pending
        // HTTP responders even if the pump failed before clearing them.
        if let Some(pump) = self.pump.take() {
            let result = pump.await;
            report.joined.push("pump");
            if let Err(error) = result {
                report.errors.push(format!("pump task: {error}"));
            }
        }
        match self.state.pending.lock() {
            Ok(mut pending) => pending.clear(),
            Err(poisoned) => {
                report
                    .errors
                    .push("pending HTTP response lock poisoned".into());
                poisoned.into_inner().clear();
            }
        }
        if let Some(http) = self.http.take() {
            let result = http.await;
            report.joined.push("http");
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => report.errors.push(format!("HTTP: {error}")),
                Err(error) => report.errors.push(format!("HTTP task: {error}")),
            }
        }
        // A failed/aborted join stays an error. `joined` records an observed
        // JoinHandle result, not a claim that every task completed gracefully.
        report
    }
}
