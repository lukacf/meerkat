//! MCP Apps wire contract through an actual rmcp HTTP server and native router.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::apps::{MCP_APP_RESOURCE_MIME_TYPE, MCP_APPS_EXTENSION, McpAppInvocation};
use futures::FutureExt;
use meerkat_core::tool_application::ToolApplicationResolution;
use meerkat_core::{
    SessionId, ToolApplicationOperation, ToolApplicationRequest, ToolDispatchContext,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, InitializeRequestParams, InitializeResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::json;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::AtomicBool;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(10);
const APP_URI: &str = "ui://fixture/view.html";
const OTHER_URI: &str = "ui://fixture/other.html";

fn wire_result(name: &str) -> Value {
    json!({
        "content": [{"type":"text","text":format!("Visible {name}")}],
        "structuredContent": {"version":1,"rows":[{"id":"row-a","value":7}]},
        "_meta": {"io.example/private":{"token":"host-only-secret"}},
        "isError": name == "refresh"
    })
}

fn wire_resource(uri: &str) -> Value {
    json!({"contents":[{
        "uri":uri,"mimeType":MCP_APP_RESOURCE_MIME_TYPE,
        "text":"<!doctype html><title>Native fixture</title><p>Fixture view</p>",
        "_meta":{"ui":{"csp":{"connectDomains":["wss://stream.example.test"]}}}
    }]})
}

#[derive(Clone)]
struct Server {
    initializations: Arc<Mutex<Vec<Value>>>,
    calls: Arc<Mutex<Vec<Value>>>,
    reads: Arc<Mutex<Vec<String>>>,
    resource_lists: Arc<AtomicUsize>,
    block_reads: Arc<AtomicBool>,
    read_entered: Arc<Semaphore>,
    read_release: Arc<Semaphore>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            initializations: Default::default(),
            calls: Default::default(),
            reads: Default::default(),
            resource_lists: Default::default(),
            block_reads: Default::default(),
            read_entered: Arc::new(Semaphore::new(0)),
            read_release: Arc::new(Semaphore::new(0)),
        }
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.initializations
            .lock()
            .unwrap()
            .push(serde_json::to_value(&request).unwrap());
        context.peer.set_peer_info(request);
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // The launch tool is model-only: owning a view does not make a tool
        // callable from that view. The action tool is deliberately app-only.
        Ok(serde_json::from_value(json!({"tools":[
            {"name":"display","description":"Open the view","inputSchema":{"type":"object"},
             "_meta":{"ui":{"resourceUri":APP_URI,"visibility":["model"]}}},
            {"name":"refresh","description":"Refresh from the app","inputSchema":{"type":"object"},
             "_meta":{"ui":{"visibility":["app"]}}},
            {"name":"default_audience","description":"Both audiences by default","inputSchema":{"type":"object"}},
            {"name":"hidden","description":"Hidden from both audiences","inputSchema":{"type":"object"},
             "_meta":{"ui":{"visibility":[]}}}
        ]})).unwrap())
    }

    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.resource_lists.fetch_add(1, Ordering::SeqCst);
        Ok(ListResourcesResult::default())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.reads.lock().unwrap().push(request.uri.clone());
        if request.uri != APP_URI && request.uri != OTHER_URI {
            return Err(ErrorData::invalid_params("unknown fixture resource", None));
        }
        if self.block_reads.load(Ordering::SeqCst) {
            self.read_entered.add_permits(1);
            self.read_release.acquire().await.unwrap().forget();
        }
        Ok(serde_json::from_value(wire_resource(&request.uri)).unwrap())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.calls
            .lock()
            .unwrap()
            .push(json!({"name":request.name,"arguments":request.arguments}));
        Ok(serde_json::from_value(wire_result(&request.name)).unwrap())
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
        let server = Server::default();
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
            "apps-fixture",
            format!("http://{address}/mcp"),
            Default::default(),
        );
        config
            .tool_names
            .insert("display".into(), "workspace_display".into());
        config
            .tool_names
            .insert("refresh".into(), "workspace_refresh".into());
        Self {
            server,
            config,
            stop,
            task,
        }
    }

    async fn router(&self, apps: bool) -> McpRouter {
        let mut router = McpRouter::new_with_surface_handle(Arc::new(
            meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral(),
        ));
        if apps {
            router = router
                .with_client_service_factory(Arc::new(crate::apps::McpAppsClientServiceFactory));
        }
        tokio::time::timeout(LIMIT, router.add_server(self.config.clone()))
            .await
            .unwrap()
            .unwrap();
        router
    }

    async fn finish(self, result: std::thread::Result<()>) {
        self.server.read_release.add_permits(8);
        self.stop.cancel();
        let joined = tokio::time::timeout(LIMIT, self.task).await;
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        joined.unwrap().unwrap().unwrap();
    }
}

struct Ingress(AtomicBool);

impl meerkat_core::ToolApplicationIngress for Ingress {
    fn revalidate(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.0
            .load(Ordering::SeqCst)
            .then_some(())
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

#[tokio::test]
async fn app_resource_result_is_refused_if_viewer_admission_retires_during_io() {
    let fixture = Fixture::start().await;
    let router = fixture.router(true).await;
    let result = AssertUnwindSafe(async {
        let original = dispatch(&router, "workspace_display", json!({})).await;
        let invocation = &original.host_metadata[MCP_APPS_EXTENSION];
        fixture.server.block_reads.store(true, Ordering::SeqCst);
        let request = ToolApplicationRequest {
            tool_call_id: "original-call".into(),
            extension: MCP_APPS_EXTENSION.into(),
            operation: ToolApplicationOperation::ReadResource {
                uri: OTHER_URI.into(),
            },
        };
        let ingress = Arc::new(Ingress(AtomicBool::new(true)));
        let session = SessionId::new();
        let control = meerkat_core::ToolApplicationControlRequest::from_trusted_ingress(
            session.clone(),
            request.clone(),
            ingress.clone(),
        )
        .unwrap();
        let context = ToolDispatchContext::default()
            .with_runtime_identity(session, None)
            .with_tool_application_control(control);
        let (read, ()) = tokio::join!(
            tokio::time::timeout(
                LIMIT,
                router.resolve_tool_application(
                    "workspace_display",
                    &request,
                    invocation,
                    &context,
                )
            ),
            async {
                tokio::time::timeout(LIMIT, fixture.server.read_entered.acquire())
                    .await
                    .unwrap()
                    .unwrap()
                    .forget();
                ingress.0.store(false, Ordering::SeqCst);
                fixture.server.read_release.add_permits(1);
            }
        );
        assert!(
            read.unwrap().is_err(),
            "private resource must not escape retired admission"
        );
        assert_eq!(*fixture.server.reads.lock().unwrap(), [APP_URI, OTHER_URI]);
        assert_eq!(
            router.external_tool_surface_snapshot().entries[0].inflight_call_count,
            0
        );
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}

fn context() -> ToolDispatchContext {
    ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None)
}

async fn dispatch(router: &McpRouter, name: &str, arguments: Value) -> ToolResult {
    let args = serde_json::value::to_raw_value(&arguments).unwrap();
    tokio::time::timeout(
        LIMIT,
        router.dispatch_with_context(
            ToolCallView {
                id: "original-call",
                name,
                args: &args,
            },
            &context(),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .result
}

async fn resolve(
    router: &McpRouter,
    invocation: &Value,
    operation: ToolApplicationOperation,
) -> Result<ToolApplicationResolution, ToolError> {
    tokio::time::timeout(
        LIMIT,
        router.resolve_tool_application(
            "workspace_display",
            &ToolApplicationRequest {
                tool_call_id: "original-call".into(),
                extension: MCP_APPS_EXTENSION.into(),
                operation,
            },
            invocation,
            &context(),
        ),
    )
    .await
    .unwrap()
}

fn resolved_value(resolution: ToolApplicationResolution) -> Value {
    match resolution {
        ToolApplicationResolution::Value(value) => value,
        _ => panic!("expected value"),
    }
}

fn refresh() -> ToolApplicationOperation {
    ToolApplicationOperation::CallTool {
        name: "refresh".into(),
        arguments: json!({"row":"row-a"}),
    }
}

#[tokio::test]
async fn apps_factory_advertises_standard_extension_while_headless_client_omits_it() {
    for apps in [false, true] {
        let fixture = Fixture::start().await;
        let router = fixture.router(apps).await;
        let result = AssertUnwindSafe(async {
            let observed = fixture.server.initializations.lock().unwrap();
            assert_eq!(observed.len(), 1);
            let extension = &observed[0]["capabilities"]["extensions"][MCP_APPS_EXTENSION];
            if apps {
                assert_eq!(
                    extension,
                    &json!({"mimeTypes":[MCP_APP_RESOURCE_MIME_TYPE]})
                );
            } else {
                assert!(extension.is_null());
            }
        })
        .catch_unwind()
        .await;
        router.shutdown().await;
        fixture.finish(result).await;
    }
}

#[tokio::test]
async fn ordinary_mcp_tools_do_not_retain_ui_carriers_or_private_metadata() {
    for apps in [false, true] {
        let fixture = Fixture::start().await;
        let router = fixture.router(apps).await;
        let result = AssertUnwindSafe(async {
            let result = dispatch(&router, "default_audience", json!({})).await;
            assert!(result.text_content().contains("Visible default_audience"));
            assert!(result.host_metadata.is_empty());
            assert!(
                !serde_json::to_string(&result)
                    .unwrap()
                    .contains("host-only-secret")
            );
            assert!(fixture.server.reads.lock().unwrap().is_empty());
            assert_eq!(fixture.server.calls.lock().unwrap().len(), 1);
        })
        .catch_unwind()
        .await;
        router.shutdown().await;
        fixture.finish(result).await;
    }
}

#[tokio::test]
async fn headless_tool_calls_do_not_prefetch_unnegotiated_app_resources() {
    let fixture = Fixture::start().await;
    let router = fixture.router(false).await;
    let result = AssertUnwindSafe(async {
        let result = dispatch(&router, "workspace_display", json!({})).await;
        assert!(result.text_content().contains("Visible display"));
        let invocation = &result.host_metadata[MCP_APPS_EXTENSION];
        assert_eq!(invocation["result"], wire_result("display"));
        assert!(invocation.get("resource").is_none());
        assert!(fixture.server.reads.lock().unwrap().is_empty());
        assert_eq!(fixture.server.calls.lock().unwrap().len(), 1);
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}

#[tokio::test]
async fn apps_discovery_retains_full_result_and_unlisted_declared_resource() {
    let fixture = Fixture::start().await;
    let router = fixture.router(true).await;
    let result = AssertUnwindSafe(async {
        let tools = router.list_tools();
        let display = tools
            .iter()
            .find(|tool| tool.name.as_str() == "workspace_display")
            .unwrap();
        let refresh = tools
            .iter()
            .find(|tool| tool.name.as_str() == "workspace_refresh")
            .unwrap();
        let default = tools
            .iter()
            .find(|tool| tool.name.as_str() == "default_audience")
            .unwrap();
        assert!(display.audience.allows_model());
        assert!(!display.audience.allows_app());
        assert!(!refresh.audience.allows_model());
        assert!(refresh.audience.allows_app());
        assert!(default.audience.allows_model() && default.audience.allows_app());
        let hidden = tools
            .iter()
            .find(|tool| tool.name.as_str() == "hidden")
            .unwrap();
        assert!(!hidden.audience.allows_model() && !hidden.audience.allows_app());
        assert!(
            !serde_json::to_string(display)
                .unwrap()
                .contains("resourceUri")
        );

        let arguments = json!({"filter":"selected","version":1});
        let result = dispatch(&router, "workspace_display", arguments.clone()).await;
        assert!(result.text_content().contains("Visible display"));
        assert!(!result.text_content().contains("host-only-secret"));
        assert!(
            !serde_json::to_string(&result.content)
                .unwrap()
                .contains("host-only-secret")
        );
        let invocation = result.host_metadata.get(MCP_APPS_EXTENSION).unwrap();
        assert_eq!(invocation["result"], wire_result("display"));
        assert_eq!(invocation["arguments"], arguments);
        assert_eq!(invocation["tool"]["_meta"]["ui"]["resourceUri"], APP_URI);
        assert_eq!(invocation["resource"], wire_resource(APP_URI));
        assert_eq!(*fixture.server.reads.lock().unwrap(), [APP_URI]);

        let cached = resolved_value(
            resolve(
                &router,
                invocation,
                ToolApplicationOperation::ReadResource {
                    uri: APP_URI.into(),
                },
            )
            .await
            .unwrap(),
        );
        assert_eq!(cached, wire_resource(APP_URI));
        assert_eq!(fixture.server.reads.lock().unwrap().len(), 1);
        let resolved = resolved_value(
            resolve(&router, invocation, ToolApplicationOperation::Resolve)
                .await
                .unwrap(),
        );
        assert_eq!(resolved["result"], wire_result("display"));
        assert_eq!(resolved["canCallTools"], true);
        assert_eq!(
            fixture.server.calls.lock().unwrap().len(),
            1,
            "view loading must not replay its tool"
        );
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}

#[tokio::test]
async fn app_only_raw_action_resolves_canonical_route_for_native_dispatch() {
    let fixture = Fixture::start().await;
    let router = fixture.router(true).await;
    let result = AssertUnwindSafe(async {
        let original = dispatch(&router, "workspace_display", json!({})).await;
        let invocation = &original.host_metadata[MCP_APPS_EXTENSION];
        let ToolApplicationResolution::Call {
            name,
            binding,
            project_result,
        } = resolve(&router, invocation, refresh()).await.unwrap()
        else {
            panic!("expected call");
        };
        assert_eq!(name, "workspace_refresh");
        assert_eq!(binding.extension, MCP_APPS_EXTENSION);
        let payload: crate::apps::McpAppCallBinding =
            serde_json::from_value(binding.payload).unwrap();
        assert_eq!(payload.source_tool, "workspace_display");
        assert_eq!(payload.target, "refresh");
        assert_eq!(
            payload.invocation.registration,
            serde_json::from_value::<McpAppInvocation>(invocation.clone())
                .unwrap()
                .registration
        );
        // The Core runner owns attaching this private binding after native
        // admission. An ordinary leaf dispatch has no such binding and must
        // not acquire an app result carrier merely from the target audience.
        let result = dispatch(&router, &name, json!({"row":"row-a"})).await;
        assert!(result.host_metadata.is_empty());
        assert!(project_result(&result).is_err());
        assert!(result.is_error, "MCP tool errors remain tool errors");
        assert_eq!(
            fixture.server.calls.lock().unwrap()[1],
            json!({"name":"refresh","arguments":{"row":"row-a"}})
        );

        for forbidden in ["display", "hidden", "workspace_refresh", "missing"] {
            assert!(
                resolve(
                    &router,
                    invocation,
                    ToolApplicationOperation::CallTool {
                        name: forbidden.into(),
                        arguments: json!({}),
                    }
                )
                .await
                .is_err()
            );
        }
        assert_eq!(fixture.server.calls.lock().unwrap().len(), 2);
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}

#[tokio::test]
async fn retired_or_replaced_physical_connection_cannot_receive_old_app_io() {
    let fixture = Fixture::start().await;
    let mut router = fixture.router(true).await;
    let result = AssertUnwindSafe(async {
        let original = dispatch(&router, "workspace_display", json!({})).await;
        let invocation = &original.host_metadata[MCP_APPS_EXTENSION];
        let original_registration = invocation["registration"].clone();
        assert!(resolve(&router, invocation, refresh()).await.is_ok());
        router.stage_remove("apps-fixture").unwrap();
        router.apply_staged().await.unwrap();
        router.progress_removals().await.unwrap();

        for replaced in [false, true] {
            if replaced {
                tokio::time::timeout(LIMIT, router.add_server(fixture.config.clone()))
                    .await
                    .unwrap()
                    .unwrap();
            }
            let resolved = resolved_value(
                resolve(&router, invocation, ToolApplicationOperation::Resolve)
                    .await
                    .unwrap(),
            );
            assert_eq!(resolved["canCallTools"], false);
            assert_eq!(resolved["result"], wire_result("display"));
            assert!(resolve(&router, invocation, refresh()).await.is_err());
            assert!(
                resolve(
                    &router,
                    invocation,
                    ToolApplicationOperation::ReadResource {
                        uri: OTHER_URI.into()
                    }
                )
                .await
                .is_err()
            );
            let archived = resolved_value(
                resolve(
                    &router,
                    invocation,
                    ToolApplicationOperation::ReadResource {
                        uri: APP_URI.into(),
                    },
                )
                .await
                .unwrap(),
            );
            assert_eq!(archived, wire_resource(APP_URI));
            assert_eq!(fixture.server.calls.lock().unwrap().len(), 1);
            assert_eq!(fixture.server.reads.lock().unwrap().len(), 1);
        }
        let fresh = dispatch(&router, "workspace_display", json!({})).await;
        let fresh = &fresh.host_metadata[MCP_APPS_EXTENSION];
        assert_ne!(fresh["registration"], original_registration);
        assert_eq!(
            fresh["tool"], invocation["tool"],
            "identical tool metadata is not connection custody"
        );
        assert!(resolve(&router, fresh, refresh()).await.is_ok());
    })
    .catch_unwind()
    .await;
    router.shutdown().await;
    fixture.finish(result).await;
}
