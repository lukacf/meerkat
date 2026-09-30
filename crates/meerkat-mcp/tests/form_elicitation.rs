//! Real rmcp transport, callback, and native-router evidence for the optional
//! form-only profile. No external accounts, fabricated session owners or UI.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "form_elicitation/http.rs"]
mod http;
use http::{Endpoint, Mode};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use meerkat_auth_core::{McpAuthMode, McpOAuthError, McpServerIdentity};
use meerkat_core::{
    AgentToolDispatcher, McpServerConfig, SessionId, ToolCallView, ToolDeadlineChain,
    ToolDeadlineContributor, ToolDeadlineOwner, ToolDispatchContext,
    ToolExecutionResolutionContext, dispatch_tool_execution_plan_fenced,
    resolve_tool_execution_plan_fenced,
};
use meerkat_mcp::{
    McpAuthResolver, McpClientServiceFactory, McpConnection, McpError, McpRouter, McpRouterAdapter,
};
use meerkat_runtime::{MeerkatMachine, session_runtime_bindings_have_machine_authority};
use rmcp::model::{
    ClientInfo, CreateElicitationRequestParams, CreateElicitationResult, ElicitationAction,
    ErrorCode,
};
use rmcp::service::{DynService, RequestContext};
use rmcp::{ClientHandler, ErrorData, RoleClient, ServiceExt};
use serde_json::{Value, json};

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

async fn cleanup_endpoint(
    endpoint: &mut Option<Endpoint>,
    expected_initialized: Option<bool>,
    errors: &mut Vec<String>,
) {
    if let Some(endpoint) = endpoint.as_mut()
        && let Some(report) = cleanup(errors, "endpoint shutdown", async {
            Ok::<_, std::convert::Infallible>(endpoint.shutdown().await)
        })
        .await
    {
        if report.joined != ["service", "pump", "http"] {
            errors.push(format!(
                "endpoint did not observe all task joins: {:?}",
                report.joined
            ));
        }
        if expected_initialized.is_some_and(|expected| report.initialized != expected) {
            errors.push(format!(
                "endpoint initialized={}, expected={expected_initialized:?}",
                report.initialized
            ));
        }
        errors.extend(report.errors);
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

#[derive(Default)]
struct Trace {
    selections: Vec<McpServerConfig>,
    requests: Vec<Value>,
}

fn host_info(account: &str, instance: usize, forms: bool) -> ClientInfo {
    // Intentionally over-declare to prove the first profile restricts only
    // capabilities and does not accidentally enable roots, URL or sampling.
    serde_json::from_value(json!({
        "protocolVersion": "2025-11-25", "_meta": {"selectedAccount": account},
        "capabilities": {"roots": {}, "sampling": {}, "experimental": {"fixture": {}},
            "elicitation": if forms { json!({"form": {}, "url": {}}) } else { json!({"url": {}}) }},
        "clientInfo": {"name": account, "version": instance.to_string(), "title": "Selected host"}
    }))
    .unwrap()
}

struct Host {
    account: String,
    instance: usize,
    forms: bool,
    trace: Arc<Mutex<Trace>>,
}
impl ClientHandler for Host {
    fn get_info(&self) -> ClientInfo {
        host_info(&self.account, self.instance, self.forms)
    }
    async fn create_elicitation(
        &self,
        request: CreateElicitationRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, ErrorData> {
        self.trace.lock().unwrap().requests.push(json!({
            "account": self.account, "instance": self.instance, "request": request, "meta": context.meta
        }));
        let CreateElicitationRequestParams::FormElicitationParams { message, .. } = request else {
            panic!("URL callback escaped the form guard");
        };
        let (action, content) = match message.as_str() {
            "decline" => (ElicitationAction::Decline, None),
            "cancel" => (ElicitationAction::Cancel, None),
            _ => (ElicitationAction::Accept, Some(json!({"confirm": true}))),
        };
        Ok(CreateElicitationResult {
            action,
            content,
            meta: Some(
                serde_json::from_value(json!({"account": self.account, "instance": self.instance}))
                    .unwrap(),
            ),
        })
    }
}

struct Factory {
    selected: Vec<(McpServerConfig, String)>,
    forms: bool,
    allowed_attempts: usize,
    trace: Arc<Mutex<Trace>>,
}
impl McpClientServiceFactory for Factory {
    fn create(
        &self,
        config: &McpServerConfig,
    ) -> Result<Box<dyn DynService<RoleClient>>, McpError> {
        let mut trace = self.trace.lock().unwrap();
        trace.selections.push(config.clone());
        if trace.selections.len() > self.allowed_attempts {
            return Err(McpError::ConnectionFailed {
                reason: "host refused connection retry".into(),
            });
        }
        let Some((_, account)) = self
            .selected
            .iter()
            .find(|(selected, _)| selected == config)
        else {
            return Err(McpError::ConnectionFailed {
                reason: "host refused unselected exact config".into(),
            });
        };
        Ok(Host {
            account: account.clone(),
            instance: trace.selections.len(),
            forms: self.forms,
            trace: Arc::clone(&self.trace),
        }
        .into_dyn())
    }
}
fn factory(
    configs: Vec<(McpServerConfig, &str)>,
    forms: bool,
) -> (Arc<Factory>, Arc<Mutex<Trace>>) {
    let trace = Arc::new(Mutex::new(Trace::default()));
    (
        Arc::new(Factory {
            selected: configs.into_iter().map(|(c, a)| (c, a.into())).collect(),
            forms,
            allowed_attempts: usize::MAX,
            trace: Arc::clone(&trace),
        }),
        trace,
    )
}

fn binary() -> PathBuf {
    let binary = PathBuf::from(std::env::var_os("MEERKAT_MCP_TEST_SERVER")
        .expect("set MEERKAT_MCP_TEST_SERVER to the exact fixture binary from ./scripts/repo-cargo build -p mcp-test-server; no ambient executable search is performed"));
    assert!(
        binary.is_file(),
        "form fixture binary missing at {}. First run ./scripts/repo-cargo build -p mcp-test-server, or set MEERKAT_MCP_TEST_SERVER to that built binary",
        binary.display()
    );
    binary
}
fn stdio_config(marker: Option<&std::path::Path>) -> McpServerConfig {
    let mut args = vec!["--form-elicitation".to_owned()];
    if let Some(marker) = marker {
        args.push(marker.to_str().unwrap().to_owned());
    }
    McpServerConfig::stdio(
        "same-display-name",
        binary().to_str().unwrap(),
        args,
        Default::default(),
    )
}
fn http_config(endpoint: &Endpoint, mode: Mode) -> McpServerConfig {
    match mode {
        Mode::Sse => McpServerConfig::sse("same-display-name", &endpoint.url, Default::default()),
        Mode::Streamable => {
            McpServerConfig::streamable_http("same-display-name", &endpoint.url, Default::default())
        }
    }
}
async fn receipt(connection: &McpConnection, message: &str) -> Value {
    serde_json::from_str(
        &tokio::time::timeout(
            LIMIT,
            connection.call_tool_text("mcp_form", &json!({"message": message})),
        )
        .await
        .unwrap()
        .unwrap(),
    )
    .unwrap()
}
fn assert_form(value: &Value, action: &str, account: Option<&str>, instance: usize) {
    assert_eq!(
        value["form"]["kind"], "result",
        "transport failures cannot stand in for an action: {value}"
    );
    assert_eq!(value["form"]["value"]["action"], action);
    if let Some(account) = account {
        assert_eq!(
            value["form"]["value"]["_meta"],
            json!({"account": account, "instance": instance})
        );
        let mut expected = serde_json::to_value(host_info(account, instance, true)).unwrap();
        expected["capabilities"] = json!({"elicitation": {"form": {}}});
        let expected_meta = expected.as_object_mut().unwrap().remove("_meta").unwrap();
        // rmcp moves request metadata into the actual RequestContext on decode.
        assert_eq!(value["initialize_meta"], expected_meta);
        assert_eq!(value["client_info"], expected);
    } else {
        assert_eq!(value["client_info"]["capabilities"], json!({}));
        assert!(value["form"]["value"].get("content").is_none());
    }
}

async fn exercise_transport(mode: Option<Mode>, with_host: bool) {
    let mut endpoint = None;
    let mut connection = None;
    let result = AssertUnwindSafe(async {
        endpoint = match mode {
            Some(mode) => Some(Endpoint::start(mode, None).await.unwrap()),
            None => None,
        };
        let config = match (&endpoint, mode) {
            (Some(endpoint), Some(mode)) => http_config(endpoint, mode),
            _ => stdio_config(None),
        };
        let (factory, trace) = factory(vec![(config.clone(), "work-account")], true);
        connection = Some(
            tokio::time::timeout(LIMIT, async {
                if with_host {
                    McpConnection::connect_with_services(
                        &config,
                        McpAuthMode::Stored,
                        None,
                        Some(factory.clone()),
                    )
                    .await
                } else {
                    McpConnection::connect(&config).await
                }
            })
            .await
            .unwrap()
            .unwrap(),
        );
        if with_host {
            assert_eq!(
                connection
                    .as_ref()
                    .unwrap()
                    .server_info()
                    .unwrap()
                    .protocol_version,
                host_info("work-account", 1, true).protocol_version
            );
        }
        for action in ["accept", "decline", "cancel"] {
            assert_form(
                &receipt(connection.as_ref().unwrap(), action).await,
                if with_host { action } else { "decline" },
                with_host.then_some("work-account"),
                1,
            );
        }
        let defaults: Value = serde_json::from_str(
            &connection
                .as_ref()
                .unwrap()
                .call_tool_text("mcp_defaults", &json!({}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(defaults["url"]["kind"], "result");
        assert_eq!(defaults["url"]["value"]["action"], "decline");
        assert_eq!(defaults["sampling"]["kind"], "protocol_error");
        assert_eq!(
            defaults["sampling"]["error"]["code"],
            ErrorCode::METHOD_NOT_FOUND.0
        );
        assert_eq!(
            trace.lock().unwrap().selections.len(),
            usize::from(with_host)
        );
        let trace = trace.lock().unwrap();
        assert_eq!(trace.requests.len(), if with_host { 3 } else { 0 });
        for (index, request) in trace.requests.iter().enumerate() {
            let expected =
                mcp_test_server::form_request(["accept", "decline", "cancel"][index]).unwrap();
            assert_eq!(request["request"], serde_json::to_value(expected).unwrap());
            assert_eq!(request["meta"].as_object().unwrap().len(), 1);
            assert!(request["meta"].get("progressToken").is_some());
        }
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(connection) = connection.take() {
        let _ = cleanup(&mut errors, "connection close", connection.close()).await;
    }
    cleanup_endpoint(&mut endpoint, result.is_ok().then_some(true), &mut errors).await;
    finish_test(result, errors);
}

#[tokio::test]
async fn form_stdio_default_and_optional_host() {
    exercise_transport(None, false).await;
    exercise_transport(None, true).await;
}
#[tokio::test]
async fn form_sse_default_and_optional_host() {
    exercise_transport(Some(Mode::Sse), false).await;
    exercise_transport(Some(Mode::Sse), true).await;
}
#[tokio::test]
async fn form_streamable_default_and_optional_host() {
    exercise_transport(Some(Mode::Streamable), false).await;
    exercise_transport(Some(Mode::Streamable), true).await;
}

#[tokio::test]
async fn form_factory_refusal_precedes_every_transport_effect() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let marker = std::env::temp_dir().join(format!(
        "meerkat-form-started-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    assert!(!marker.exists());
    let stdio = stdio_config(Some(&marker));
    let mut sse = None;
    let mut streamable = None;
    let mut unexpected_connection = None;
    let result = AssertUnwindSafe(async {
        sse = Some(Endpoint::start(Mode::Sse, None).await.unwrap());
        streamable = Some(Endpoint::start(Mode::Streamable, None).await.unwrap());
        let configs = [
            stdio,
            http_config(sse.as_ref().unwrap(), Mode::Sse),
            http_config(streamable.as_ref().unwrap(), Mode::Streamable),
        ];
        let (factory, trace) = factory(vec![], true);
        for config in &configs {
            let error = match McpConnection::connect_with_services(
                config,
                McpAuthMode::Interactive,
                None,
                Some(factory.clone()),
            )
            .await
            {
                Err(error) => error,
                Ok(connection) => {
                    unexpected_connection = Some(connection);
                    panic!("factory refusal unexpectedly created a connection");
                }
            };
            assert!(matches!(error, McpError::ConnectionFailed { reason }
                if reason == "host refused unselected exact config"));
        }
        assert!(
            !marker.exists(),
            "factory rejection spawned the selected child"
        );
        assert_eq!(sse.as_ref().unwrap().requests(), 0);
        assert_eq!(streamable.as_ref().unwrap().requests(), 0);
        assert_eq!(trace.lock().unwrap().selections, configs);
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(connection) = unexpected_connection.take() {
        let _ = cleanup(
            &mut errors,
            "unexpected connection close",
            connection.close(),
        )
        .await;
    }
    cleanup_endpoint(&mut sse, result.is_ok().then_some(false), &mut errors).await;
    cleanup_endpoint(
        &mut streamable,
        result.is_ok().then_some(false),
        &mut errors,
    )
    .await;
    if marker.exists()
        && let Err(error) = std::fs::remove_file(&marker)
    {
        errors.push(format!("marker cleanup: {error}"));
    }
    finish_test(result, errors);
}

#[tokio::test]
async fn form_selection_uses_full_config_and_protocol_transfer_keeps_one_owner() {
    let mut work = None;
    let mut personal = None;
    let mut first = None;
    let mut protocol = None;
    let result = AssertUnwindSafe(async {
        work = Some(Endpoint::start(Mode::Streamable, None).await.unwrap());
        personal = Some(Endpoint::start(Mode::Streamable, None).await.unwrap());
        let work_config = http_config(work.as_ref().unwrap(), Mode::Streamable);
        let personal_config = http_config(personal.as_ref().unwrap(), Mode::Streamable);
        assert_eq!(work_config.name, personal_config.name);
        let (factory, trace) = factory(
            vec![
                (work_config.clone(), "work"),
                (personal_config.clone(), "personal"),
            ],
            true,
        );
        first = Some(
            McpConnection::connect_with_services(
                &work_config,
                McpAuthMode::Stored,
                None,
                Some(factory.clone()),
            )
            .await
            .unwrap(),
        );
        protocol = Some(
            McpConnection::connect_with_services(
                &personal_config,
                McpAuthMode::Stored,
                None,
                Some(factory.clone()),
            )
            .await
            .unwrap()
            .into_protocol(),
        );
        assert_form(
            &receipt(first.as_ref().unwrap(), "accept").await,
            "accept",
            Some("work"),
            1,
        );
        let value: Value = serde_json::from_str(
            &protocol
                .as_ref()
                .unwrap()
                .call_tool_text("mcp_form", &json!({"message": "accept"}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_form(&value, "accept", Some("personal"), 2);
        assert_eq!(
            trace.lock().unwrap().selections,
            [work_config, personal_config]
        );
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(connection) = first.take() {
        let _ = cleanup(&mut errors, "work connection close", connection.close()).await;
    }
    if let Some(protocol) = protocol.take() {
        let _ = cleanup(&mut errors, "personal protocol close", protocol.close()).await;
    }
    cleanup_endpoint(&mut work, result.is_ok().then_some(true), &mut errors).await;
    cleanup_endpoint(&mut personal, result.is_ok().then_some(true), &mut errors).await;
    finish_test(result, errors);
}

#[tokio::test]
async fn form_undeclared_host_capability_keeps_default_decline() {
    let mut endpoint = None;
    let mut connection = None;
    let result = AssertUnwindSafe(async {
        endpoint = Some(Endpoint::start(Mode::Streamable, None).await.unwrap());
        let config = http_config(endpoint.as_ref().unwrap(), Mode::Streamable);
        let (factory, trace) = factory(vec![(config.clone(), "no-forms")], false);
        connection = Some(
            McpConnection::connect_with_services(&config, McpAuthMode::Stored, None, Some(factory))
                .await
                .unwrap(),
        );
        assert_form(
            &receipt(connection.as_ref().unwrap(), "accept").await,
            "decline",
            None,
            1,
        );
        assert!(trace.lock().unwrap().requests.is_empty());
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(connection) = connection.take() {
        let _ = cleanup(&mut errors, "connection close", connection.close()).await;
    }
    cleanup_endpoint(&mut endpoint, result.is_ok().then_some(true), &mut errors).await;
    finish_test(result, errors);
}

#[derive(Default)]
struct Resolver {
    targets: Mutex<Vec<McpServerIdentity>>,
    logins: AtomicUsize,
}
#[async_trait::async_trait]
impl McpAuthResolver for Resolver {
    async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        self.targets.lock().unwrap().push(target.clone());
        Ok(Some("stale-fixture-token".into()))
    }
    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        challenge: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        self.targets.lock().unwrap().push(target.clone());
        assert_eq!(challenge, Some("Bearer realm=\"form-fixture\""));
        self.logins.fetch_add(1, Ordering::SeqCst);
        Ok("fresh-fixture-token".into())
    }
}

#[tokio::test]
async fn form_http_auth_retry_keeps_resolver_identity_and_selects_fresh_host() {
    let mut endpoint = None;
    let mut connection = None;
    let result = AssertUnwindSafe(async {
        endpoint = Some(
            Endpoint::start(Mode::Streamable, Some("Bearer fresh-fixture-token"))
                .await
                .unwrap(),
        );
        let config = http_config(endpoint.as_ref().unwrap(), Mode::Streamable);
        let (factory, trace) = factory(vec![(config.clone(), "retried")], true);
        let resolver = Arc::new(Resolver::default());
        connection = Some(
            McpConnection::connect_with_services(
                &config,
                McpAuthMode::Interactive,
                Some(resolver.clone()),
                Some(factory),
            )
            .await
            .unwrap(),
        );
        assert_form(
            &receipt(connection.as_ref().unwrap(), "accept").await,
            "accept",
            Some("retried"),
            2,
        );
        assert_eq!(
            trace.lock().unwrap().selections,
            [config.clone(), config.clone()]
        );
        assert_eq!(resolver.logins.load(Ordering::SeqCst), 1);
        assert_eq!(
            *resolver.targets.lock().unwrap(),
            vec![
                McpServerIdentity::from_server_config(
                    config.name.clone(),
                    endpoint.as_ref().unwrap().url.clone()
                );
                2
            ]
        );
        assert_eq!(endpoint.as_ref().unwrap().rejected_auth(), 1);
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(connection) = connection.take() {
        let _ = cleanup(&mut errors, "connection close", connection.close()).await;
    }
    cleanup_endpoint(&mut endpoint, result.is_ok().then_some(true), &mut errors).await;
    finish_test(result, errors);
}

fn resolution() -> ToolExecutionResolutionContext {
    ToolExecutionResolutionContext::new(
        ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::CoreToolDispatch,
            LIMIT,
        )])
        .unwrap(),
    )
}

#[tokio::test]
async fn form_native_staged_router_uses_canonical_session_and_selected_host() {
    let machine = MeerkatMachine::ephemeral();
    let bindings = machine.prepare_bindings(SessionId::new()).await.unwrap();
    assert!(session_runtime_bindings_have_machine_authority(&bindings));
    let mut endpoint = None;
    let mut retained_adapter = None;
    let result = AssertUnwindSafe(async {
        endpoint = Some(Endpoint::start(Mode::Streamable, None).await.unwrap());
        let config = http_config(endpoint.as_ref().unwrap(), Mode::Streamable);
        let (factory, trace) = factory(vec![(config.clone(), "native-staged")], true);
        retained_adapter = Some(Arc::new(McpRouterAdapter::new(
            McpRouter::new().with_client_service_factory(factory),
        )));
        let adapter = retained_adapter.as_ref().unwrap();
        adapter.bind_external_tool_surface_handle(Arc::clone(bindings.external_tool_surface()));
        adapter.bind_mcp_server_lifecycle_handle(Arc::clone(bindings.mcp_server_lifecycle()));
        adapter.stage_add(config.clone()).await.unwrap();
        assert!(
            adapter
                .apply_staged()
                .await
                .unwrap()
                .delta
                .rejected_boundaries
                .is_empty()
        );
        adapter.wait_until_ready(LIMIT).await.unwrap();
        assert!(
            bindings
                .external_tool_surface()
                .visible_surfaces()
                .contains(&config.name)
        );
        let args =
            serde_json::value::RawValue::from_string("{\"message\":\"accept\"}".into()).unwrap();
        let call = ToolCallView {
            id: "form-call",
            name: "mcp_form",
            args: &args,
        };
        let plan = resolve_tool_execution_plan_fenced(
            adapter,
            call,
            &ToolDispatchContext::default(),
            &resolution(),
        )
        .unwrap();
        assert!(plan.owner_witness("root-dispatcher").is_some());
        assert!(
            plan.owner_witnesses()
                .iter()
                .any(|w| w.authority_key().starts_with("mcp-router-adapter:"))
        );
        let output = dispatch_tool_execution_plan_fenced(
            adapter,
            call,
            &ToolDispatchContext::default(),
            &plan,
        )
        .await
        .unwrap();
        assert!(!output.result.is_error);
        assert_form(
            &serde_json::from_str(&output.result.text_content()).unwrap(),
            "accept",
            Some("native-staged"),
            1,
        );
        adapter.stage_remove(&config.name).await.unwrap();
        let removed = adapter.apply_staged().await.unwrap();
        assert_eq!(
            removed.delta.removed_servers,
            std::slice::from_ref(&config.name)
        );
        assert!(removed.delta.degraded_removals.is_empty());
        assert!(
            resolve_tool_execution_plan_fenced(
                adapter,
                call,
                &ToolDispatchContext::default(),
                &resolution()
            )
            .is_err()
        );
        assert_eq!(
            trace.lock().unwrap().selections,
            std::slice::from_ref(&config)
        );
        assert_eq!(trace.lock().unwrap().requests.len(), 1);
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(adapter) = retained_adapter.take() {
        let _ = cleanup(&mut errors, "native adapter shutdown", async {
            adapter.shutdown().await;
            Ok::<_, std::convert::Infallible>(())
        })
        .await;
    }
    // Native removal alone does not join the host's fixture owners.
    cleanup_endpoint(&mut endpoint, result.is_ok().then_some(true), &mut errors).await;
    finish_test(result, errors);
}

#[tokio::test]
async fn form_native_immediate_router_path_uses_the_same_factory() {
    let machine = MeerkatMachine::ephemeral();
    let bindings = machine.prepare_bindings(SessionId::new()).await.unwrap();
    let mut endpoint = None;
    let mut retained_router = None;
    let result = AssertUnwindSafe(async {
        endpoint = Some(Endpoint::start(Mode::Streamable, None).await.unwrap());
        let config = http_config(endpoint.as_ref().unwrap(), Mode::Streamable);
        let (factory, trace) = factory(vec![(config.clone(), "native-immediate")], true);
        retained_router = Some(
            McpRouter::new_with_surface_handle(Arc::clone(bindings.external_tool_surface()))
                .with_client_service_factory(factory),
        );
        let router = retained_router.as_mut().unwrap();
        *router.mcp_lifecycle_handle_slot().write().unwrap() =
            Some(Arc::clone(bindings.mcp_server_lifecycle()));
        router.add_server(config.clone()).await.unwrap();
        let blocks = router
            .call_tool("mcp_form", &json!({"message": "accept"}))
            .await
            .unwrap();
        let [meerkat_core::ContentBlock::Text { text }] = blocks.as_slice() else {
            panic!("expected fixture text")
        };
        assert_form(
            &serde_json::from_str(text).unwrap(),
            "accept",
            Some("native-immediate"),
            1,
        );
        assert_eq!(
            trace.lock().unwrap().selections,
            std::slice::from_ref(&config)
        );
        assert_eq!(trace.lock().unwrap().requests.len(), 1);
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(router) = retained_router.take() {
        let _ = cleanup(&mut errors, "native router shutdown", async {
            router.shutdown().await;
            Ok::<_, std::convert::Infallible>(())
        })
        .await;
    }
    cleanup_endpoint(&mut endpoint, result.is_ok().then_some(true), &mut errors).await;
    finish_test(result, errors);
}

#[tokio::test]
async fn form_retry_factory_refusal_precedes_the_second_http_attempt() {
    let mut endpoint = None;
    let mut unexpected_connection = None;
    let result = AssertUnwindSafe(async {
        endpoint = Some(
            Endpoint::start(Mode::Streamable, Some("Bearer fresh-fixture-token"))
                .await
                .unwrap(),
        );
        let config = http_config(endpoint.as_ref().unwrap(), Mode::Streamable);
        let trace = Arc::new(Mutex::new(Trace::default()));
        let factory = Arc::new(Factory {
            selected: vec![(config.clone(), "refuse-retry".into())],
            forms: true,
            allowed_attempts: 1,
            trace: Arc::clone(&trace),
        });
        let resolver = Arc::new(Resolver::default());
        let error = match McpConnection::connect_with_services(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
            Some(factory),
        )
        .await
        {
            Err(error) => error,
            Ok(connection) => {
                unexpected_connection = Some(connection);
                panic!("retry refusal unexpectedly created a connection");
            }
        };
        assert!(matches!(error, McpError::ConnectionFailed { reason }
            if reason == "host refused connection retry"));
        assert_eq!(endpoint.as_ref().unwrap().requests(), 1);
        assert_eq!(endpoint.as_ref().unwrap().rejected_auth(), 1);
        assert_eq!(resolver.logins.load(Ordering::SeqCst), 1);
        assert_eq!(
            trace.lock().unwrap().selections,
            [config.clone(), config.clone()]
        );
        assert!(trace.lock().unwrap().requests.is_empty());
    })
    .catch_unwind()
    .await;
    let mut errors = vec![];
    if let Some(connection) = unexpected_connection.take() {
        let _ = cleanup(
            &mut errors,
            "unexpected retry connection close",
            connection.close(),
        )
        .await;
    }
    cleanup_endpoint(&mut endpoint, result.is_ok().then_some(false), &mut errors).await;
    finish_test(result, errors);
}

#[tokio::test]
async fn form_fixture_service_join_failure_still_joins_pump_and_http_and_preserves_body_panic() {
    let mut endpoint = Endpoint::start(Mode::Streamable, None).await.unwrap();
    let body: std::thread::Result<()> = AssertUnwindSafe(async {
        endpoint.abort_service_for_test();
        panic!("original fixture body panic");
    })
    .catch_unwind()
    .await;
    let report = endpoint.shutdown().await;
    assert_eq!(report.joined, ["service", "pump", "http"]);
    assert!(!report.initialized);
    assert_eq!(
        report.errors.len(),
        1,
        "later owned joins must succeed: {:?}",
        report.errors
    );
    assert!(report.errors[0].starts_with("service task:"));
    assert!(report.errors[0].contains("cancelled"));
    let propagated =
        std::panic::catch_unwind(AssertUnwindSafe(|| finish_test(body, report.errors)));
    assert_eq!(
        panic_message(propagated.unwrap_err().as_ref()),
        "original fixture body panic"
    );
}

#[tokio::test]
async fn form_fixture_second_bind_failure_joins_the_first_endpoint() {
    let mut first = None;
    let body: std::thread::Result<()> = AssertUnwindSafe(async {
        first = Some(Endpoint::start(Mode::Streamable, None).await.unwrap());
        let address = first
            .as_ref()
            .unwrap()
            .url
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/mcp")
            .unwrap()
            .parse()
            .unwrap();
        // An actual occupied listener is the failing second preflight.
        let second = Endpoint::start_at(Mode::Sse, None, address).await;
        match second {
            Err(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
                panic!("second endpoint preflight refused");
            }
            Ok(mut unexpected) => {
                let report = unexpected.shutdown().await;
                panic!("occupied endpoint unexpectedly acquired: {report:?}");
            }
        }
    })
    .catch_unwind()
    .await;
    let report = first
        .as_mut()
        .expect("first endpoint was acquired")
        .shutdown()
        .await;
    assert_eq!(report.joined, ["service", "pump", "http"]);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(!report.initialized);
    assert_eq!(
        panic_message(body.unwrap_err().as_ref()),
        "second endpoint preflight refused"
    );
}
