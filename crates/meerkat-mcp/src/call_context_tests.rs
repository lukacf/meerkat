//! Actual rmcp HTTP calls through the native router and its context adapter.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{
    McpCallContext, McpCallContextError, McpCallContextProvider, McpCallTarget, McpRouterAdapter,
};
use futures::FutureExt;
use meerkat_core::{SessionId, ToolDispatchContext, ToolMutationClass};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::json;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(10);
const PRIVATE_KEY: &str = "io.example/call";

#[derive(Clone)]
struct Server {
    requests: Arc<std::sync::Mutex<Vec<Value>>>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
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
        let mut result = ListToolsResult::default();
        result.tools.push(Tool::new(
            "read",
            "context fixture",
            json!({"type":"object"}).as_object().unwrap().clone(),
        ));
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.requests.lock().unwrap().push(
            json!({"name": request.name, "arguments": request.arguments, "meta": context.meta}),
        );
        let action = request
            .arguments
            .as_ref()
            .and_then(|args| args.get("action"))
            .and_then(Value::as_str)
            .unwrap_or("ok");
        if action == "block" {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        if action == "protocol_error" {
            return Err(ErrorData::invalid_params("fixture refusal", None));
        }
        let mut result =
            CallToolResult::success(vec![Content::text("first"), Content::text("last")]);
        if action == "error" {
            result.is_error = Some(true);
            result.structured_content = Some(json!({"code":"fixture_error"}));
        }
        Ok(result)
    }
}

struct Fixture {
    server: Server,
    config: McpServerConfig,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Fixture {
    async fn start() -> Self {
        let server = Server {
            requests: Default::default(),
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
        };
        let stop = CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.stateful_mode = false;
        config.json_response = true;
        config.cancellation_token = stop.child_token();
        let handler = server.clone();
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let app = axum::Router::new().nest_service("/mcp", service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stopping = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stopping.cancelled_owned())
                .await
        });
        Self {
            server,
            config: McpServerConfig::streamable_http(
                "context-fixture",
                format!("http://{address}/mcp"),
                Default::default(),
            ),
            stop,
            task,
        }
    }

    async fn router(
        &self,
        provider: Option<Arc<dyn McpCallContextProvider>>,
        config: McpServerConfig,
    ) -> McpRouter {
        let mut router = McpRouter::new_with_surface_handle(Arc::new(
            meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral(),
        ));
        if let Some(provider) = provider {
            router = router.with_call_context_provider(provider);
        }
        tokio::time::timeout(LIMIT, router.add_server(config))
            .await
            .unwrap()
            .unwrap();
        router
    }

    async fn finish(self, result: std::thread::Result<()>) {
        self.server.release.add_permits(32);
        self.stop.cancel();
        let joined = tokio::time::timeout(LIMIT, self.task).await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        joined.unwrap().unwrap().unwrap();
    }
}

struct Lease(Arc<AtomicUsize>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Provider {
    selected: McpServerConfig,
    observed: std::sync::Mutex<Vec<(SessionId, crate::McpConnectionId, Value)>>,
    prepared: AtomicUsize,
    dropped: Arc<AtomicUsize>,
    reserved: bool,
    unselected_class: ToolMutationClass,
    restrictions: std::sync::Mutex<Vec<bool>>,
}

impl Provider {
    fn new(selected: McpServerConfig) -> Self {
        Self {
            selected,
            observed: Default::default(),
            prepared: AtomicUsize::new(0),
            dropped: Default::default(),
            reserved: false,
            unselected_class: ToolMutationClass::Unknown,
            restrictions: Default::default(),
        }
    }
}

#[async_trait]
impl McpCallContextProvider for Provider {
    async fn prepare(
        &self,
        target: McpCallTarget<'_>,
        _: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        if target.config != &self.selected {
            return Ok(None);
        }
        assert_eq!(target.raw_operation, "read");
        self.restrictions
            .lock()
            .unwrap()
            .push(context.read_only_execution_required());
        let session = context
            .origin_session_id()
            .ok_or(McpCallContextError::Unavailable)?;
        let origin = serde_json::to_value(target.origin).unwrap();
        self.observed
            .lock()
            .unwrap()
            .push((session.clone(), target.connection_id, origin));
        let sequence = self.prepared.fetch_add(1, Ordering::SeqCst);
        let key = if self.reserved {
            meerkat_core::CALL_ORIGIN_META_KEY
        } else {
            PRIVATE_KEY
        };
        Ok(Some(McpCallContext::new(
            serde_json::Map::from_iter([(
                key.into(),
                json!({"correlation":format!("private-{sequence}")}),
            )]),
            Lease(Arc::clone(&self.dropped)),
        )))
    }

    fn tool_mutation_class(
        &self,
        config: &McpServerConfig,
        raw_operation: &str,
    ) -> ToolMutationClass {
        if config == &self.selected && raw_operation == "read" {
            ToolMutationClass::ReadOnly
        } else {
            self.unselected_class
        }
    }
}

fn context() -> ToolDispatchContext {
    ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None)
}
fn resolution() -> meerkat_core::ToolExecutionResolutionContext {
    meerkat_core::ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            LIMIT,
        )])
        .unwrap(),
    )
}

#[tokio::test]
async fn actual_context_alias_and_error_envelope_survive_the_shared_adapter() {
    let fixture = Fixture::start().await;
    let mut config = fixture.config.clone();
    config
        .tool_names
        .insert("read".into(), "workspace_read".into());
    let provider = Arc::new(Provider::new(config.clone()));
    let router = fixture.router(Some(provider.clone()), config).await;
    let adapter = McpRouterAdapter::new(router);
    let result = AssertUnwindSafe(async {
        assert_eq!(
            adapter.tool_mutation_class("workspace_read"),
            ToolMutationClass::ReadOnly
        );
        assert_eq!(
            adapter.tool_mutation_class("read"),
            ToolMutationClass::Unknown
        );
        let first = context();
        let second = context();
        let args = serde_json::value::to_raw_value(
            &json!({"action":"error", "session_id":"model-forgery"}),
        )
        .unwrap();
        let call = ToolCallView {
            id: "context-call",
            name: "workspace_read",
            args: &args,
        };
        let plain = adapter.dispatch_with_context(call, &first).await.unwrap();
        assert!(plain.result.is_error);
        assert_eq!(plain.result.content.len(), 3);
        let plan = adapter
            .resolve_execution_plan(call, &second, &resolution())
            .unwrap();
        let resolved = adapter
            .dispatch_resolved_with_context(call, &second, &plan)
            .await
            .unwrap();
        assert!(resolved.result.is_error);
        let observed = provider.observed.lock().unwrap();
        assert_eq!(observed[0].0, *first.origin_session_id().unwrap());
        assert_eq!(observed[1].0, *second.origin_session_id().unwrap());
        assert_eq!(observed[0].1, observed[1].1);
        assert_ne!(observed[0].0, observed[1].0);
        assert!(
            observed
                .iter()
                .all(|(_, _, origin)| *origin == json!({"kind":"unavailable"}))
        );
        let requests = fixture.server.requests.lock().unwrap();
        for request in requests.iter() {
            assert_eq!(request["name"], "read");
            assert_eq!(
                request["meta"][meerkat_core::CALL_ORIGIN_META_KEY],
                json!({"kind":"unavailable"})
            );
            let wire = request.to_string();
            assert!(!wire.contains(&first.origin_session_id().unwrap().to_string()));
            assert!(!wire.contains(&second.origin_session_id().unwrap().to_string()));
        }
        assert_ne!(
            requests[0]["meta"][PRIVATE_KEY],
            requests[1]["meta"][PRIVATE_KEY]
        );
        assert_eq!(provider.dropped.load(Ordering::SeqCst), 2);
    })
    .catch_unwind()
    .await;
    adapter.shutdown().await;
    fixture.finish(result).await;
}

#[tokio::test]
async fn missing_context_and_reserved_override_refuse_before_server_effects() {
    let fixture = Fixture::start().await;
    let mut source = Provider::new(fixture.config.clone());
    source.reserved = true;
    let provider = Arc::new(source);
    let router = fixture
        .router(Some(provider.clone()), fixture.config.clone())
        .await;
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({"session_id":"forged"})).unwrap();
        let call = ToolCallView {
            id: "refusal",
            name: "read",
            args: &args,
        };
        let claimed =
            ToolDispatchContext::default().with_turn_metadata(std::collections::BTreeMap::from([
                ("origin_session_id".into(), json!(SessionId::new())),
                ("io.meerkat/origin".into(), json!({"kind":"admitted"})),
            ]));
        assert!(router.dispatch_with_context(call, &claimed).await.is_err());
        assert!(
            router
                .dispatch_with_context(call, &context())
                .await
                .is_err()
        );
        assert_eq!(provider.prepared.load(Ordering::SeqCst), 1);
        assert_eq!(provider.dropped.load(Ordering::SeqCst), 1);
        assert!(fixture.server.requests.lock().unwrap().is_empty());
        assert!(
            router
                .external_tool_surface_snapshot()
                .entries
                .iter()
                .all(|entry| entry.inflight_call_count == 0)
        );
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}

#[tokio::test]
async fn no_provider_and_unselected_exact_destination_preserve_call_payloads() {
    let fixture = Fixture::start().await;
    let mut changed = fixture.config.clone();
    if let McpTransportConfig::Http(http) = &mut changed.transport {
        http.headers.insert("X-Destination".into(), "other".into());
    }
    let provider = Arc::new(Provider::new(changed));
    let plain = fixture.router(None, fixture.config.clone()).await;
    let unselected = fixture
        .router(Some(provider.clone()), fixture.config.clone())
        .await;
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({"action":"ok"})).unwrap();
        let call = ToolCallView {
            id: "ordinary",
            name: "read",
            args: &args,
        };
        plain.dispatch_with_context(call, &context()).await.unwrap();
        unselected
            .dispatch_with_context(call, &context())
            .await
            .unwrap();
        let requests = fixture.server.requests.lock().unwrap();
        assert_eq!(requests[0], requests[1]);
        assert!(
            requests[0]["meta"]
                .get(meerkat_core::CALL_ORIGIN_META_KEY)
                .is_none()
        );
        assert_eq!(provider.prepared.load(Ordering::SeqCst), 0);
        assert_eq!(
            unselected.tool_mutation_class("read"),
            ToolMutationClass::Unknown
        );
    })
    .catch_unwind()
    .await;
    plain.shutdown().await;
    unselected.shutdown().await;
    fixture.finish(result).await;
}

#[tokio::test]
async fn cancellation_drops_lease_finishes_native_call_and_wakes_progress() {
    let fixture = Fixture::start().await;
    let provider = Arc::new(Provider::new(fixture.config.clone()));
    let mut router = fixture
        .router(Some(provider.clone()), fixture.config.clone())
        .await;
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({"action":"block"})).unwrap();
        let current = context();
        let call = router.dispatch_with_context(
            ToolCallView {
                id: "cancelled",
                name: "read",
                args: &args,
            },
            &current,
        );
        let mut call = Box::pin(call);
        tokio::select! {
            _ = &mut call => panic!("blocked call returned before cancellation"),
            permit = fixture.server.entered.acquire() => permit.unwrap().forget(),
            () = tokio::time::sleep(LIMIT) => panic!("server never entered"),
        }
        assert_eq!(
            router.external_tool_surface_snapshot().entries[0].inflight_call_count,
            1
        );
        let mut progress = router.progress.subscribe();
        drop(call);
        tokio::time::timeout(LIMIT, progress.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(provider.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(
            router.external_tool_surface_snapshot().entries[0].inflight_call_count,
            0
        );
        assert_eq!(
            router.servers["context-fixture"]
                .active_calls
                .load(Ordering::SeqCst),
            0
        );
        fixture.server.release.add_permits(1);
        router.stage_remove("context-fixture").unwrap();
        router.apply_staged().await.unwrap();
        router.progress_removals().await.unwrap();
        assert!(router.list_tools().is_empty());
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}

#[tokio::test]
async fn stale_resolved_owner_refuses_before_preparation_after_reload() {
    let fixture = Fixture::start().await;
    let provider = Arc::new(Provider::new(fixture.config.clone()));
    let router = fixture
        .router(Some(provider.clone()), fixture.config.clone())
        .await;
    let adapter = McpRouterAdapter::new(router);
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({})).unwrap();
        let current = context();
        let call = ToolCallView {
            id: "resolved",
            name: "read",
            args: &args,
        };
        let plan = adapter
            .resolve_execution_plan(call, &current, &resolution())
            .unwrap();
        adapter
            .dispatch_resolved_with_context(call, &current, &plan)
            .await
            .unwrap();
        let first_connection = provider.observed.lock().unwrap()[0].1;
        adapter.stage_reload("context-fixture").await.unwrap();
        adapter.apply_staged().await.unwrap();
        adapter.wait_until_ready(LIMIT).await.unwrap();
        adapter.refresh_tools().await.unwrap();
        assert!(
            adapter
                .dispatch_resolved_with_context(call, &current, &plan)
                .await
                .is_err()
        );
        assert_eq!(provider.prepared.load(Ordering::SeqCst), 1);
        let fresh = adapter
            .resolve_execution_plan(call, &current, &resolution())
            .unwrap();
        adapter
            .dispatch_resolved_with_context(call, &current, &fresh)
            .await
            .unwrap();
        assert_ne!(first_connection, provider.observed.lock().unwrap()[1].1);
        assert_eq!(provider.dropped.load(Ordering::SeqCst), 2);
    })
    .catch_unwind()
    .await;
    adapter.shutdown().await;
    fixture.finish(result).await;
}

#[path = "call_context_policy_tests.rs"]
mod policy_tests;
