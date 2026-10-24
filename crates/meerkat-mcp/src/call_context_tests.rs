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
    /// When set, each request records how many final-check custodies had
    /// been dropped when it arrived.
    custody_probe: Arc<std::sync::Mutex<Option<Arc<AtomicUsize>>>>,
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
        let custody_dropped = self
            .custody_probe
            .lock()
            .unwrap()
            .as_ref()
            .map(|probe| probe.load(Ordering::SeqCst));
        self.requests.lock().unwrap().push(json!({
            "name": request.name,
            "arguments": request.arguments,
            "meta": context.meta,
            "custody_dropped": custody_dropped,
        }));
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
            custody_probe: Default::default(),
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
        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            crate::McpStdioLaunchProfile::trusted_host(),
        );
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
    /// The Core-owned name a `reserved` preparation uses.
    reserved_key: &'static str,
    unselected_class: ToolMutationClass,
    restrictions: std::sync::Mutex<Vec<bool>>,
    /// The final check this provider attaches: `Some(passes)`.
    final_check: Option<bool>,
    final_checked: Arc<AtomicUsize>,
    final_custody_dropped: Arc<AtomicUsize>,
}

impl Provider {
    fn new(selected: McpServerConfig) -> Self {
        Self {
            selected,
            observed: Default::default(),
            prepared: AtomicUsize::new(0),
            dropped: Default::default(),
            reserved: false,
            reserved_key: meerkat_core::CALL_ORIGIN_META_KEY,
            unselected_class: ToolMutationClass::Unknown,
            restrictions: Default::default(),
            final_check: None,
            final_checked: Default::default(),
            final_custody_dropped: Default::default(),
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
            self.reserved_key
        } else {
            PRIVATE_KEY
        };
        let prepared = McpCallContext::new(
            serde_json::Map::from_iter([(
                key.into(),
                json!({"correlation":format!("private-{sequence}")}),
            )]),
            Lease(Arc::clone(&self.dropped)),
        );
        Ok(Some(match self.final_check {
            None => prepared,
            Some(passes) => {
                let checked = Arc::clone(&self.final_checked);
                let custody = Arc::clone(&self.final_custody_dropped);
                prepared.with_final_check(move || {
                    checked.fetch_add(1, Ordering::SeqCst);
                    if passes {
                        Ok(Lease(custody))
                    } else {
                        Err(McpCallContextError::Denied)
                    }
                })
            }
        }))
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

/// The dedupe key of a call to the fixture's `read` operation.
fn read_dedupe_key(args: &serde_json::value::RawValue) -> meerkat_core::ToolCallDedupeKey {
    meerkat_core::ToolCallDedupeKey::new(meerkat_core::RunId::new(), 4, "keyed-call", "read", args)
        .unwrap()
}

/// A call that has a dedupe key sends it, unchanged, as Core's own `_meta`
/// entry, beside the host's prepared metadata, which reaches the server
/// unchanged too. (Unrun until the candidate's validation.)
#[tokio::test]
async fn a_keyed_call_sends_its_dedupe_key_beside_the_host_metadata() {
    let fixture = Fixture::start().await;
    let result = AssertUnwindSafe(async {
        let connection = crate::McpConnection::connect_within(&fixture.config, LIMIT)
            .await
            .unwrap();
        let args = json!({"action": "ok"});
        let key = read_dedupe_key(&serde_json::value::to_raw_value(&args).unwrap());
        let host = serde_json::Map::from_iter([(
            PRIVATE_KEY.to_string(),
            json!({"correlation": "host-prepared"}),
        )]);
        connection
            .call_tool_result_admitted(
                crate::connection::CallAdmission::Ceiling(None),
                "read",
                &args,
                Some(host),
                Some(&key),
                || Ok(None),
            )
            .await
            .unwrap();
        {
            let requests = fixture.server.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let meta = &requests[0]["meta"];
            assert_eq!(
                meta[meerkat_core::TOOL_CALL_DEDUPE_KEY_META_KEY],
                serde_json::to_value(&key).unwrap()
            );
            assert_eq!(
                meta[meerkat_core::TOOL_CALL_DEDUPE_KEY_META_KEY]["call_id"],
                "keyed-call"
            );
            assert_eq!(
                meta[meerkat_core::TOOL_CALL_DEDUPE_KEY_META_KEY]["round"],
                4
            );
            assert_eq!(meta[PRIVATE_KEY], json!({"correlation": "host-prepared"}));
        }
        connection.close().await.unwrap();
    })
    .catch_unwind()
    .await;
    fixture.finish(result).await;
}

/// A call outside an agent-loop round (its context binds no run and no
/// round) has no dedupe key, and its `_meta` names none; the host's
/// prepared metadata still reaches the server. (Unrun until the
/// candidate's validation.)
#[tokio::test]
async fn a_call_outside_a_round_sends_no_dedupe_key() {
    let fixture = Fixture::start().await;
    let provider = Arc::new(Provider::new(fixture.config.clone()));
    let router = fixture
        .router(Some(provider.clone()), fixture.config.clone())
        .await;
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({"action":"ok"})).unwrap();
        let call = ToolCallView {
            id: "unkeyed",
            name: "read",
            args: &args,
        };
        let current = context();
        assert!(current.tool_call_dedupe_key(call).is_none());
        router.dispatch_with_context(call, &current).await.unwrap();
        let requests = fixture.server.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]["meta"]
                .get(meerkat_core::TOOL_CALL_DEDUPE_KEY_META_KEY)
                .is_none()
        );
        assert_eq!(requests[0]["meta"][PRIVATE_KEY]["correlation"], "private-0");
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}

/// Core owns the dedupe key's `_meta` name: a host preparation that uses it
/// is refused before the server sees anything, as one that uses the origin
/// key is, and its lease is dropped. (Unrun until the candidate's
/// validation.)
#[tokio::test]
async fn a_host_preparation_that_names_the_dedupe_key_is_refused() {
    let fixture = Fixture::start().await;
    let mut source = Provider::new(fixture.config.clone());
    source.reserved = true;
    source.reserved_key = meerkat_core::TOOL_CALL_DEDUPE_KEY_META_KEY;
    let provider = Arc::new(source);
    let router = fixture
        .router(Some(provider.clone()), fixture.config.clone())
        .await;
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({"action":"ok"})).unwrap();
        let call = ToolCallView {
            id: "forged",
            name: "read",
            args: &args,
        };
        let error = router
            .dispatch_with_context(call, &context())
            .await
            .unwrap_err();
        assert!(
            matches!(error.primary_error(), ToolError::AccessDenied { .. }),
            "{error:?}"
        );
        assert_eq!(provider.prepared.load(Ordering::SeqCst), 1);
        assert_eq!(provider.dropped.load(Ordering::SeqCst), 1);
        assert!(fixture.server.requests.lock().unwrap().is_empty());
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
/// Cancelling an entered call finishes the native call and wakes progress,
/// but the preparation lease stays with the native send owner, which still
/// holds the request, until that request settles. (Fails-old: the lease
/// dropped with the caller while the request could still be in flight.)
async fn cancellation_finishes_native_call_and_the_send_owner_keeps_the_lease() {
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
        let held_after_cancel = provider.dropped.load(Ordering::SeqCst) == 0;
        let inflight = router.external_tool_surface_snapshot().entries[0].inflight_call_count;
        let active = router.servers["context-fixture"]
            .active_calls
            .load(Ordering::SeqCst);
        // Release the server before asserting, so a failure cannot leave the
        // fixture blocked behind the in-flight request.
        fixture.server.release.add_permits(1);
        let released = tokio::time::timeout(LIMIT, async {
            while provider.dropped.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            held_after_cancel,
            "the send owner still holds the in-flight request's lease"
        );
        assert_eq!((inflight, active), (0, 0));
        assert!(
            released.is_ok(),
            "the lease is released once the request settles"
        );
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

#[path = "call_context_review_tests.rs"]
mod review_tests;

/// A preparation's final check runs once at the call's final native entry: a
/// refusal there sends nothing and drops the lease, and a pass sends the
/// prepared metadata.
#[tokio::test]
async fn a_final_check_refusal_sends_nothing_and_a_pass_sends_the_metadata() {
    let fixture = Fixture::start().await;
    let mut refusing = Provider::new(fixture.config.clone());
    refusing.final_check = Some(false);
    let refusing = Arc::new(refusing);
    let mut passing = Provider::new(fixture.config.clone());
    passing.final_check = Some(true);
    let passing = Arc::new(passing);
    let refused_router = fixture
        .router(Some(refusing.clone()), fixture.config.clone())
        .await;
    let passing_router = fixture
        .router(Some(passing.clone()), fixture.config.clone())
        .await;
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({"action":"ok"})).unwrap();
        let call = ToolCallView {
            id: "final-check",
            name: "read",
            args: &args,
        };
        let refused = refused_router.dispatch_with_context(call, &context()).await;
        assert!(refused.is_err(), "the final check refused the call");
        assert_eq!(refusing.final_checked.load(Ordering::SeqCst), 1);
        assert_eq!(refusing.dropped.load(Ordering::SeqCst), 1);
        assert!(
            fixture.server.requests.lock().unwrap().is_empty(),
            "a refused final check sent nothing"
        );

        *fixture.server.custody_probe.lock().unwrap() =
            Some(Arc::clone(&passing.final_custody_dropped));
        passing_router
            .dispatch_with_context(call, &context())
            .await
            .unwrap();
        assert_eq!(passing.final_checked.load(Ordering::SeqCst), 1);
        let requests = fixture.server.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]["meta"].get(PRIVATE_KEY).is_some());
        // What the passed check guards was still held when the request
        // reached the server: through the reviewed entry and the handoff.
        assert_eq!(requests[0]["custody_dropped"], json!(0));
    })
    .catch_unwind()
    .await;
    refused_router.shutdown().await;
    passing_router.shutdown().await;
    fixture.finish(result).await;
}

/// A connector-bound server's call with no connector preparation (here no
/// provider at all) is refused locally and never reaches the server unbound.
#[tokio::test]
async fn a_bound_server_without_its_preparation_is_refused_locally() {
    let fixture = Fixture::start().await;
    let mut config = fixture.config.clone();
    config.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
        realm_id: "tenant-a".into(),
        slot_id: "mail-work".into(),
        issuer: "https://issuer.example".into(),
        client: "client".into(),
        resource: "https://mail.example".into(),
        strategy_id: "oidc-userinfo-v1".into(),
        account: "subject-a".into(),
        operations: std::collections::BTreeMap::from([(
            "read".to_owned(),
            meerkat_core::mcp_config::McpConnectorOperation {
                required_scopes: Default::default(),
                destination_origin: "https://mail.example".into(),
                read_only: false,
            },
        )]),
    });
    let router = fixture.router(None, config).await;
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({"action":"ok"})).unwrap();
        let call = ToolCallView {
            id: "unprepared",
            name: "read",
            args: &args,
        };
        let refused = router.dispatch_with_context(call, &context()).await;
        assert!(refused.is_err(), "an unprepared bound call was refused");
        assert!(
            fixture.server.requests.lock().unwrap().is_empty(),
            "nothing reached the server"
        );
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}
