//! A trusted host's checked route through the actual router, adapter, fenced
//! root dispatch and an rmcp HTTP server that records every call it receives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use futures::FutureExt;
use meerkat_core::approval::review::OperationReviewTier;
use meerkat_core::authorization::{
    OperationAuthorizationError, PreparedAuthorizationBinding, PreparedOperationAuthorization,
    ReviewOperationContextResolution, WorkAuthorization, WorkAuthorizationContext,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::{
    McpServerConfig, ToolCallView, ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
    ToolExecutionResolutionContext, ToolExecutionResolutionError, ToolOperationTarget,
    ToolUnavailableReason,
};
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
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(10);
const SERVER: &str = "checked-fixture";

/// Lists an ordinary operation the ceiling admits and one it excludes,
/// records the name of every call that reaches it, and holds a call for the
/// package `block` until released.
#[derive(Clone)]
struct Server {
    calls: Arc<std::sync::Mutex<Vec<String>>>,
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
        for (name, description) in [
            ("read", "an ordinary operation"),
            ("install", "an operation outside the ceiling"),
        ] {
            result.tools.push(Tool::new(
                name,
                description,
                json!({"type": "object", "properties": {"package": {"type": "string"}}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ));
        }
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.calls.lock().unwrap().push(request.name.to_string());
        let blocked = request
            .arguments
            .as_ref()
            .and_then(|args| args.get("package"))
            .and_then(serde_json::Value::as_str)
            == Some("block");
        if blocked {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        Ok(CallToolResult::success(vec![Content::text(format!(
            "{} done",
            request.name
        ))]))
    }
}

struct Fixture {
    server: Server,
    config: McpServerConfig,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

#[cfg(feature = "workspace-tests")]
impl Fixture {
    async fn start() -> Self {
        let server = Server {
            calls: Default::default(),
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
        let mut config = McpServerConfig::streamable_http(
            SERVER,
            format!("http://{address}/mcp"),
            Default::default(),
        );
        config.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
        Self {
            server,
            config,
            stop,
            task,
        }
    }

    /// One session's own adapter over its own connection to the server,
    /// optionally preparing its calls through `provider`.
    async fn adapter(
        &self,
        provider: Option<Arc<dyn crate::McpCallContextProvider>>,
    ) -> Arc<McpRouterAdapter> {
        let mut router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            crate::McpStdioLaunchProfile::trusted_host(),
        );
        if let Some(provider) = provider {
            router = router.with_call_context_provider(provider);
        }
        tokio::time::timeout(LIMIT, router.add_server(self.config.clone()))
            .await
            .unwrap()
            .unwrap();
        Arc::new(McpRouterAdapter::new(router))
    }

    fn calls(&self) -> Vec<String> {
        self.server.calls.lock().unwrap().clone()
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

/// Bound work that allows every operation at R1: the route's own conjuncts,
/// not this owner, are under test here.
struct AllowWork;

struct Allowed;

impl WorkAuthorization for AllowWork {
    fn review_operation_context(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
        Ok(ReviewOperationContextResolution::NotApplicable)
    }

    fn prepare(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Ok(Arc::new(Allowed))
    }
}

impl PreparedOperationAuthorization for Allowed {
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R1
    }

    fn check_current(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }
}

/// `session`'s dispatch context under actual bound work.
fn governed(session: &SessionId) -> ToolDispatchContext {
    ToolDispatchContext::default()
        .with_runtime_identity(session.clone(), None)
        .with_work_authorization(Some(WorkAuthorizationContext::new(
            Arc::new(AllowWork),
            OperationExecutionScope::Domain,
        )))
}

/// Holds the call-context preparation of every call `call-paused`, between
/// the route's entry checks and the transport handoff, until released.
struct PausingProvider {
    entered: Semaphore,
    release: Semaphore,
}

impl Default for PausingProvider {
    fn default() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}

#[async_trait]
impl crate::McpCallContextProvider for PausingProvider {
    async fn prepare(
        &self,
        _: crate::McpCallTarget<'_>,
        call: ToolCallView<'_>,
        _: &ToolDispatchContext,
    ) -> Result<Option<crate::McpCallContext>, crate::McpCallContextError> {
        if call.id == "call-paused" {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
        Ok(None)
    }

    fn tool_mutation_class(&self, _: &McpServerConfig, _: &str) -> meerkat_core::ToolMutationClass {
        meerkat_core::ToolMutationClass::Unknown
    }
}

fn spec() -> CheckedMcpRouteSpec {
    CheckedMcpRouteSpec {
        route: "checked".into(),
        server: SERVER.into(),
        operations: ["install".to_string()].into(),
    }
}

/// The session's root: its adapter and its checked route, composed.
fn root(
    adapter: &Arc<McpRouterAdapter>,
    route: &Arc<CheckedMcpRoute>,
) -> Arc<dyn AgentToolDispatcher> {
    Arc::new(meerkat_core::DynamicToolComposite::new(vec![
        Arc::clone(adapter) as Arc<dyn AgentToolDispatcher>,
        Arc::clone(route) as Arc<dyn AgentToolDispatcher>,
    ]))
}

/// A host's checked wrapper as a consumer writes it: it spends its own
/// one-shot occurrence before the first poll of the route's dispatch, with
/// no await in between, and refuses nothing on its own. Whatever the route
/// refuses behind it, the route refuses natively, reading no host state.
struct ConsumingHost {
    route: Arc<CheckedMcpRoute>,
    consumed: std::sync::atomic::AtomicUsize,
}

impl ConsumingHost {
    fn consumed(&self) -> usize {
        self.consumed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AgentToolDispatcher for ConsumingHost {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.route.tools()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        self.route.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.route.tool_catalog()
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        self.route.tool_mutation_class(tool_name)
    }

    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        self.route.review_entry_support(tool_name)
    }

    fn execution_binding_epoch(&self, tool_name: &str) -> u64 {
        self.route.execution_binding_epoch(tool_name)
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<meerkat_core::EphemeralToolBindingFingerprint, ToolExecutionResolutionError> {
        self.route.execution_binding_fingerprint(tool_name)
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, ToolExecutionResolutionError> {
        self.route
            .resolve_execution_plan(call, dispatch_context, resolution_context)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        self.route.dispatch(call).await
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let operation = self
            .route
            .dispatch_resolved_with_context(call, context, plan);
        self.consumed.fetch_add(1, Ordering::SeqCst);
        operation.await
    }
}

fn resolution() -> ToolExecutionResolutionContext {
    ToolExecutionResolutionContext::new(
        ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_mins(10),
        )])
        .unwrap(),
    )
}

fn resolve(
    root: &Arc<dyn AgentToolDispatcher>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
) -> Result<meerkat_core::ResolvedToolExecutionPlan, ToolExecutionResolutionError> {
    meerkat_core::agent::resolve_tool_execution_plan_fenced(root, call, context, &resolution())
}

async fn dispatch(
    root: &Arc<dyn AgentToolDispatcher>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
    plan: &meerkat_core::ResolvedToolExecutionPlan,
) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
    meerkat_core::agent::dispatch_tool_execution_plan_fenced(root, call, context, plan).await
}

fn names(tools: &[Arc<ToolDef>]) -> Vec<String> {
    tools.iter().map(|tool| tool.name.to_string()).collect()
}

fn owner_changed(name: &str) -> ToolError {
    ToolError::unavailable(name, ToolUnavailableReason::ExecutionOwnerChanged)
}

#[test]
fn an_incomplete_route_is_refused() {
    for spec in [
        CheckedMcpRouteSpec {
            route: String::new(),
            ..spec()
        },
        CheckedMcpRouteSpec {
            server: String::new(),
            ..spec()
        },
        CheckedMcpRouteSpec {
            operations: Default::default(),
            ..spec()
        },
        CheckedMcpRouteSpec {
            operations: [String::new()].into(),
            ..spec()
        },
    ] {
        assert!(matches!(
            CheckedMcpRoute::new(
                Arc::new(McpRouterAdapter::new(McpRouter::new_with_stdio_profile(
                    crate::McpStdioLaunchProfile::trusted_host()
                ))),
                SessionId::new(),
                spec,
            ),
            Err(CheckedMcpRouteError::Incomplete)
        ));
    }
}

/// The projection keeps the ceiling while the route offers exactly its
/// selected operation with the server's own definition. The route enters
/// only under actual bound work for its own session, with a plan its own
/// instance resolved on the live connection, once per plan. Every refusal
/// is before anything reaches the server.
#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn a_checked_route_enters_once_under_its_own_sessions_bound_work() {
    let fixture = Fixture::start().await;
    let result = AssertUnwindSafe(async {
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let adapter_a = fixture.adapter(None).await;
        let adapter_b = fixture.adapter(None).await;
        let route_a = adapter_a.checked_route(session_a.clone(), spec()).unwrap();
        let route_b = adapter_b.checked_route(session_b.clone(), spec()).unwrap();
        let root_a = root(&adapter_a, &route_a);
        let root_b = root(&adapter_b, &route_b);
        let context_a = governed(&session_a);
        let context_b = governed(&session_b);
        let args =
            serde_json::value::RawValue::from_string(r#"{"package":"demo"}"#.into()).unwrap();
        let install = ToolCallView {
            id: "call-install",
            name: "install",
            args: &args,
        };
        let read = ToolCallView {
            id: "call-read",
            name: "read",
            args: &args,
        };

        // Discovery: the projection never offers the excluded operation;
        // the route offers exactly it, under the server's own definition.
        assert_eq!(names(&adapter_a.tools()), vec!["read"]);
        assert_eq!(names(&route_a.tools()), vec!["install"]);
        let offered = route_a.tool_catalog()[0].clone();
        assert_eq!(offered.tool.description, "an operation outside the ceiling");
        assert_eq!(
            offered
                .tool
                .provenance
                .as_ref()
                .map(|source| source.source_id.as_str()),
            Some(SERVER)
        );

        // The raw route stays refused before any request: the projection
        // does not route it, and the route runs no unresolved call.
        let raw: Arc<dyn AgentToolDispatcher> = adapter_a.clone();
        assert!(matches!(
            resolve(&raw, install, &context_a),
            Err(ToolExecutionResolutionError::NotFound { .. })
        ));
        assert!(matches!(
            adapter_a.dispatch_with_context(install, &context_a).await,
            Err(ToolError::NotFound { .. })
        ));
        assert!(
            route_a
                .dispatch_with_context(install, &context_a)
                .await
                .is_err()
        );

        // A matching runtime identity on an ungoverned (default) context is
        // not bound work: refused before entry.
        let ungoverned =
            ToolDispatchContext::default().with_runtime_identity(session_a.clone(), None);
        let plan = resolve(&root_a, install, &ungoverned).expect("ungoverned plan");
        assert_eq!(
            dispatch(&root_a, install, &ungoverned, &plan)
                .await
                .unwrap_err(),
            ToolError::access_denied("install")
        );
        assert!(fixture.calls().is_empty());

        // Under its session's bound work the route resolves exactly its
        // checked target and enters once.
        let plan = resolve(&root_a, install, &context_a).expect("checked plan");
        let targets: Vec<_> = plan
            .owner_witnesses()
            .iter()
            .filter_map(|witness| witness.operation_target())
            .collect();
        assert!(
            matches!(
                targets.as_slice(),
                [ToolOperationTarget::CheckedMcpServerOperation {
                    route,
                    server,
                    raw_operation,
                    ..
                }] if route == "checked" && server == SERVER && raw_operation == "install"
            ),
            "{targets:?}"
        );
        dispatch(&root_a, install, &context_a, &plan)
            .await
            .expect("the checked route enters");
        assert_eq!(fixture.calls(), vec!["install"]);

        // The identical replay is refused natively.
        assert_eq!(
            dispatch(&root_a, install, &context_a, &plan)
                .await
                .unwrap_err(),
            owner_changed("install")
        );
        assert_eq!(fixture.calls(), vec!["install"]);

        // Another session never runs this plan: not through its own root,
        // and not through this root under its own work and identity.
        let second = ToolCallView {
            id: "call-install-2",
            name: "install",
            args: &args,
        };
        let plan_two = resolve(&root_a, second, &context_a).expect("second plan");
        assert!(
            dispatch(&root_b, second, &context_b, &plan_two)
                .await
                .is_err()
        );
        assert_eq!(
            dispatch(&root_a, second, &context_b, &plan_two)
                .await
                .unwrap_err(),
            ToolError::access_denied("install")
        );
        // Another registration on the same adapter never runs it either.
        let other = adapter_a.checked_route(session_a.clone(), spec()).unwrap();
        assert_eq!(
            other
                .dispatch_resolved_with_context(second, &context_a, &plan_two)
                .await
                .unwrap_err(),
            ToolError::access_denied("install"),
            "a plan outside its dispatch carries no prepared authorization"
        );
        assert_eq!(fixture.calls(), vec!["install"]);

        // Session B's own route serves session B.
        let plan_b = resolve(&root_b, install, &context_b).expect("B's plan");
        dispatch(&root_b, install, &context_b, &plan_b)
            .await
            .expect("B's own route enters");
        // An admitted sibling still works.
        let sibling = resolve(&root_a, read, &context_a).expect("sibling plan");
        dispatch(&root_a, read, &context_a, &sibling)
            .await
            .expect("the sibling enters");
        assert_eq!(fixture.calls(), vec!["install", "install", "read"]);

        // A plan names the physical connection it resolved on: once a
        // reload replaces that connection the plan never enters, and a fresh
        // resolution enters on the new one.
        let reloaded = ToolCallView {
            id: "call-install-reloaded",
            name: "install",
            args: &args,
        };
        let stale = resolve(&root_a, reloaded, &context_a).expect("plan on the first connection");
        adapter_a.stage_reload(SERVER).await.unwrap();
        adapter_a.apply_staged().await.unwrap();
        adapter_a.wait_until_ready(LIMIT).await.unwrap();
        adapter_a.refresh_tools().await.unwrap();
        assert_eq!(
            dispatch(&root_a, reloaded, &context_a, &stale)
                .await
                .unwrap_err(),
            owner_changed("install")
        );
        assert_eq!(fixture.calls(), vec!["install", "install", "read"]);
        let fresh = resolve(&root_a, reloaded, &context_a).expect("plan on the new connection");
        dispatch(&root_a, reloaded, &context_a, &fresh)
            .await
            .expect("the fresh plan enters on the new connection");
        assert_eq!(
            fixture.calls(),
            vec!["install", "install", "read", "install"]
        );

        // The adapter's shutdown ends B's route as well.
        adapter_b.shutdown().await;
        assert!(route_b.tools().is_empty());
        let after = ToolCallView {
            id: "call-after-shutdown",
            name: "install",
            args: &args,
        };
        assert!(matches!(
            resolve(&root_b, after, &context_b),
            Err(ToolExecutionResolutionError::NotFound { .. })
        ));
        assert_eq!(
            fixture.calls(),
            vec!["install", "install", "read", "install"]
        );
        adapter_a.shutdown().await;
    })
    .catch_unwind()
    .await;
    fixture.finish(result).await;
}

/// Closing the route refuses work that has not reached its transport
/// handoff, even after its own entry checks passed, and never cancels an
/// effect that already entered.
#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn closing_a_route_refuses_unentered_work_and_keeps_entered_custody() {
    let fixture = Fixture::start().await;
    let result = AssertUnwindSafe(async {
        let session = SessionId::new();
        let provider = Arc::new(PausingProvider::default());
        let adapter = fixture
            .adapter(Some(
                Arc::clone(&provider) as Arc<dyn crate::McpCallContextProvider>
            ))
            .await;
        let context = governed(&session);
        let blocking =
            serde_json::value::RawValue::from_string(r#"{"package":"block"}"#.into()).unwrap();
        let demo =
            serde_json::value::RawValue::from_string(r#"{"package":"demo"}"#.into()).unwrap();

        // Entered: the server holds the call; closing the route then keeps
        // its custody, and the call completes.
        let route = adapter.checked_route(session.clone(), spec()).unwrap();
        let root_one = root(&adapter, &route);
        let held = ToolCallView {
            id: "call-held",
            name: "install",
            args: &blocking,
        };
        let plan = resolve(&root_one, held, &context).expect("held plan");
        let mut pending = Box::pin(dispatch(&root_one, held, &context, &plan));
        tokio::select! {
            _ = &mut pending => panic!("the call finished before the server held it"),
            permit = fixture.server.entered.acquire() => permit.unwrap().forget(),
        }
        route.close();
        fixture.server.release.add_permits(1);
        let outcome = tokio::time::timeout(LIMIT, pending)
            .await
            .unwrap()
            .expect("an entered effect keeps its custody");
        assert_eq!(outcome.result.text_content(), "install done");
        assert_eq!(fixture.calls(), vec!["install"]);

        // Not yet entered: the call passed the route's entry checks and
        // waits in preparation; closing the route refuses it at the
        // handoff, before anything is sent.
        let route = adapter.checked_route(session.clone(), spec()).unwrap();
        let root_two = root(&adapter, &route);
        let paused = ToolCallView {
            id: "call-paused",
            name: "install",
            args: &demo,
        };
        let plan = resolve(&root_two, paused, &context).expect("paused plan");
        let mut pending = Box::pin(dispatch(&root_two, paused, &context, &plan));
        tokio::select! {
            _ = &mut pending => panic!("dispatch skipped call-context preparation"),
            permit = provider.entered.acquire() => permit.unwrap().forget(),
        }
        route.close();
        provider.release.add_permits(1);
        let refused = tokio::time::timeout(LIMIT, pending)
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(refused, owner_changed("install"));
        assert_eq!(fixture.calls(), vec!["install"]);
        adapter.shutdown().await;
    })
    .catch_unwind()
    .await;
    fixture.finish(result).await;
}

/// A host's checked wrapper spends its own one-shot occurrence before the
/// first poll of the route's dispatch. The route reads no host state: that
/// entry runs, and the identical replay never reaches the server, through a
/// host that refuses nothing. A provider can repeat a call id in a later
/// turn: its fresh resolution is a new occurrence, entering once under its
/// own prepared authorization. Of two dispatches of one plan, both held
/// after the route's entry checks, exactly one enters at the handoff.
#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn after_the_hosts_own_consume_each_plan_enters_once() {
    let fixture = Fixture::start().await;
    let result = AssertUnwindSafe(async {
        let session = SessionId::new();
        let provider = Arc::new(PausingProvider::default());
        let adapter = fixture
            .adapter(Some(
                Arc::clone(&provider) as Arc<dyn crate::McpCallContextProvider>
            ))
            .await;
        let route = adapter.checked_route(session.clone(), spec()).unwrap();
        let host = Arc::new(ConsumingHost {
            route,
            consumed: Default::default(),
        });
        let root: Arc<dyn AgentToolDispatcher> =
            Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                Arc::clone(&adapter) as Arc<dyn AgentToolDispatcher>,
                Arc::clone(&host) as Arc<dyn AgentToolDispatcher>,
            ]));
        let context = governed(&session);
        let args =
            serde_json::value::RawValue::from_string(r#"{"package":"demo"}"#.into()).unwrap();
        let install = ToolCallView {
            id: "fc_0",
            name: "install",
            args: &args,
        };

        // The entry after the host's own consume runs.
        let plan = resolve(&root, install, &context).expect("checked plan");
        dispatch(&root, install, &context, &plan)
            .await
            .expect("the entry after the host's consume runs");
        assert_eq!(host.consumed(), 1);
        assert_eq!(fixture.calls(), vec!["install"]);

        // The identical replay passes the host and is refused natively.
        assert_eq!(
            dispatch(&root, install, &context, &plan).await.unwrap_err(),
            owner_changed("install")
        );
        assert_eq!(host.consumed(), 2);
        assert_eq!(fixture.calls(), vec!["install"]);

        // The same call id in a later turn is resolved afresh: a new
        // occurrence, entering once.
        let later = resolve(&root, install, &context).expect("a later turn's plan");
        dispatch(&root, install, &context, &later)
            .await
            .expect("a new occurrence enters");
        assert_eq!(
            dispatch(&root, install, &context, &later)
                .await
                .unwrap_err(),
            owner_changed("install")
        );
        assert_eq!(fixture.calls(), vec!["install", "install"]);

        // Two dispatches of one plan, both past the route's entry checks
        // and held in preparation: exactly one enters at the handoff.
        let paused = ToolCallView {
            id: "call-paused",
            name: "install",
            args: &args,
        };
        let plan = resolve(&root, paused, &context).expect("held plan");
        let (first, second, ()) = tokio::time::timeout(LIMIT, async {
            tokio::join!(
                dispatch(&root, paused, &context, &plan),
                dispatch(&root, paused, &context, &plan),
                async {
                    provider.entered.acquire_many(2).await.unwrap().forget();
                    provider.release.add_permits(2);
                },
            )
        })
        .await
        .unwrap();
        let refused: Vec<_> = [first, second]
            .into_iter()
            .filter_map(Result::err)
            .collect();
        assert_eq!(refused, vec![owner_changed("install")]);
        assert_eq!(host.consumed(), 6);
        assert_eq!(fixture.calls(), vec!["install", "install", "install"]);
        adapter.shutdown().await;
    })
    .catch_unwind()
    .await;
    fixture.finish(result).await;
}
