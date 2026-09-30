//! In-memory protocol and request-lifetime tests; no subprocess or listener.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::client_service::ConnectedClient;
use crate::{McpClientServiceFactory, McpError, McpProtocol};
use futures::FutureExt;
use meerkat_core::McpServerConfig;
use rmcp::model::{
    ClientInfo, ClientResult, CreateElicitationRequest, CreateElicitationRequestParams,
    CreateElicitationResult, ElicitationAction, ErrorCode, ServerNotification, ServerRequest,
};
use rmcp::service::{
    DynService, NotificationContext, PeerRequestOptions, RequestContext, RunningService,
    ServiceError,
};
use rmcp::{ClientHandler, ErrorData, RoleClient, RoleServer, Service, ServiceExt};
use serde_json::{Value, json};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
const LIMIT: Duration = Duration::from_secs(10);

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic".into())
}
async fn cleanup<T, E: std::fmt::Debug>(
    errors: &mut Vec<String>,
    label: &str,
    future: impl std::future::Future<Output = Result<T, E>>,
) -> Option<T> {
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(error)) => {
            errors.push(format!("{label}: {error:?}"));
            None
        }
        Err(panic) => {
            errors.push(format!(
                "{label} panicked: {}",
                panic_message(panic.as_ref())
            ));
            None
        }
    }
}
fn finish_test(result: std::thread::Result<()>, errors: Vec<String>) {
    if let Err(panic) = result {
        for error in errors {
            eprintln!("cleanup after test panic: {error}");
        }
        std::panic::resume_unwind(panic);
    }
    assert!(errors.is_empty(), "fixture cleanup failed: {errors:?}");
}

type Server = RunningService<RoleServer, mcp_test_server::FormTestServer>;
#[derive(Debug)]
struct InitFailure {
    errors: Vec<String>,
    joined: Vec<&'static str>,
}

/// Inspect both init outcomes before propagating either failure. A successful
/// peer's real cancel/join remains owned even when its counterpart failed.
async fn initialized_peers<CE: std::fmt::Debug, SE: std::fmt::Debug>(
    client: Result<ConnectedClient, CE>,
    server: Result<Server, SE>,
) -> Result<(ConnectedClient, Server), InitFailure> {
    match (client, server) {
        (Ok(client), Ok(server)) => Ok((client, server)),
        (client, server) => {
            let mut failure = InitFailure {
                errors: vec![],
                joined: vec![],
            };
            match client {
                Ok(client) => {
                    if cleanup(
                        &mut failure.errors,
                        "initialized client close",
                        client.cancel(),
                    )
                    .await
                    .is_some()
                    {
                        failure.joined.push("client");
                    }
                }
                Err(error) => failure.errors.push(format!("client init: {error:?}")),
            }
            match server {
                Ok(server) => {
                    if cleanup(
                        &mut failure.errors,
                        "initialized server close",
                        server.cancel(),
                    )
                    .await
                    .is_some()
                    {
                        failure.joined.push("server");
                    }
                }
                Err(error) => failure.errors.push(format!("server init: {error:?}")),
            }
            Err(failure)
        }
    }
}

fn host_info(account: &str, instance: usize, _forms: bool) -> ClientInfo {
    serde_json::from_value(json!({
        "protocolVersion": "2025-11-25", "capabilities": {"elicitation": {"form": {}}},
        "clientInfo": {"name": account, "version": instance.to_string()}
    }))
    .unwrap()
}

struct Gate {
    entered: Semaphore,
    release: Semaphore,
    dropped: Semaphore,
    returned: AtomicUsize,
    request_id: Mutex<Option<rmcp::model::RequestId>>,
}
impl Default for Gate {
    fn default() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            dropped: Semaphore::new(0),
            returned: AtomicUsize::new(0),
            request_id: Mutex::new(None),
        }
    }
}
struct Entered(Arc<Gate>);
impl Drop for Entered {
    fn drop(&mut self) {
        self.0.dropped.add_permits(1);
    }
}
struct GatedHost {
    gate: Arc<Gate>,
}
impl ClientHandler for GatedHost {
    fn get_info(&self) -> ClientInfo {
        host_info("gated", 1, true)
    }
    async fn create_elicitation(
        &self,
        _request: CreateElicitationRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, ErrorData> {
        *self.gate.request_id.lock().unwrap() = Some(context.id.clone());
        let _entered = Entered(Arc::clone(&self.gate));
        self.gate.entered.add_permits(1);
        self.gate.release.acquire().await.unwrap().forget();
        self.gate.returned.fetch_add(1, Ordering::SeqCst);
        Ok(CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(json!({"confirm": true})),
            meta: None,
        })
    }
}
struct GateFactory(Arc<Gate>);
impl McpClientServiceFactory for GateFactory {
    fn create(
        &self,
        _config: &McpServerConfig,
    ) -> Result<Box<dyn DynService<RoleClient>>, McpError> {
        Ok(GatedHost {
            gate: Arc::clone(&self.0),
        }
        .into_dyn())
    }
}

async fn acquire(semaphore: &Semaphore) {
    tokio::time::timeout(LIMIT, semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}
fn dummy_config() -> McpServerConfig {
    McpServerConfig::stdio(
        "duplex-form",
        "no-process-in-this-fixture",
        vec![],
        Default::default(),
    )
}
async fn form_handle(
    server: &rmcp::service::Peer<RoleServer>,
) -> rmcp::service::RequestHandle<RoleServer> {
    server
        .send_request_with_option(
            ServerRequest::CreateElicitationRequest(CreateElicitationRequest::new(
                mcp_test_server::form_request("accept").unwrap(),
            )),
            PeerRequestOptions::with_timeout(LIMIT),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn form_exact_request_cancellation_drops_pending_handler_without_user_cancel() {
    let gate = Arc::new(Gate::default());
    let (client_io, server_io) = tokio::io::duplex(8192);
    let selection = crate::client_service::ClientServiceSelection::select(
        &dummy_config(),
        Some(&GateFactory(Arc::clone(&gate))),
    )
    .unwrap();
    let (client, server) = tokio::join!(
        selection.serve(client_io),
        mcp_test_server::FormTestServer::default().serve(server_io)
    );
    let (client, server) = initialized_peers(client, server).await.unwrap();
    let result = AssertUnwindSafe(async {
        let request = form_handle(server.peer()).await;
        acquire(&gate.entered).await;
        assert_eq!(*gate.request_id.lock().unwrap(), Some(request.id.clone()));
        server
            .notify_cancelled(rmcp::model::CancelledNotificationParam {
                request_id: request.id.clone(),
                reason: Some("fixture request retired".into()),
            })
            .await
            .unwrap();
        acquire(&gate.dropped).await;
        assert_eq!(gate.returned.load(Ordering::SeqCst), 0);
        let response = request.await_response().await;
        // rmcp retires the local responder when sending cancellation; its
        // native result can win the race with the remote guard's exact error.
        // Neither outcome is a user Cancel/Decline or a successful answer.
        match &response {
            Err(ServiceError::Cancelled { reason })
                if reason.as_deref() == Some("fixture request retired") => {}
            Err(ServiceError::McpError(error))
                if error.code == ErrorCode::INVALID_REQUEST
                    && error.message == "form elicitation request is no longer active" => {}
            other => panic!("unexpected exact-request cancellation result: {other:?}"),
        }
        // Cancellation is per-request, not a copied connection-wide deny bit.
        gate.release.add_permits(1);
        let response = form_handle(server.peer())
            .await
            .await_response()
            .await
            .unwrap();
        assert!(
            matches!(response, ClientResult::CreateElicitationResult(result)
            if result.action == ElicitationAction::Accept)
        );
        assert_eq!(gate.returned.load(Ordering::SeqCst), 1);
    })
    .catch_unwind()
    .await;
    gate.release.add_permits(4);
    let mut errors = vec![];
    let _ = cleanup(&mut errors, "client close", client.cancel()).await;
    let _ = cleanup(&mut errors, "server close", server.cancel()).await;
    finish_test(result, errors);
}

#[tokio::test]
async fn form_explicit_connection_close_cancels_the_entered_handler() {
    let gate = Arc::new(Gate::default());
    let (client_io, server_io) = tokio::io::duplex(8192);
    let selection = crate::client_service::ClientServiceSelection::select(
        &dummy_config(),
        Some(&GateFactory(Arc::clone(&gate))),
    )
    .unwrap();
    let (client, server) = tokio::join!(
        selection.serve(client_io),
        mcp_test_server::FormTestServer::default().serve(server_io)
    );
    let (client, server) = initialized_peers(client, server).await.unwrap();
    let mut client = Some(client);
    let result = AssertUnwindSafe(async {
        let request = form_handle(server.peer()).await;
        acquire(&gate.entered).await;
        client.take().unwrap().cancel().await.unwrap();
        acquire(&gate.dropped).await;
        assert_eq!(gate.returned.load(Ordering::SeqCst), 0);
        match request.await_response().await {
            Err(ServiceError::TransportClosed) => {}
            Err(ServiceError::McpError(error)) => {
                assert_eq!(error.code, ErrorCode::INVALID_REQUEST);
                assert_eq!(
                    error.message,
                    "form elicitation request is no longer active"
                );
            }
            other => panic!("close became a user action or unrelated error: {other:?}"),
        }
        // This proves entered future retirement, not a join of arbitrary work
        // that an application might separately spawn from its callback.
    })
    .catch_unwind()
    .await;
    gate.release.add_permits(4);
    let mut errors = vec![];
    if let Some(client) = client.take() {
        let _ = cleanup(&mut errors, "client close", client.cancel()).await;
    }
    let _ = cleanup(&mut errors, "server close", server.cancel()).await;
    finish_test(result, errors);
}

struct WrongResponse;
impl Service<RoleClient> for WrongResponse {
    async fn handle_request(
        &self,
        _request: ServerRequest,
        _context: RequestContext<RoleClient>,
    ) -> Result<ClientResult, ErrorData> {
        Ok(ClientResult::empty(()))
    }
    async fn handle_notification(
        &self,
        _notification: ServerNotification,
        _context: NotificationContext<RoleClient>,
    ) -> Result<(), ErrorData> {
        Ok(())
    }
    fn get_info(&self) -> ClientInfo {
        host_info("wrong-response", 1, true)
    }
}
struct WrongFactory;
impl McpClientServiceFactory for WrongFactory {
    fn create(
        &self,
        _config: &McpServerConfig,
    ) -> Result<Box<dyn DynService<RoleClient>>, McpError> {
        Ok(WrongResponse.into_dyn())
    }
}
#[tokio::test]
async fn form_wrong_host_result_is_typed_protocol_error() {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let selection =
        crate::client_service::ClientServiceSelection::select(&dummy_config(), Some(&WrongFactory))
            .unwrap();
    let (client, server) = tokio::join!(
        selection.serve(client_io),
        mcp_test_server::FormTestServer::default().serve(server_io)
    );
    let (client, server) = initialized_peers(client, server).await.unwrap();
    let result = AssertUnwindSafe(async {
        let response = form_handle(server.peer()).await.await_response().await;
        assert!(matches!(response, Err(ServiceError::McpError(error))
            if error.code == ErrorCode::INTERNAL_ERROR && error.message == "host returned a non-elicitation response"));
    }).catch_unwind().await;
    let mut errors = vec![];
    let _ = cleanup(&mut errors, "client close", client.cancel()).await;
    let _ = cleanup(&mut errors, "server close", server.cancel()).await;
    finish_test(result, errors);
}

#[tokio::test]
async fn form_existing_protocol_unit_constructor_stays_source_compatible() {
    let (client_io, server_io) = tokio::io::duplex(8192);
    let (client, server) = tokio::join!(
        ().serve(client_io),
        mcp_test_server::FormTestServer::default().serve(server_io)
    );
    let (client, server) = initialized_peers(client.map(ConnectedClient::from), server)
        .await
        .unwrap();
    let mut client = Some(client);
    let mut protocol = None;
    let result = AssertUnwindSafe(async {
        match client.take().unwrap() {
            ConnectedClient::Default(unit) => protocol = Some(McpProtocol::new(unit)),
            other => {
                client = Some(other);
                panic!("unit constructor unexpectedly selected a host");
            }
        }
        let response = protocol
            .as_ref()
            .unwrap()
            .call_tool_text("mcp_form", &json!({"message": "accept"}))
            .await;
        let value: Value = serde_json::from_str(&response.unwrap()).unwrap();
        assert_eq!(value["client_info"]["capabilities"], json!({}));
        assert_eq!(value["form"]["kind"], "result");
        assert_eq!(value["form"]["value"]["action"], "decline");
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(client) = client.take() {
        let _ = cleanup(&mut errors, "client close", client.cancel()).await;
    }
    if let Some(protocol) = protocol.take() {
        let _ = cleanup(&mut errors, "protocol close", protocol.close()).await;
    }
    let _ = cleanup(&mut errors, "server close", server.cancel()).await;
    finish_test(result, errors);
}

#[tokio::test]
async fn form_fixture_partial_initialization_joins_the_successful_native_peer() {
    use rmcp::model::{ClientJsonRpcMessage, ClientNotification, ServerJsonRpcMessage};
    let (client_tx, client_rx) = futures::channel::mpsc::unbounded::<ClientJsonRpcMessage>();
    let (server_tx, server_rx) = futures::channel::mpsc::unbounded::<ServerJsonRpcMessage>();
    // Fail only the final client handshake write. The actual rmcp server has
    // sent its InitializeResult and entered its native RunningService by then.
    let client_sink =
        futures::sink::unfold(client_tx, |tx, message: ClientJsonRpcMessage| async move {
            if matches!(&message, ClientJsonRpcMessage::Notification(notification)
            if matches!(&notification.notification, ClientNotification::InitializedNotification(_)))
            {
                return Err(std::io::Error::other("injected initialized write failure"));
            }
            tx.unbounded_send(message)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            Ok(tx)
        });
    let selection =
        crate::client_service::ClientServiceSelection::select(&dummy_config(), None).unwrap();
    let (client, server) = tokio::join!(
        selection.serve((Box::pin(client_sink), server_rx)),
        mcp_test_server::FormTestServer::default().serve((server_tx, client_rx)),
    );
    let exact_failure = matches!(&client, Err(rmcp::service::ClientInitializeError::TransportError { context, .. })
        if context.as_ref() == "send initialized notification");
    let server_initialized = server.is_ok();
    let outcome = initialized_peers(client, server).await;
    let failure = match outcome {
        Err(failure) => failure,
        Ok((client, server)) => {
            let mut errors = vec![];
            let _ = cleanup(&mut errors, "unexpected client close", client.cancel()).await;
            let _ = cleanup(&mut errors, "unexpected server close", server.cancel()).await;
            panic!("both peers initialized despite injected write failure; cleanup={errors:?}");
        }
    };
    // These are asserted only after every acquired native owner was joined.
    assert!(exact_failure);
    assert!(server_initialized);
    assert_eq!(failure.joined, ["server"]);
    assert_eq!(
        failure.errors.len(),
        1,
        "cleanup itself must succeed: {:?}",
        failure.errors
    );
    assert!(failure.errors[0].contains("injected initialized write failure"));
}
