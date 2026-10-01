//! Test-only HTTP framing derived from Meerkat's MIT/Apache-2.0
//! crates/meerkat-mcp/tests/form_elicitation/http.rs at 138b175d8a559724e95017696e336aedc27ff3e2.
//! rmcp owns all protocol dispatch. Every fixture-created task is retained/joined.
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::get,
};
use futures::{StreamExt, channel::mpsc};
use meerkat_core::ToolDef;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientJsonRpcMessage, ListToolsResult,
        PaginatedRequestParams, RequestId, ServerCapabilities, ServerInfo, ServerJsonRpcMessage,
        Tool,
    },
    service::RequestContext,
};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    sync::{Notify, oneshot},
    task::JoinHandle,
};

#[derive(Default)]
pub struct Gate {
    pub entered: Notify,
    pub release: Notify,
}

pub struct Server {
    pub account: &'static str,
    pub tools: Vec<Arc<ToolDef>>,
    pub list_gate: Option<Arc<Gate>>,
    pub fail_list: bool,
    pub lists: Arc<AtomicUsize>,
    pub calls: Arc<AtomicUsize>,
}
impl Server {
    pub fn new(account: &'static str, tools: Vec<Arc<ToolDef>>) -> Self {
        Self {
            account,
            tools,
            list_gate: None,
            fail_list: false,
            lists: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.list_gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        if self.fail_list {
            return Err(ErrorData::internal_error("provider-secret-canary", None));
        }
        Ok(ListToolsResult {
            tools: self
                .tools
                .iter()
                .map(|tool| {
                    Tool::new(
                        tool.name.to_string(),
                        tool.description.clone(),
                        tool.input_schema.as_object().unwrap().clone(),
                    )
                })
                .collect(),
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if !self
            .tools
            .iter()
            .any(|tool| tool.name.as_str() == request.name.as_ref())
        {
            return Err(ErrorData::invalid_params("unknown fixture tool", None));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CallToolResult::structured(
            serde_json::json!({"account":self.account,"arguments":request.arguments}),
        ))
    }
}

struct HttpState {
    incoming: mpsc::UnboundedSender<ClientJsonRpcMessage>,
    events: Mutex<Option<mpsc::UnboundedReceiver<ServerJsonRpcMessage>>>,
    pending: Mutex<HashMap<RequestId, oneshot::Sender<ServerJsonRpcMessage>>>,
    required_bearer: Option<&'static str>,
    requests: AtomicUsize,
    deletes: AtomicUsize,
    delete_gate: Option<Arc<Gate>>,
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
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", "Bearer realm=\"fixture\"")],
        )
            .into_response();
    }
    let reply = if let ClientJsonRpcMessage::Request(request) = &message {
        let (tx, rx) = oneshot::channel();
        let mut pending = state.pending.lock().unwrap();
        if pending.contains_key(&request.id) {
            return StatusCode::CONFLICT.into_response();
        }
        pending.insert(request.id.clone(), tx);
        Some((request.id.clone(), rx))
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
                [("mcp-session-id", "toolkit-transport-fixture")],
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
    Sse::new(rx.map(|message| {
        Ok::<_, Infallible>(
            Event::default()
                .event("message")
                .data(serde_json::to_string(&message).unwrap()),
        )
    }))
    .into_response()
}
async fn delete(State(state): State<Arc<HttpState>>) -> StatusCode {
    state.deletes.fetch_add(1, Ordering::SeqCst);
    if let Some(gate) = &state.delete_gate {
        gate.entered.notify_one();
        gate.release.notified().await;
    }
    state.incoming.close_channel();
    StatusCode::OK
}

struct HttpPeer {
    state: Arc<HttpState>,
    list_gate: Option<Arc<Gate>>,
    stop_service: Option<oneshot::Sender<()>>,
    service: Option<JoinHandle<Result<bool, String>>>,
    pump: Option<JoinHandle<()>>,
}
impl HttpPeer {
    fn start(server: Server, bearer: Option<&'static str>, delete_gate: Option<Arc<Gate>>) -> Self {
        let list_gate = server.list_gate.clone();
        let (incoming, input) = mpsc::unbounded();
        let (output, mut outgoing) = mpsc::unbounded();
        let (event_tx, event_rx) = mpsc::unbounded();
        let state = Arc::new(HttpState {
            incoming,
            events: Mutex::new(Some(event_rx)),
            pending: Mutex::new(HashMap::new()),
            required_bearer: bearer,
            requests: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            delete_gate,
        });
        let pump_state = Arc::clone(&state);
        let pump = tokio::spawn(async move {
            while let Some(message) = outgoing.next().await {
                let id = match &message {
                    ServerJsonRpcMessage::Response(response) => Some(response.id.clone()),
                    ServerJsonRpcMessage::Error(error) => error.id.clone(),
                    _ => None,
                };
                if let Some(id) = id {
                    if let Some(tx) = pump_state.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(message);
                    }
                } else {
                    let _ = event_tx.unbounded_send(message);
                }
            }
            pump_state.pending.lock().unwrap().clear();
        });
        let (stop_service, stop) = oneshot::channel();
        let service = tokio::spawn(async move {
            match server.serve((output, input)).await {
                Ok(service) => {
                    let _ = stop.await;
                    match service.cancel().await.map_err(|e| e.to_string())? {
                        rmcp::service::QuitReason::Closed
                        | rmcp::service::QuitReason::Cancelled => Ok(true),
                        reason => Err(format!("fixture service: {reason:?}")),
                    }
                }
                Err(_) => Ok(false),
            }
        });
        Self {
            state,
            list_gate,
            stop_service: Some(stop_service),
            service: Some(service),
            pump: Some(pump),
        }
    }
    pub fn release_gates(&self) {
        if let Some(gate) = &self.list_gate {
            gate.release.notify_one();
        }
        if let Some(gate) = &self.state.delete_gate {
            gate.release.notify_one();
        }
    }
    async fn shutdown(&mut self) -> (bool, Vec<&'static str>, Vec<String>) {
        self.release_gates();
        self.state.incoming.close_channel();
        if let Some(stop) = self.stop_service.take() {
            let _ = stop.send(());
        }
        let mut initialized = false;
        let mut joined = vec![];
        let mut errors = vec![];
        if let Some(task) = self.service.take() {
            let result = task.await;
            joined.push("service");
            match result {
                Ok(Ok(value)) => initialized = value,
                other => errors.push(format!("service: {other:?}")),
            }
        }
        if let Some(task) = self.pump.take() {
            let result = task.await;
            joined.push("pump");
            if let Err(error) = result {
                errors.push(format!("pump: {error}"));
            }
        }
        match self.state.pending.lock() {
            Ok(mut pending) => pending.clear(),
            Err(poisoned) => {
                errors.push("pending mutex poisoned".into());
                poisoned.into_inner().clear();
            }
        }
        (initialized, joined, errors)
    }
}

struct AccountRoutes {
    peers: HashMap<&'static str, Arc<HttpState>>,
}
impl AccountRoutes {
    fn select(&self, headers: &HeaderMap) -> Option<Arc<HttpState>> {
        let bearer = headers.get("authorization")?.to_str().ok()?;
        self.peers.get(bearer).cloned()
    }
}
async fn account_receive(
    State(accounts): State<Arc<AccountRoutes>>,
    headers: HeaderMap,
    message: Json<ClientJsonRpcMessage>,
) -> Response {
    let Some(peer) = accounts.select(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    receive(State(peer), headers, message).await
}
async fn account_events(
    State(accounts): State<Arc<AccountRoutes>>,
    headers: HeaderMap,
) -> Response {
    let Some(peer) = accounts.select(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    events(State(peer)).await
}
async fn account_delete(
    State(accounts): State<Arc<AccountRoutes>>,
    headers: HeaderMap,
) -> Response {
    let Some(peer) = accounts.select(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    delete(State(peer)).await.into_response()
}

pub struct Endpoint {
    pub url: String,
    peers: Vec<HttpPeer>,
    stop_http: Option<oneshot::Sender<()>>,
    http: Option<JoinHandle<Result<(), std::io::Error>>>,
}
impl Endpoint {
    pub async fn start(
        server: Server,
        bearer: Option<&'static str>,
        delete_gate: Option<Arc<Gate>>,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/mcp", listener.local_addr()?);
        let peer = HttpPeer::start(server, bearer, delete_gate);
        let router = Router::new()
            .route("/mcp", get(events).post(receive).delete(delete))
            .with_state(Arc::clone(&peer.state));
        Ok(Self::serve(url, listener, router, vec![peer]))
    }

    /// One network endpoint with separate actual rmcp peers selected only by the
    /// request's bearer header. No account is selected by URL or tool name.
    pub async fn start_accounts(accounts: Vec<(Server, &'static str)>) -> std::io::Result<Self> {
        let mut names = std::collections::HashSet::new();
        if accounts.is_empty() || accounts.iter().any(|(_, bearer)| !names.insert(*bearer)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "fixture accounts require distinct bearer headers",
            ));
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/mcp", listener.local_addr()?);
        let mut routes = HashMap::new();
        let peers = accounts
            .into_iter()
            .map(|(server, bearer)| {
                let peer = HttpPeer::start(server, Some(bearer), None);
                routes.insert(bearer, Arc::clone(&peer.state));
                peer
            })
            .collect();
        let router = Router::new()
            .route(
                "/mcp",
                get(account_events)
                    .post(account_receive)
                    .delete(account_delete),
            )
            .with_state(Arc::new(AccountRoutes { peers: routes }));
        Ok(Self::serve(url, listener, router, peers))
    }

    fn serve(
        url: String,
        listener: tokio::net::TcpListener,
        router: Router,
        peers: Vec<HttpPeer>,
    ) -> Self {
        let (stop_http, stop) = oneshot::channel();
        let http = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stop.await;
                })
                .await
        });
        Self {
            url,
            peers,
            stop_http: Some(stop_http),
            http: Some(http),
        }
    }

    pub fn requests(&self) -> usize {
        self.peers
            .iter()
            .map(|peer| peer.state.requests.load(Ordering::SeqCst))
            .sum()
    }
    pub fn deletes(&self) -> usize {
        self.peers
            .iter()
            .map(|peer| peer.state.deletes.load(Ordering::SeqCst))
            .sum()
    }
    pub fn release_gates(&self) {
        for peer in &self.peers {
            peer.release_gates();
        }
    }
    pub fn expected_joins(&self) -> Vec<&'static str> {
        self.peers
            .iter()
            .flat_map(|_| ["service", "pump"])
            .chain(["http"])
            .collect()
    }
    /// Every owned join is attempted before assertions, including after body
    /// panic and partial endpoint acquisition. Native close alone is not this join.
    pub async fn shutdown(&mut self) -> (bool, Vec<&'static str>, Vec<String>) {
        self.release_gates();
        if let Some(stop) = self.stop_http.take() {
            let _ = stop.send(());
        }
        let mut initialized = true;
        let mut joined = vec![];
        let mut errors = vec![];
        for peer in &mut self.peers {
            let (peer_initialized, peer_joined, peer_errors) = peer.shutdown().await;
            initialized &= peer_initialized;
            joined.extend(peer_joined);
            errors.extend(peer_errors);
        }
        if let Some(task) = self.http.take() {
            let result = task.await;
            joined.push("http");
            if !matches!(result, Ok(Ok(()))) {
                errors.push(format!("http: {result:?}"));
            }
        }
        (initialized, joined, errors)
    }
}
