//! Method router - dispatches JSON-RPC requests to the correct handler.

use std::sync::Arc;

#[cfg(feature = "comms")]
use futures::StreamExt;
#[cfg(feature = "comms")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "comms")]
use std::collections::HashMap;
#[cfg(feature = "comms")]
use tokio::sync::Mutex;
use tokio::sync::mpsc;
#[cfg(feature = "comms")]
use tokio::sync::oneshot;
#[cfg(feature = "comms")]
use tokio::task::JoinHandle;
#[cfg(feature = "comms")]
use uuid::Uuid;

use meerkat_core::ConfigStore;
#[cfg(feature = "comms")]
use meerkat_core::comms::StreamScope;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;

use crate::error;
use crate::handlers;
use crate::protocol::{RpcNotification, RpcRequest, RpcResponse};
use crate::session_runtime::SessionRuntime;

#[cfg(feature = "comms")]
use crate::handlers::comms::COMMS_STREAM_CONTRACT_VERSION;

// ---------------------------------------------------------------------------
// NotificationSink
// ---------------------------------------------------------------------------

/// Channel-based sink for sending notifications (agent events) back to the
/// client transport layer.
#[derive(Clone)]
pub struct NotificationSink {
    tx: mpsc::Sender<RpcNotification>,
}

impl NotificationSink {
    /// Create a new notification sink backed by the given channel sender.
    pub fn new(tx: mpsc::Sender<RpcNotification>) -> Self {
        Self { tx }
    }

    /// Emit an agent event as a JSON-RPC notification.
    pub async fn emit_event(&self, session_id: &SessionId, event: &AgentEvent) {
        let params = serde_json::json!({
            "session_id": session_id.to_string(),
            "event": event,
        });
        let notification = RpcNotification::new("session/event", params);
        // Best-effort: drop if the receiver is gone.
        let _ = self.tx.send(notification).await;
    }

    #[cfg(feature = "comms")]
    /// Emit a scoped comms stream event as a JSON-RPC notification.
    async fn emit_comms_stream_event(
        &self,
        stream_id: &Uuid,
        scope: &StreamScopeState,
        sequence: u64,
        event: &AgentEvent,
    ) {
        let params = serde_json::json!({
            "stream_id": stream_id.to_string(),
            "scope": scope,
            "sequence": sequence,
            "event": event,
            "contract_version": COMMS_STREAM_CONTRACT_VERSION,
        });
        let notification = RpcNotification::new("comms/stream_event", params);
        let _ = self.tx.send(notification).await;
    }
}

#[cfg(feature = "comms")]
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum StreamScopeState {
    Session { session_id: String },
    Interaction { interaction_id: String },
}

#[cfg(feature = "comms")]
impl StreamScopeState {
    fn session(session_id: &SessionId) -> Self {
        Self::Session {
            session_id: session_id.to_string(),
        }
    }

    fn interaction(id: &uuid::Uuid) -> Self {
        Self::Interaction {
            interaction_id: id.to_string(),
        }
    }
}

#[cfg(feature = "comms")]
struct StreamForwarder {
    state: StreamForwarderState,
}

#[cfg(feature = "comms")]
enum StreamForwarderState {
    Active {
        stop_tx: Option<oneshot::Sender<()>>,
        task: JoinHandle<()>,
    },
    Closed,
}

// ---------------------------------------------------------------------------
// MethodRouter
// ---------------------------------------------------------------------------

/// Dispatches incoming JSON-RPC requests to the appropriate handler.
#[derive(Clone)]
pub struct MethodRouter {
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    notification_sink: NotificationSink,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    #[cfg(feature = "comms")]
    active_streams: Arc<Mutex<HashMap<Uuid, StreamForwarder>>>,
}

impl MethodRouter {
    /// Create a new method router.
    pub fn new(
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        notification_sink: NotificationSink,
    ) -> Self {
        Self {
            runtime,
            config_store,
            notification_sink,
            skill_runtime: None,
            #[cfg(feature = "comms")]
            active_streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set the skill runtime for introspection methods.
    pub fn with_skill_runtime(
        mut self,
        runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    ) -> Self {
        self.skill_runtime = runtime;
        self
    }

    /// Dispatch a request to the appropriate handler.
    ///
    /// Returns `None` for notifications (requests without an id) that do not
    /// require a response.
    pub async fn dispatch(&self, request: RpcRequest) -> Option<RpcResponse> {
        // Notifications (no id) are fire-and-forget
        if request.is_notification() {
            // Handle known notification methods silently
            match request.method.as_str() {
                "initialized" => { /* no-op ack */ }
                _ => {
                    tracing::debug!("Unknown notification method: {}", request.method);
                }
            }
            return None;
        }

        let id = request.id.clone();
        let params = request.params.as_deref();

        let response = match request.method.as_str() {
            "initialize" => handlers::initialize::handle_initialize(id),
            "session/create" => {
                handlers::session::handle_create(
                    id,
                    params,
                    self.runtime.clone(),
                    &self.notification_sink,
                )
                .await
            }
            "session/list" => handlers::session::handle_list(id, &self.runtime).await,
            "session/read" => handlers::session::handle_read(id, params, &self.runtime).await,
            "session/archive" => handlers::session::handle_archive(id, params, &self.runtime).await,
            "turn/start" => {
                handlers::turn::handle_start(id, params, &self.runtime, &self.notification_sink)
                    .await
            }
            "turn/interrupt" => handlers::turn::handle_interrupt(id, params, &self.runtime).await,
            #[cfg(feature = "comms")]
            "comms/send" => handlers::comms::handle_send(id, params, &self.runtime).await,
            #[cfg(feature = "comms")]
            "comms/peers" => handlers::comms::handle_peers(id, params, &self.runtime).await,
            #[cfg(feature = "comms")]
            "comms/stream_open" => self.handle_comms_stream_open(id, params).await,
            #[cfg(feature = "comms")]
            "comms/stream_close" => self.handle_comms_stream_close(id, params).await,
            // M12: event/push removed. Use comms/send instead.
            "skills/list" => handlers::skills::handle_list(id, &self.skill_runtime).await,
            "skills/inspect" => {
                handlers::skills::handle_inspect(id, params, &self.skill_runtime).await
            }
            "capabilities/get" => {
                let config = self.config_store.get().await.unwrap_or_default();
                handlers::capabilities::handle_get(id, &config)
            }
            "config/get" => {
                handlers::config::handle_get(id, &self.config_store, self.runtime.config_runtime())
                    .await
            }
            "config/set" => {
                handlers::config::handle_set(
                    id,
                    params,
                    &self.runtime,
                    &self.config_store,
                    self.runtime.config_runtime(),
                )
                .await
            }
            "config/patch" => {
                handlers::config::handle_patch(
                    id,
                    params,
                    &self.runtime,
                    &self.config_store,
                    self.runtime.config_runtime(),
                )
                .await
            }
            _ => RpcResponse::error(
                id,
                error::METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
            ),
        };

        Some(response)
    }

    /// Access the underlying session runtime.
    pub fn runtime(&self) -> &SessionRuntime {
        &self.runtime
    }

    #[cfg(feature = "comms")]
    async fn handle_comms_stream_open(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        use crate::error;
        use meerkat_core::InteractionId;
        use serde_json::json;

        #[derive(Deserialize)]
        struct CommsStreamOpenParams {
            session_id: String,
            scope: String,
            #[serde(default)]
            interaction_id: Option<String>,
        }

        let params = match handlers::parse_params::<CommsStreamOpenParams>(params) {
            Ok(p) => p,
            Err(resp) => return resp,
        };

        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };

        let comms = match self.runtime.comms_runtime(&session_id).await {
            Some(comms) => comms,
            None => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Session not found or comms not enabled: {session_id}"),
                );
            }
        };

        let (stream_scope, scope_state) = match params.scope.as_str() {
            "session" => (
                StreamScope::Session(session_id.clone()),
                StreamScopeState::session(&session_id),
            ),
            "interaction" => {
                let interaction_id = match &params.interaction_id {
                    Some(value) => match uuid::Uuid::parse_str(value) {
                        Ok(interaction_id) => interaction_id,
                        Err(_) => {
                            return RpcResponse::error(
                                id,
                                error::INVALID_PARAMS,
                                format!("Invalid interaction ID: {value}"),
                            );
                        }
                    },
                    None => {
                        return RpcResponse::error(
                            id,
                            error::INVALID_PARAMS,
                            "scope 'interaction' requires interaction_id",
                        );
                    }
                };

                (
                    StreamScope::Interaction(InteractionId(interaction_id)),
                    StreamScopeState::interaction(&interaction_id),
                )
            }
            other => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Unknown stream scope: {other}"),
                );
            }
        };

        let stream = match comms.stream(stream_scope.clone()) {
            Ok(stream) => stream,
            Err(err) => {
                return RpcResponse::error(id, error::INVALID_PARAMS, err.to_string());
            }
        };

        let stream_id = Uuid::new_v4();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let notification_sink = self.notification_sink.clone();
        let active_streams = self.active_streams.clone();
        let stream_id_for_task = stream_id;

        let task = tokio::spawn(async move {
            let mut stream = stream;
            let mut stop_rx = stop_rx;
            let mut sequence = 0u64;

            loop {
                tokio::select! {
                    _ = &mut stop_rx => {
                        break;
                    }
                    event = stream.next() => {
                        match event {
                            Some(event) => {
                                sequence += 1;
                                notification_sink
                                    .emit_comms_stream_event(
                                        &stream_id_for_task,
                                        &scope_state,
                                        sequence,
                                        &event,
                                    )
                                    .await;
                            }
                            None => break,
                        }
                    }
                }
            }

            let mut active_streams = active_streams.lock().await;
            if active_streams
                .get(&stream_id_for_task)
                .is_some_and(|stream| matches!(stream.state, StreamForwarderState::Active { .. }))
            {
                active_streams.remove(&stream_id_for_task);
            }
        });

        self.active_streams.lock().await.insert(
            stream_id,
            StreamForwarder {
                state: StreamForwarderState::Active {
                    stop_tx: Some(stop_tx),
                    task,
                },
            },
        );

        RpcResponse::success(
            id,
            json!({
                "stream_id": stream_id.to_string(),
                "opened": true,
            }),
        )
    }

    #[cfg(feature = "comms")]
    async fn handle_comms_stream_close(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        use crate::error;
        use serde_json::json;

        #[derive(Deserialize)]
        struct CommsStreamCloseParams {
            stream_id: String,
        }

        let params = match handlers::parse_params::<CommsStreamCloseParams>(params) {
            Ok(p) => p,
            Err(resp) => return resp,
        };

        let stream_id = match Uuid::parse_str(&params.stream_id) {
            Ok(stream_id) => stream_id,
            Err(_) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid stream_id: {}", params.stream_id),
                );
            }
        };

        let mut active_streams = self.active_streams.lock().await;
        let already_closed = match active_streams.get_mut(&stream_id) {
            Some(stream) => match &mut stream.state {
                StreamForwarderState::Active { stop_tx, task } => {
                    if let Some(stop_tx) = stop_tx.take() {
                        let _ = stop_tx.send(());
                    }
                    task.abort();
                    stream.state = StreamForwarderState::Closed;
                    false
                }
                StreamForwarderState::Closed => true,
            },
            None => false,
        };

        RpcResponse::success(
            id,
            json!({
                "stream_id": stream_id.to_string(),
                "closed": true,
                "already_closed": already_closed,
            }),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures::stream;
    use meerkat::AgentFactory;
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::skills::{
        SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage, SourceIdentityLineageEvent,
        SourceIdentityRecord, SourceIdentityRegistry, SourceIdentityStatus, SourceTransportKind,
        SourceUuid,
    };
    use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, StopReason};
    use serde::Serialize;

    use crate::protocol::RpcId;

    // -----------------------------------------------------------------------
    // Mock LLM client (same as session_runtime tests)
    // -----------------------------------------------------------------------

    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
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

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    async fn test_router() -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(factory, config, 10, store);
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new(runtime, config_store, sink);
        (router, notif_rx)
    }

    async fn test_router_with_registry(
        registry: SourceIdentityRegistry,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(factory, config, 10, store);
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        runtime.set_skill_identity_registry(registry);
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new(runtime, config_store, sink);
        (router, notif_rx)
    }

    fn source_uuid(raw: &str) -> SourceUuid {
        SourceUuid::parse(raw).expect("valid source uuid")
    }

    fn skill_name(raw: &str) -> SkillName {
        SkillName::parse(raw).expect("valid skill name")
    }

    fn key(source: &str, name: &str) -> SkillKey {
        SkillKey {
            source_uuid: source_uuid(source),
            skill_name: skill_name(name),
        }
    }

    fn record(source: &str, fingerprint: &str) -> SourceIdentityRecord {
        SourceIdentityRecord {
            source_uuid: source_uuid(source),
            display_name: source.to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: fingerprint.to_string(),
            status: SourceIdentityStatus::Active,
        }
    }

    fn alias_registry() -> SourceIdentityRegistry {
        SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
                record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
            ],
            vec![SourceIdentityLineage {
                event_id: "rotate-1".to_string(),
                recorded_at_unix_secs: 1,
                required_from_skills: vec![skill_name("email-extractor")],
                event: SourceIdentityLineageEvent::Rotate {
                    from: source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                    to: source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                },
            }],
            vec![SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
                to: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor"),
                reason: Some("rotate".to_string()),
            }],
            vec![meerkat_core::skills::SkillAlias {
                alias: "legacy/email".to_string(),
                to: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
            }],
        )
        .expect("registry")
    }

    fn make_request(method: &str, params: impl Serialize) -> RpcRequest {
        let params_raw =
            serde_json::value::RawValue::from_string(serde_json::to_string(&params).unwrap())
                .unwrap();
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: method.to_string(),
            params: Some(params_raw),
        }
    }

    fn make_request_no_params(method: &str) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: method.to_string(),
            params: None,
        }
    }

    fn make_notification(method: &str) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.to_string(),
            params: None,
        }
    }

    /// Extract the result JSON value from a successful response.
    fn result_value(resp: &RpcResponse) -> serde_json::Value {
        assert!(
            resp.error.is_none(),
            "Expected success response, got error: {:?}",
            resp.error
        );
        let raw = resp
            .result
            .as_ref()
            .expect("Missing result in success response");
        serde_json::from_str(raw.get()).unwrap()
    }

    /// Extract the error code from an error response.
    fn error_code(resp: &RpcResponse) -> i32 {
        resp.error.as_ref().expect("Expected error response").code
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// 1. `initialize` returns server capabilities with server info and methods.
    #[tokio::test]
    async fn initialize_returns_capabilities() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request_no_params("initialize");

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);

        // Verify server info
        assert_eq!(result["server_info"]["name"], "meerkat-rpc");
        assert!(result["server_info"]["version"].is_string());

        // Verify methods list includes expected methods
        let methods = result["methods"].as_array().unwrap();
        let method_names: Vec<&str> = methods.iter().map(|m| m.as_str().unwrap()).collect();
        assert!(method_names.contains(&"initialize"));
        assert!(method_names.contains(&"session/create"));
        assert!(method_names.contains(&"turn/start"));
        assert!(method_names.contains(&"config/get"));
        #[cfg(feature = "comms")]
        {
            assert!(method_names.contains(&"comms/stream_open"));
            assert!(method_names.contains(&"comms/stream_close"));
        }
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn comms_stream_open_requires_reserved_interaction() {
        use tokio::time::{Duration, timeout};

        let (router, _notif_rx) = test_router().await;

        // Create a session.
        let create_req = make_request(
            "session/create",
            serde_json::json!({
                "prompt":"Hello",
                "host_mode": true,
                "comms_name": "router-stream-test",
            }),
        );
        let create_resp = timeout(Duration::from_secs(5), router.dispatch(create_req))
            .await
            .unwrap()
            .unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        // session scope is not supported by runtime stream implementation.
        let open_session_scope = make_request(
            "comms/stream_open",
            serde_json::json!({"session_id": session_id, "scope": "session"}),
        );
        let session_stream_resp =
            timeout(Duration::from_secs(5), router.dispatch(open_session_scope))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(error_code(&session_stream_resp), error::INVALID_PARAMS);

        // Reserve interaction stream via send.
        let send_req = make_request(
            "comms/send",
            serde_json::json!({
                "session_id": created["session_id"].as_str().unwrap(),
                "kind": "input",
                "body": "hello",
                "stream": "reserve_interaction",
                "allow_self_session": true
            }),
        );
        let send_resp = timeout(Duration::from_secs(5), router.dispatch(send_req))
            .await
            .unwrap()
            .unwrap();
        let send_result = result_value(&send_resp);
        assert_eq!(send_result["kind"], "input_accepted");
        let interaction_id = send_result["interaction_id"].as_str().unwrap();

        // Open stream and close it immediately.
        let open_req = make_request(
            "comms/stream_open",
            serde_json::json!({
                "session_id": created["session_id"].as_str().unwrap(),
                "scope": "interaction",
                "interaction_id": interaction_id,
            }),
        );
        let open_resp = timeout(Duration::from_secs(5), router.dispatch(open_req))
            .await
            .unwrap()
            .unwrap();
        assert!(open_resp.error.is_none());
        let opened = result_value(&open_resp);
        let stream_id = opened["stream_id"].as_str().unwrap();

        let close_req = make_request(
            "comms/stream_close",
            serde_json::json!({"stream_id": stream_id}),
        );
        let close_resp = timeout(Duration::from_secs(5), router.dispatch(close_req))
            .await
            .unwrap()
            .unwrap();
        let closed = result_value(&close_resp);
        assert_eq!(closed["closed"], true);
        assert_eq!(closed["already_closed"], false);

        // Idempotent: second close succeeds with already_closed=true.
        let close_again_req = make_request(
            "comms/stream_close",
            serde_json::json!({"stream_id": stream_id}),
        );
        let close_again_resp = timeout(Duration::from_secs(5), router.dispatch(close_again_req))
            .await
            .unwrap()
            .unwrap();
        let close_again = result_value(&close_again_resp);
        assert_eq!(close_again["closed"], true);
        assert_eq!(close_again["already_closed"], true);
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn comms_stream_close_unknown_returns_not_closed() {
        let (router, _notif_rx) = test_router().await;

        let close_req = make_request(
            "comms/stream_close",
            serde_json::json!({"stream_id": "00000000-0000-0000-0000-000000000000"}),
        );
        let close_resp = router.dispatch(close_req).await.unwrap();
        let closed = result_value(&close_resp);
        assert_eq!(closed["closed"], true);
        assert_eq!(closed["already_closed"], false);
    }

    /// 2. Unknown method returns METHOD_NOT_FOUND error.
    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request_no_params("foo/bar");

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::METHOD_NOT_FOUND);
    }

    /// 3. `session/create` happy path - creates session, runs first turn, returns result.
    #[tokio::test]
    async fn session_create_returns_session_id_and_result() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello"
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);

        // session_id should be a non-empty string
        let sid = result["session_id"].as_str().unwrap();
        assert!(!sid.is_empty());

        // text should contain the mock response
        let text = result["text"].as_str().unwrap();
        assert!(
            text.contains("Hello from mock"),
            "Expected mock text, got: {text}"
        );

        // turns and tool_calls should be present
        assert!(result["turns"].is_u64());
        assert!(result["tool_calls"].is_u64());

        // usage should be present
        assert!(result["usage"]["input_tokens"].is_u64());
        assert!(result["usage"]["output_tokens"].is_u64());
    }

    /// 4. `session/create` with missing prompt returns INVALID_PARAMS.
    #[tokio::test]
    async fn session_create_missing_prompt_returns_invalid_params() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "model": "claude-sonnet-4-5"
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
    }

    /// 5. `session/list` returns the list of sessions after creating one.
    #[tokio::test]
    async fn session_list_returns_sessions() {
        let (router, _notif_rx) = test_router().await;

        // Create a session first
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let created_id = created["session_id"].as_str().unwrap();

        // Now list sessions
        let list_req = make_request_no_params("session/list");
        let list_resp = router.dispatch(list_req).await.unwrap();
        let list_result = result_value(&list_resp);

        let sessions = list_result["sessions"].as_array().unwrap();
        assert!(!sessions.is_empty(), "Should have at least one session");

        // Find our session in the list
        let found = sessions
            .iter()
            .any(|s| s["session_id"].as_str() == Some(created_id));
        assert!(found, "Created session should appear in list");
    }

    /// 6. `session/archive` removes a session.
    #[tokio::test]
    async fn session_archive_removes_session() {
        let (router, _notif_rx) = test_router().await;

        // Create a session
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        // Archive it
        let archive_req = make_request(
            "session/archive",
            serde_json::json!({"session_id": session_id}),
        );
        let archive_resp = router.dispatch(archive_req).await.unwrap();
        let archive_result = result_value(&archive_resp);
        assert_eq!(archive_result["archived"], true);

        // Verify it's gone from list
        let list_req = make_request_no_params("session/list");
        let list_resp = router.dispatch(list_req).await.unwrap();
        let list_result = result_value(&list_resp);
        let sessions = list_result["sessions"].as_array().unwrap();
        let found = sessions
            .iter()
            .any(|s| s["session_id"].as_str() == Some(&session_id));
        assert!(!found, "Archived session should not appear in list");
    }

    /// 7. `turn/start` returns a result for an existing session.
    #[tokio::test]
    async fn turn_start_returns_result() {
        let (router, _notif_rx) = test_router().await;

        // Create a session first
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        // Start another turn
        let turn_req = make_request(
            "turn/start",
            serde_json::json!({
                "session_id": session_id,
                "prompt": "Follow up"
            }),
        );
        let turn_resp = router.dispatch(turn_req).await.unwrap();
        let turn_result = result_value(&turn_resp);

        assert_eq!(turn_result["session_id"].as_str().unwrap(), session_id);
        let text = turn_result["text"].as_str().unwrap();
        assert!(
            text.contains("Hello from mock"),
            "Expected mock text in turn, got: {text}"
        );
    }

    /// 8. `turn/start` emits notifications via the notification sink.
    #[tokio::test]
    async fn turn_start_emits_notifications() {
        let (router, mut notif_rx) = test_router().await;

        // Create a session
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let _create_resp = router.dispatch(create_req).await.unwrap();

        // Give the event forwarder task a moment to send notifications
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Check that we received at least one notification
        let mut notifications = Vec::new();
        while let Ok(notif) = notif_rx.try_recv() {
            notifications.push(notif);
        }

        assert!(
            !notifications.is_empty(),
            "Should have received at least one notification"
        );

        // All notifications should be session/event
        for notif in &notifications {
            assert_eq!(notif.method, "session/event");
            assert!(notif.params["session_id"].is_string());
            assert!(notif.params["event"].is_object());
        }
    }

    /// 8b. `session/create` accepts structured skill refs at wire boundary.
    #[tokio::test]
    async fn session_create_accepts_structured_skill_refs() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_refs": [
                    {
                        "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                        "skill_name": "email-extractor"
                    }
                ]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);
        assert!(result["session_id"].as_str().is_some());
        assert!(result["text"].as_str().unwrap().contains("Hello from mock"));
    }

    /// 8b2. Alias refs resolve through configured non-default identity registry.
    #[tokio::test]
    async fn session_create_accepts_alias_ref_with_registry() {
        let (router, _notif_rx) = test_router_with_registry(alias_registry()).await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_references": ["legacy/email"]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);
        assert!(result["session_id"].as_str().is_some());
        assert!(result["text"].as_str().unwrap().contains("Hello from mock"));
    }

    /// 8c. `turn/start` accepts legacy+structured refs together.
    #[tokio::test]
    async fn turn_start_accepts_mixed_skill_ref_formats() {
        let (router, _notif_rx) = test_router().await;

        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        let turn_req = make_request(
            "turn/start",
            serde_json::json!({
                "session_id": session_id,
                "prompt": "Follow up",
                "skill_refs": [{
                    "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                    "skill_name": "email-extractor"
                }],
                "skill_references": ["dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"]
            }),
        );

        let turn_resp = router.dispatch(turn_req).await.unwrap();
        let turn_result = result_value(&turn_resp);
        assert!(
            turn_result["text"]
                .as_str()
                .unwrap()
                .contains("Hello from mock")
        );
    }

    /// 8d. Invalid structured refs fail deterministically at the wire boundary.
    #[tokio::test]
    async fn session_create_rejects_invalid_structured_skill_ref() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_refs": [
                    {
                        "source_uuid": "not-a-uuid",
                        "skill_name": "email-extractor"
                    }
                ]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
    }

    /// 8e. Unknown aliases fail deterministically with configured registry.
    #[tokio::test]
    async fn session_create_rejects_unknown_alias_with_registry() {
        let (router, _notif_rx) = test_router_with_registry(alias_registry()).await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_references": ["legacy/unknown"]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
    }

    /// 9. `turn/interrupt` on an idle session returns ok.
    #[tokio::test]
    async fn turn_interrupt_returns_ok() {
        let (router, _notif_rx) = test_router().await;

        // Create a session
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        // Interrupt (session is idle, should be a no-op success)
        let interrupt_req = make_request(
            "turn/interrupt",
            serde_json::json!({"session_id": session_id}),
        );
        let interrupt_resp = router.dispatch(interrupt_req).await.unwrap();
        let interrupt_result = result_value(&interrupt_resp);
        assert_eq!(interrupt_result["interrupted"], true);
    }

    /// 10. `config/get` returns the default config.
    #[tokio::test]
    async fn config_get_returns_config() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request_no_params("config/get");

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);

        // Config response should be an envelope with config + metadata
        assert!(result.is_object(), "Config should be a JSON object");
        assert!(
            result.get("config").is_some(),
            "Config envelope should include 'config'"
        );
        assert!(
            result
                .get("config")
                .and_then(|cfg| cfg.get("agent"))
                .is_some(),
            "Config envelope should have 'config.agent' field"
        );
    }

    /// 11. `config/set` then `config/get` roundtrip.
    #[tokio::test]
    async fn config_set_and_get_roundtrip() {
        let (router, _notif_rx) = test_router().await;

        // Get the current config
        let get_req = make_request_no_params("config/get");
        let get_resp = router.dispatch(get_req).await.unwrap();
        let mut config = result_value(&get_resp)["config"].clone();

        // Modify max_tokens
        config["max_tokens"] = serde_json::json!(2048);

        // Set the modified config
        let set_req = make_request("config/set", serde_json::json!({ "config": config }));
        let set_resp = router.dispatch(set_req).await.unwrap();
        let set_result = result_value(&set_resp);
        assert_eq!(set_result["config"]["max_tokens"], 2048);
        assert!(set_result["generation"].as_u64().is_some());

        // Get again and verify
        let get_req2 = make_request_no_params("config/get");
        let get_resp2 = router.dispatch(get_req2).await.unwrap();
        let config2 = result_value(&get_resp2);
        assert_eq!(config2["config"]["max_tokens"], 2048);
    }

    /// 12. `config/patch` merges a delta.
    #[tokio::test]
    async fn config_patch_merges_delta() {
        let (router, _notif_rx) = test_router().await;

        // Get initial max_tokens
        let get_req = make_request_no_params("config/get");
        let get_resp = router.dispatch(get_req).await.unwrap();
        let initial = result_value(&get_resp);
        let initial_max_tokens = initial["config"]["max_tokens"].as_u64().unwrap();

        // Patch max_tokens to a different value
        let new_max_tokens = initial_max_tokens + 1000;
        let patch_req = make_request(
            "config/patch",
            serde_json::json!({"max_tokens": new_max_tokens}),
        );
        let patch_resp = router.dispatch(patch_req).await.unwrap();
        let patched = result_value(&patch_resp);
        assert_eq!(patched["config"]["max_tokens"], new_max_tokens);

        // Verify via get
        let get_req2 = make_request_no_params("config/get");
        let get_resp2 = router.dispatch(get_req2).await.unwrap();
        let final_config = result_value(&get_resp2);
        assert_eq!(final_config["config"]["max_tokens"], new_max_tokens);
    }

    /// 12b. `config/patch` refreshes runtime identity registry used by session handlers.
    #[tokio::test]
    async fn config_patch_refreshes_identity_registry_for_alias_resolution() {
        let (router, _notif_rx) = test_router().await;

        let fail_before = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "hello",
                "skill_references": ["legacy/email"]
            }),
        );
        let fail_before_resp = router.dispatch(fail_before).await.unwrap();
        assert_eq!(error_code(&fail_before_resp), error::INVALID_PARAMS);

        let set_req = make_request(
            "config/patch",
            serde_json::json!({
                "patch": {
                    "skills": {
                        "repositories": [
                            {
                                "name": "legacy-source",
                                "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                                "type": "filesystem",
                                "path": ".rkat/skills/legacy"
                            },
                            {
                                "name": "new-source",
                                "source_uuid": "a93d587d-8f44-438f-8189-6e8cf549f6e7",
                                "type": "filesystem",
                                "path": ".rkat/skills/new"
                            }
                        ],
                        "identity": {
                            "aliases": [{
                                "alias": "legacy/email",
                                "to": {
                                    "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                                    "skill_name": "email-extractor"
                                }
                            }]
                        }
                    }
                }
            }),
        );
        let set_resp = router.dispatch(set_req).await.unwrap();
        assert!(
            set_resp.error.is_none(),
            "config/patch failed unexpectedly: {:?}",
            set_resp.error
        );

        let success_after = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "hello",
                "skill_references": ["legacy/email"]
            }),
        );
        let success_after_resp = router.dispatch(success_after).await.unwrap();
        let result = result_value(&success_after_resp);
        assert!(result["session_id"].as_str().is_some());
    }

    /// 12c. Invalid identity configs are rejected on patch and do not advance generation.
    #[tokio::test]
    async fn config_patch_rejects_invalid_identity_registry_update() {
        let (router, _notif_rx) = test_router().await;

        let before = make_request_no_params("config/get");
        let before_resp = router.dispatch(before).await.unwrap();
        let before_value = result_value(&before_resp);
        let generation_before = before_value["generation"].as_u64().unwrap_or(0);

        let patch_req = make_request(
            "config/patch",
            serde_json::json!({
                "patch": {
                    "skills": {
                        "identity": {
                            "lineage": [{
                                "event_id": "split-1",
                                "recorded_at_unix_secs": 1,
                                "event": {
                                    "type": "split",
                                    "from": "dc256086-0d2f-4f61-a307-320d4148107f",
                                    "into": [
                                        "a93d587d-8f44-438f-8189-6e8cf549f6e7",
                                        "e8df561d-d38f-4242-af55-3a6efb34c950"
                                    ]
                                }
                            }],
                            "remaps": []
                        }
                    }
                }
            }),
        );
        let patch_resp = router.dispatch(patch_req).await.unwrap();
        assert_eq!(error_code(&patch_resp), error::INVALID_PARAMS);

        let after = make_request_no_params("config/get");
        let after_resp = router.dispatch(after).await.unwrap();
        let after_value = result_value(&after_resp);
        let generation_after = after_value["generation"].as_u64().unwrap_or(0);
        assert_eq!(generation_after, generation_before);
    }

    /// 13. A notification (request with no id) returns None (no response).
    #[tokio::test]
    async fn notification_is_silently_dropped() {
        let (router, _notif_rx) = test_router().await;
        let req = make_notification("initialized");

        let resp = router.dispatch(req).await;
        assert!(resp.is_none(), "Notifications should return None");
    }
}
