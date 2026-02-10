//! Method router - dispatches JSON-RPC requests to the correct handler.

use std::sync::Arc;

use tokio::sync::mpsc;

use meerkat_core::ConfigStore;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;

use crate::error;
use crate::handlers;
use crate::protocol::{RpcNotification, RpcRequest, RpcResponse};
use crate::session_runtime::SessionRuntime;

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
        }
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
                handlers::session::handle_create(id, params, &self.runtime, &self.notification_sink)
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
            "capabilities/get" => {
                let config = self.config_store.get().await.unwrap_or_default();
                handlers::capabilities::handle_get(id, &config)
            }
            "config/get" => handlers::config::handle_get(id, &self.config_store).await,
            "config/set" => handlers::config::handle_set(id, params, &self.config_store).await,
            "config/patch" => handlers::config::handle_patch(id, params, &self.config_store).await,
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
    use meerkat_core::{Config, MemoryConfigStore, StopReason};
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
        let mut runtime = SessionRuntime::new(factory, config, 10);
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));
        let runtime = Arc::new(runtime);
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new(runtime, config_store, sink);
        (router, notif_rx)
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

        // Config should be an object with known fields
        assert!(result.is_object(), "Config should be a JSON object");
        assert!(
            result.get("agent").is_some(),
            "Config should have 'agent' field"
        );
    }

    /// 11. `config/set` then `config/get` roundtrip.
    #[tokio::test]
    async fn config_set_and_get_roundtrip() {
        let (router, _notif_rx) = test_router().await;

        // Get the current config
        let get_req = make_request_no_params("config/get");
        let get_resp = router.dispatch(get_req).await.unwrap();
        let mut config = result_value(&get_resp);

        // Modify max_tokens
        config["max_tokens"] = serde_json::json!(2048);

        // Set the modified config
        let set_req = make_request("config/set", &config);
        let set_resp = router.dispatch(set_req).await.unwrap();
        let set_result = result_value(&set_resp);
        assert_eq!(set_result["ok"], true);

        // Get again and verify
        let get_req2 = make_request_no_params("config/get");
        let get_resp2 = router.dispatch(get_req2).await.unwrap();
        let config2 = result_value(&get_resp2);
        assert_eq!(config2["max_tokens"], 2048);
    }

    /// 12. `config/patch` merges a delta.
    #[tokio::test]
    async fn config_patch_merges_delta() {
        let (router, _notif_rx) = test_router().await;

        // Get initial max_tokens
        let get_req = make_request_no_params("config/get");
        let get_resp = router.dispatch(get_req).await.unwrap();
        let initial = result_value(&get_resp);
        let initial_max_tokens = initial["max_tokens"].as_u64().unwrap();

        // Patch max_tokens to a different value
        let new_max_tokens = initial_max_tokens + 1000;
        let patch_req = make_request(
            "config/patch",
            serde_json::json!({"max_tokens": new_max_tokens}),
        );
        let patch_resp = router.dispatch(patch_req).await.unwrap();
        let patched = result_value(&patch_resp);
        assert_eq!(patched["max_tokens"], new_max_tokens);

        // Verify via get
        let get_req2 = make_request_no_params("config/get");
        let get_resp2 = router.dispatch(get_req2).await.unwrap();
        let final_config = result_value(&get_resp2);
        assert_eq!(final_config["max_tokens"], new_max_tokens);
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
