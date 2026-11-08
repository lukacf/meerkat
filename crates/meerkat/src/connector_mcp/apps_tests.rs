//! MCP App views read through the stock connector call-context provider.
//!
//! Every factory with a connector credential owner (each runtime-backed
//! factory with provider-auth persistence, the gateway member factories among
//! them) attaches the stock provider to every MCP router it builds. These
//! tests build member constructions through that factory composition with MCP
//! Apps enabled, then read the `ui://` views one of the member's own servers
//! declares: at tool-call time (the router reads the declared view before the
//! call) and afterwards through the tool application seam that a Console
//! `read-resource` request enters.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::tool_application::ToolApplicationResolution;
use meerkat_core::{
    AgentToolDispatcher, SessionId, ToolApplicationOperation, ToolApplicationRequest, ToolError,
};
use meerkat_mcp::McpResourceTarget;
use meerkat_mcp::apps::MCP_APPS_EXTENSION;
use serde_json::{Value, json};

const APP_URI: &str = "ui://fixture/view.html";
const OTHER_URI: &str = "ui://fixture/other.html";
const LIMIT: Duration = Duration::from_secs(20);
const LAUNCH: &str = "app-launch";

/// A stdio MCP server with one MCP App: `display` declares the view
/// `APP_URI`, and every `ui://fixture/` resource reads back as an MCP App
/// view. It appends each request line it receives to `log`.
fn apps_server(name: &str, log: &Path) -> McpServerConfig {
    let script = format!(
        r#"while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}},"resources":{{}}}},"serverInfo":{{"name":"apps","version":"0"}}}}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"display","inputSchema":{{"type":"object"}},"_meta":{{"ui":{{"resourceUri":"{app}"}}}}}}]}}}}\n' "$id" ;;
    *'"method":"resources/read"'*)
      uri=$(printf '%s\n' "$line" | sed -n 's/.*"uri":"\(ui:\/\/fixture\/[a-z.]*\)".*/\1/p')
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"contents":[{{"uri":"%s","mimeType":"text/html;profile=mcp-app","text":"<p>fixture view</p>"}}]}}}}\n' "$id" "$uri" ;;
    *'"method":"tools/call"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"shown"}}]}}}}\n' "$id" ;;
    *) if [ -n "$id" ]; then printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32601,"message":"unsupported"}}}}\n' "$id"; fi ;;
  esac
done
"#,
        log = log.display(),
        app = APP_URI,
    );
    McpServerConfig::stdio(
        name,
        "/bin/sh",
        vec!["-c".to_owned(), script],
        HashMap::new(),
    )
}

/// `config` as a trusted connector installation whose `display` operation
/// needs the slot's credential.
fn bound(mut config: McpServerConfig) -> McpServerConfig {
    config.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
        realm_id: "tenant-a".into(),
        slot_id: "apps-work".into(),
        issuer: "https://issuer.example".into(),
        client: "client".into(),
        resource: "https://apps.example".into(),
        strategy_id: "oidc-userinfo-v1".into(),
        account: "subject-a".into(),
        operations: BTreeMap::from([(
            "display".to_owned(),
            meerkat_core::mcp_config::McpConnectorOperation {
                required_scopes: ["apps.read".to_owned()].into(),
                destination_origin: "https://apps.example".into(),
                read_only: true,
            },
        )]),
    });
    config
}

/// The request lines `log` received for `method`.
fn requests(log: &Path, method: &str) -> Vec<String> {
    let needle = format!("\"method\":\"{method}\"");
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(&needle))
        .map(str::to_owned)
        .collect()
}

/// The connector credential owner's token store, counting every access.
#[derive(Default)]
struct CountingTokenStore {
    inner: meerkat_providers::auth_store::EphemeralTokenStore,
    accesses: AtomicUsize,
}

impl CountingTokenStore {
    fn accesses(&self) -> usize {
        self.accesses.load(Ordering::SeqCst)
    }

    fn count(&self) {
        self.accesses.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl meerkat_core::auth::TokenStore for CountingTokenStore {
    async fn load(
        &self,
        key: &meerkat_core::auth::TokenKey,
    ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
    {
        self.count();
        self.inner.load(key).await
    }

    async fn save(
        &self,
        fence: &meerkat_core::AdmissionGuard,
        key: &meerkat_core::auth::TokenKey,
        tokens: &meerkat_core::auth::PersistedTokens,
    ) -> Result<(), meerkat_core::auth::TokenStoreError> {
        self.count();
        self.inner.save(fence, key, tokens).await
    }

    async fn clear(
        &self,
        fence: &meerkat_core::AdmissionGuard,
        key: &meerkat_core::auth::TokenKey,
    ) -> Result<(), meerkat_core::auth::TokenStoreError> {
        self.count();
        self.inner.clear(fence, key).await
    }

    async fn list(
        &self,
    ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError> {
        self.count();
        self.inner.list().await
    }

    fn backend_name(&self) -> &'static str {
        "counting"
    }

    async fn observe_status(
        &self,
        key: &meerkat_core::auth::TokenKey,
    ) -> Result<meerkat_core::auth::TokenStoreObservation, meerkat_core::auth::TokenStoreError>
    {
        self.count();
        self.inner.observe_status(key).await
    }
}

/// The connector owner over `store`, bound to `runtime`'s AuthMachine owner.
fn connector_owner(
    runtime: &meerkat_runtime::MeerkatMachine,
    store: &Arc<CountingTokenStore>,
) -> ConnectorOAuthAuthority {
    crate::HostAuthService::new(
        meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::clone(store) as Arc<dyn meerkat_core::auth::TokenStore>,
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        ),
        runtime.provider_auth_runtime_authority(),
    )
    .connector_oauth_authority()
    .unwrap()
}

/// The trusted host composition seam, recording each member construction's
/// exact MCP adapter and publishing it unchanged.
#[derive(Default)]
struct Members(Mutex<Vec<(String, Arc<meerkat_mcp::McpRouterAdapter>)>>);

impl crate::McpAdapterComposer for Members {
    fn compose(
        &self,
        adapter: &Arc<meerkat_mcp::McpRouterAdapter>,
        _session_id: &SessionId,
        member: Option<&meerkat_core::MobMemberBinding>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, crate::McpAdapterComposeRefusal> {
        let member = member
            .map(|member| member.member.clone())
            .unwrap_or_default();
        self.0.lock().unwrap().push((member, Arc::clone(adapter)));
        Ok(Arc::clone(adapter) as Arc<dyn AgentToolDispatcher>)
    }
}

impl Members {
    fn adapter(&self, member: &str) -> Arc<meerkat_mcp::McpRouterAdapter> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|(name, _)| name == member)
            .map(|(_, adapter)| Arc::clone(adapter))
            .expect("the member's MCP adapter")
    }
}

/// A factory as a runtime-backed host composes one: MCP Apps on, the
/// member adapters recorded by `members`.
fn apps_factory(root: &Path, members: &Arc<Members>) -> crate::AgentFactory {
    crate::AgentFactory::new(root.join("sessions"))
        .builtins(false)
        .mcp_apps(true)
        .with_mcp_adapter_composer(Arc::clone(members) as Arc<dyn crate::McpAdapterComposer>)
}

/// One member construction through `factory`, whose own `tools.mcp_servers`
/// are `servers`, connected before it returns.
async fn build_member(
    factory: &crate::AgentFactory,
    runtime: &meerkat_runtime::MeerkatMachine,
    member: &str,
    servers: Vec<McpServerConfig>,
) -> crate::DynAgent {
    let session = meerkat_core::Session::new();
    let bindings = runtime
        .prepare_bindings(session.id().clone())
        .await
        .expect("member bindings");
    let mut build = crate::AgentBuildConfig::new("claude-sonnet-4-5");
    build.provider = Some(meerkat_core::Provider::Anthropic);
    build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
    build.resume_session = Some(session);
    build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings);
    build.override_builtins = crate::ToolCategoryOverride::Disable;
    build.mob_member_binding = Some(meerkat_core::MobMemberBinding {
        mob_id: "mob".into(),
        role: "worker".into(),
        member: member.into(),
    });
    build.mcp_servers = servers;
    build.wait_for_mcp = true;
    factory
        .build_agent(build, &meerkat_core::Config::default())
        .await
        .expect("the member builds")
}

fn dispatch_context(session: &SessionId) -> ToolDispatchContext {
    ToolDispatchContext::default().with_runtime_identity(session.clone(), None)
}

/// Call `display` through `dispatcher`; the MCP Apps host record its result
/// carries.
async fn display(
    dispatcher: &dyn AgentToolDispatcher,
    session: &SessionId,
) -> Result<Value, ToolError> {
    let args = serde_json::value::to_raw_value(&json!({})).unwrap();
    let outcome = tokio::time::timeout(
        LIMIT,
        dispatcher.dispatch_with_context(
            ToolCallView {
                id: LAUNCH,
                name: "display",
                args: &args,
            },
            &dispatch_context(session),
        ),
    )
    .await
    .expect("display settles")?;
    Ok(outcome
        .result
        .host_metadata
        .get(MCP_APPS_EXTENSION)
        .cloned()
        .expect("an MCP Apps host record"))
}

/// The view a host record retained from the tool-call-time read, if any.
fn retained_view(record: &Value) -> Option<String> {
    record["resource"]["contents"][0]["uri"]
        .as_str()
        .map(str::to_owned)
}

/// An authenticated Console viewer whose admission stays current.
struct Viewer;

impl meerkat_core::ToolApplicationIngress for Viewer {
    fn revalidate(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        Ok(())
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

fn read_request(uri: &str) -> ToolApplicationRequest {
    ToolApplicationRequest {
        tool_call_id: LAUNCH.into(),
        extension: MCP_APPS_EXTENSION.into(),
        operation: ToolApplicationOperation::ReadResource { uri: uri.into() },
    }
}

/// A fresh read of `uri` for the app `record` launched, through the tool
/// application seam (with `context`'s work authority, if any).
async fn read_view_in(
    dispatcher: &dyn AgentToolDispatcher,
    context: ToolDispatchContext,
    session: &SessionId,
    record: &Value,
    uri: &str,
) -> Result<Value, ToolError> {
    let request = read_request(uri);
    let control = meerkat_core::ToolApplicationControlRequest::from_trusted_ingress(
        session.clone(),
        request.clone(),
        Arc::new(Viewer),
    )
    .expect("trusted viewer ingress");
    let context = context.with_tool_application_control(control);
    match tokio::time::timeout(
        LIMIT,
        dispatcher.resolve_tool_application("display", &request, record, &context),
    )
    .await
    .expect("the read settles")?
    {
        ToolApplicationResolution::Value(value) => Ok(value),
        _ => panic!("a resource read resolves to a value"),
    }
}

async fn read_view(
    dispatcher: &dyn AgentToolDispatcher,
    session: &SessionId,
    record: &Value,
    uri: &str,
) -> Result<Value, ToolError> {
    read_view_in(dispatcher, dispatch_context(session), session, record, uri).await
}

#[derive(Clone, Copy, Debug)]
enum ProviderKind {
    /// The factory default over a connector owner.
    Live,
    /// A host-installed fixed list naming only another (bound) installation.
    Fixed,
}

/// A member whose router carries `kind` of stock connector provider reads
/// the view its own unbound server declares at tool-call time and a further
/// view of the same app through the tool application seam. Both reads reach
/// the server with no connector credential, and the credential owner is
/// never consulted.
async fn a_member_reads_its_own_app_views(kind: ProviderKind) {
    let temp = tempfile::TempDir::new().unwrap();
    let log = temp.path().join("apps.log");
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let store = Arc::new(CountingTokenStore::default());
    let owner = connector_owner(&runtime, &store);
    let members = Arc::new(Members::default());
    let factory = apps_factory(temp.path(), &members);
    let factory = match kind {
        ProviderKind::Live => factory.connector_oauth_authority(owner),
        ProviderKind::Fixed => {
            let mail = bound(apps_server("mail", &temp.path().join("mail.log")));
            factory.mcp_call_context_provider(Arc::new(ConnectorMcpCallContextProvider::new(
                owner,
                vec![
                    ConnectorMcpInstallation::from_config(&mail)
                        .unwrap()
                        .unwrap(),
                ],
            )))
        }
    };
    let _member = build_member(
        &factory,
        &runtime,
        "worker-1",
        vec![apps_server("apps", &log)],
    )
    .await;
    let adapter = members.adapter("worker-1");
    let session = SessionId::new();
    let consulted = store.accesses();

    let record = display(&*adapter, &session)
        .await
        .expect("display is called");
    let fresh = read_view(&*adapter, &session, &record, OTHER_URI).await;
    let reads = requests(&log, "resources/read");
    assert!(
        retained_view(&record).as_deref() == Some(APP_URI)
            && fresh
                .as_ref()
                .is_ok_and(|view| view["contents"][0]["uri"] == OTHER_URI),
        "{kind:?}: the member reads its own server's views; tool-call-time read: {:?}, \
         fresh read: {fresh:?}, resources/read requests that reached the server: {}",
        retained_view(&record),
        reads.len()
    );
    assert_eq!(reads.len(), 2, "{kind:?}: one request per view: {reads:?}");
    assert!(
        reads
            .iter()
            .all(|line| !line.contains(CONNECTOR_CREDENTIAL_META_KEY)),
        "{kind:?}: a read of an unbound server carries no connector credential: {reads:?}"
    );
    assert_eq!(
        store.accesses(),
        consulted,
        "{kind:?}: the connector credential owner is never consulted"
    );
    adapter.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_member_reads_its_own_app_views_through_the_live_connector_provider() {
    a_member_reads_its_own_app_views(ProviderKind::Live).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_member_reads_its_own_app_views_through_a_fixed_connector_provider() {
    a_member_reads_its_own_app_views(ProviderKind::Fixed).await;
}

/// Under the stock provider, a view read reaches only the exact connection
/// of the member whose tool call launched the app. Another member's router
/// with a server of the same name (its own configuration and process), and
/// one without that server at all, refuse the read before transport.
#[cfg(unix)]
#[tokio::test]
async fn a_view_read_reaches_only_the_launching_members_own_connection() {
    let temp = tempfile::TempDir::new().unwrap();
    let logs = ["one", "two", "three"].map(|name| temp.path().join(format!("{name}.log")));
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let store = Arc::new(CountingTokenStore::default());
    let members = Arc::new(Members::default());
    let factory = apps_factory(temp.path(), &members)
        .connector_oauth_authority(connector_owner(&runtime, &store));
    let _one = build_member(
        &factory,
        &runtime,
        "worker-1",
        vec![apps_server("apps", &logs[0])],
    )
    .await;
    let _two = build_member(
        &factory,
        &runtime,
        "worker-2",
        vec![apps_server("apps", &logs[1])],
    )
    .await;
    let _three = build_member(
        &factory,
        &runtime,
        "worker-3",
        vec![apps_server("other", &logs[2])],
    )
    .await;
    let adapters = ["worker-1", "worker-2", "worker-3"].map(|member| members.adapter(member));
    let session = SessionId::new();

    let record = display(&*adapters[0], &session)
        .await
        .expect("worker-1 calls display");
    let launched = requests(&logs[0], "resources/read").len();
    for (member, adapter) in [("worker-2", &adapters[1]), ("worker-3", &adapters[2])] {
        let refused = read_view(&**adapter, &session, &record, OTHER_URI).await;
        assert_eq!(
            refused.err(),
            Some(ToolError::access_denied("display")),
            "{member} cannot read worker-1's app view"
        );
    }
    assert!(requests(&logs[1], "resources/read").is_empty());
    assert!(requests(&logs[2], "resources/read").is_empty());
    assert_eq!(requests(&logs[0], "resources/read").len(), launched);

    // Control: the launching member reads it.
    let own = read_view(&*adapters[0], &session, &record, OTHER_URI).await;
    assert_eq!(
        own.as_ref()
            .map(|view| view["contents"][0]["uri"].clone())
            .ok(),
        Some(json!(OTHER_URI)),
        "worker-1 reads its own app view: {own:?}"
    );
    assert_eq!(requests(&logs[0], "resources/read").len(), launched + 1);
    for adapter in adapters {
        adapter.shutdown().await;
    }
}

/// Test host preparation only, not a stock policy: every call and read is
/// prepared with no metadata, so an app (a connector-bound one included,
/// whose calls and reads the router never sends unprepared) can be launched
/// before a stock provider is attached.
struct Launcher;

#[async_trait]
impl McpCallContextProvider for Launcher {
    async fn prepare_resource(
        &self,
        _target: McpResourceTarget<'_>,
        _context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        Ok(Some(McpCallContext::new(serde_json::Map::new(), ())))
    }

    async fn prepare(
        &self,
        _target: McpCallTarget<'_>,
        _call: ToolCallView<'_>,
        _context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        Ok(Some(McpCallContext::new(serde_json::Map::new(), ())))
    }
}

/// A router whose app on `config` was launched under the test `Launcher`:
/// the router and that launch's host record. The launch read the app's
/// declared view once.
async fn launched(
    config: McpServerConfig,
    log: &Path,
    session: &SessionId,
) -> (meerkat_mcp::McpRouter, Value) {
    let mut router = crate::mcp::standalone_router_with_stdio_profile(
        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
    )
    .with_client_service_factory(Arc::new(meerkat_mcp::apps::McpAppsClientServiceFactory))
    .with_call_context_provider(Arc::new(Launcher));
    tokio::time::timeout(LIMIT, router.add_server(config))
        .await
        .expect("the server connects in time")
        .expect("the server connects");
    let record = display(&router, session).await.expect("display is called");
    assert_eq!(
        requests(log, "resources/read").len(),
        1,
        "the launch's own read"
    );
    (router, record)
}

/// A connector binding declares what each tool operation needs, never a
/// resource read. So under either kind of stock provider, a view read of a
/// connector-bound server (or of a server that shares only a trusted
/// installation's name) is refused as denied before anything is sent, carries
/// no credential, and never consults the credential owner; the bound app's
/// ungoverned tool call stays refused too. A bound server that gets no
/// preparation at all (a fixed list that does not name it, or no provider)
/// is refused unprepared by the router, before even its tool-call-time read.
#[cfg(unix)]
#[tokio::test]
async fn connector_bound_views_are_refused_before_transport_without_the_credential() {
    let temp = tempfile::TempDir::new().unwrap();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let store = Arc::new(CountingTokenStore::default());
    let session = SessionId::new();
    let log = temp.path().join("bound.log");
    let bound_apps = bound(apps_server("apps", &log));
    let installation = ConnectorMcpInstallation::from_config(&bound_apps)
        .unwrap()
        .unwrap();
    let (router, record) = launched(bound_apps, &log, &session).await;
    let consulted = store.accesses();
    let providers: [(&str, Arc<dyn McpCallContextProvider>); 2] = [
        (
            "live",
            Arc::new(ConnectorMcpCallContextProvider::live(connector_owner(
                &runtime, &store,
            ))),
        ),
        (
            "fixed",
            Arc::new(ConnectorMcpCallContextProvider::new(
                connector_owner(&runtime, &store),
                vec![installation.clone()],
            )),
        ),
    ];
    let mut router = router;
    for (kind, provider) in providers {
        router = router.with_call_context_provider(provider);
        let refused = read_view(&router, &session, &record, OTHER_URI).await;
        assert_eq!(
            refused.err(),
            Some(ToolError::access_denied("display")),
            "{kind}: a bound server's view read is refused as denied"
        );
        // The bound app's ungoverned launch: its tool-call-time read is
        // refused before transport and its call is refused as before.
        assert_eq!(
            display(&router, &session).await.err(),
            Some(ToolError::access_denied("display")),
            "{kind}: an ungoverned bound call is refused"
        );
        assert_eq!(
            requests(&log, "resources/read").len(),
            1,
            "{kind}: nothing more was read"
        );
        assert_eq!(
            requests(&log, "tools/call").len(),
            1,
            "{kind}: nothing more was called"
        );
        assert!(
            !std::fs::read_to_string(&log)
                .unwrap()
                .contains(CONNECTOR_CREDENTIAL_META_KEY),
            "{kind}: no credential reached the server"
        );
        assert_eq!(
            store.accesses(),
            consulted,
            "{kind}: the credential owner is never consulted"
        );
    }

    // Missing preparation. A fixed list that names only another
    // installation prepares nothing for this bound server, and a router
    // without a provider prepares nothing at all. The router refuses such a
    // bound server's reads, as it refuses its calls: neither a fresh read nor
    // the tool-call-time read reaches it.
    let unlisted = bound(apps_server("mail", &temp.path().join("mail.log")));
    router = router.with_call_context_provider(Arc::new(ConnectorMcpCallContextProvider::new(
        connector_owner(&runtime, &store),
        vec![
            ConnectorMcpInstallation::from_config(&unlisted)
                .unwrap()
                .unwrap(),
        ],
    )));
    let unlisted_read = read_view(&router, &session, &record, OTHER_URI).await.err();
    let unlisted_call = display(&router, &session).await.err();
    let unprepared_log = temp.path().join("unprepared.log");
    let mut unprepared = crate::mcp::standalone_router_with_stdio_profile(
        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
    )
    .with_client_service_factory(Arc::new(meerkat_mcp::apps::McpAppsClientServiceFactory));
    tokio::time::timeout(
        LIMIT,
        unprepared.add_server(bound(apps_server("apps", &unprepared_log))),
    )
    .await
    .expect("the server connects in time")
    .expect("the server connects");
    let unprepared_call = display(&unprepared, &session).await.err();
    let unavailable = Some(ToolError::unavailable(
        "display",
        meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
    ));
    let reads = (
        requests(&log, "resources/read").len(),
        requests(&unprepared_log, "resources/read").len(),
    );
    assert!(
        unlisted_read == unavailable
            && unlisted_call == unavailable
            && unprepared_call == unavailable
            && reads == (1, 0),
        "a bound server without preparation is refused before transport; \
         fixed list without it: read {unlisted_read:?}, call {unlisted_call:?}; \
         no provider: call {unprepared_call:?}; resources/read requests that \
         reached the servers (launched, unprepared): {reads:?}"
    );
    assert_eq!(requests(&log, "tools/call").len(), 1);
    assert!(requests(&unprepared_log, "tools/call").is_empty());
    for log in [&log, &unprepared_log] {
        assert!(
            !std::fs::read_to_string(log)
                .unwrap_or_default()
                .contains(CONNECTOR_CREDENTIAL_META_KEY),
            "no credential reached the server"
        );
    }
    assert_eq!(store.accesses(), consulted);
    unprepared.shutdown().await;
    router.shutdown().await;

    // A server sharing only the trusted installation's name is refused too.
    let impostor_log = temp.path().join("impostor.log");
    let (router, record) =
        launched(apps_server("apps", &impostor_log), &impostor_log, &session).await;
    let router = router.with_call_context_provider(Arc::new(ConnectorMcpCallContextProvider::new(
        connector_owner(&runtime, &store),
        vec![installation],
    )));
    assert_eq!(
        read_view(&router, &session, &record, OTHER_URI).await.err(),
        Some(ToolError::access_denied("display")),
        "a name-only match of a trusted installation is refused"
    );
    assert_eq!(requests(&impostor_log, "resources/read").len(), 1);
    assert_eq!(store.accesses(), consulted);
    router.shutdown().await;
}

/// Work authority whose preparation is counted and refused.
#[derive(Default)]
struct CountingWork {
    prepared: AtomicUsize,
}

impl meerkat_core::authorization::WorkAuthorization for CountingWork {
    fn prepare(
        &self,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<
        Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
        meerkat_core::OperationAuthorizationError,
    > {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}

/// The stock provider has no native authority coordinates for an MCP
/// resource (neither a server name nor a connector slot is one), so a
/// governed view read stays unavailable: no resource authority is invented,
/// nothing is prepared and nothing is sent.
#[cfg(unix)]
#[tokio::test]
async fn a_governed_view_read_is_not_given_a_fabricated_resource_authority() {
    let temp = tempfile::TempDir::new().unwrap();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let store = Arc::new(CountingTokenStore::default());
    let session = SessionId::new();
    let log = temp.path().join("apps.log");
    let (router, record) = launched(apps_server("apps", &log), &log, &session).await;
    let router = router.with_call_context_provider(Arc::new(
        ConnectorMcpCallContextProvider::live(connector_owner(&runtime, &store)),
    ));
    let work = Arc::new(CountingWork::default());
    let governed = dispatch_context(&session).with_work_authorization(Some(
        meerkat_core::authorization::WorkAuthorizationContext::new(
            Arc::clone(&work) as Arc<dyn meerkat_core::authorization::WorkAuthorization>,
            meerkat_core::exact_operation::OperationExecutionScope::Domain,
        ),
    ));
    let refused = read_view_in(&router, governed, &session, &record, OTHER_URI).await;
    assert_eq!(
        refused.err(),
        Some(ToolError::unavailable(
            "display",
            meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
        ))
    );
    assert_eq!(
        work.prepared.load(Ordering::SeqCst),
        0,
        "no authority was prepared"
    );
    assert_eq!(
        requests(&log, "resources/read").len(),
        1,
        "nothing more was read"
    );
    router.shutdown().await;
}

/// A scripted model: its first request calls `display`; every later one
/// answers in text.
#[cfg(feature = "session-store")]
#[derive(Default)]
struct LaunchingModel {
    requests: AtomicUsize,
}

#[cfg(feature = "session-store")]
#[async_trait]
impl meerkat_client::LlmClient for LaunchingModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>>
                + Send
                + 'a,
        >,
    > {
        use meerkat_client::{LlmDoneOutcome, LlmEvent};
        let usage = meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::Anthropic,
            &request.model,
            Default::default(),
        );
        let events = if self.requests.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![
                LlmEvent::ToolCallComplete {
                    id: LAUNCH.into(),
                    name: "display".into(),
                    args: json!({}),
                    meta: None,
                },
                LlmEvent::UsageUpdate { usage },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                },
            ]
        } else {
            vec![
                LlmEvent::TextDelta {
                    delta: "shown".into(),
                    meta: None,
                },
                LlmEvent::UsageUpdate { usage },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                },
            ]
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// The Core end of the Console's MCP App routes, through the stock
/// runtime-backed composition: a session built by the facade's runtime-backed
/// service (which installs the connector owner, so its router carries the
/// stock live provider) runs a turn whose model calls `display`. The
/// session's tool application observations (what the Console reads as the
/// original invocation) retain the view read at tool-call time, and a fresh
/// read through `SessionService::tool_application` (what the Console's
/// `read-resource` route executes) returns another view of the app. Both
/// reads reach the server with no connector credential, and the credential
/// owner is never consulted for them.
#[cfg(all(unix, feature = "session-store"))]
#[tokio::test]
async fn a_console_view_read_reaches_the_server_through_the_runtime_backed_composition() {
    use meerkat_core::service::{
        CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
        SessionService as _,
    };
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, PromptInput};

    let temp = tempfile::TempDir::new().unwrap();
    let log = temp.path().join("apps.log");
    let store = Arc::new(CountingTokenStore::default());
    let factory = crate::AgentFactory::new(temp.path().join("sessions"))
        .builtins(false)
        .mcp_apps(true)
        .with_provider_auth_persistence(
            meerkat_providers::auth_store::ProviderAuthPersistence::new(
                Arc::clone(&store) as Arc<dyn meerkat_core::auth::TokenStore>,
                Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
            ),
        );
    let mut builder = crate::FactoryAgentBuilder::new(factory, meerkat_core::Config::default());
    builder.default_llm_client = Some(Arc::new(LaunchingModel::default()));
    builder.wait_for_mcp = true;
    let persistence = crate::PersistenceBundle::new(
        Arc::new(meerkat_store::JsonlStore::new(temp.path().join("jsonl")))
            as Arc<dyn crate::SessionStore>,
        Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
                temp.path().join("runtime.sqlite3"),
            )
            .unwrap(),
        ),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    )
    .unwrap();
    let (service, machine) =
        crate::surface::build_runtime_backed_service_with_default_reconfigure_host(
            builder,
            4,
            persistence,
            temp.path().join("config_state.json"),
        )
        .unwrap();
    let session = meerkat_core::Session::new();
    let session_id = session.id().clone();
    Box::pin(crate::surface::materialize_session(
        &service,
        &machine,
        session,
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".into(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: crate::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                mcp_servers: vec![apps_server("apps", &log)],
                ..Default::default()
            }),
            labels: None,
        },
        {
            let service = Arc::clone(&service);
            let machine = Arc::clone(&machine);
            move |session_id| {
                crate::surface::default_persistent_executor(service, machine, session_id)
            }
        },
    ))
    .await
    .expect("the session materializes");
    let (_, completion) = machine
        .accept_input_with_completion(
            &session_id,
            Input::Prompt(PromptInput::new("show the app", None)),
        )
        .await
        .expect("the runtime accepts the prompt");
    let outcome = tokio::time::timeout(LIMIT, completion.expect("a completion").wait())
        .await
        .expect("the turn settles")
        .expect("the completion resolves");
    assert!(
        matches!(outcome, CompletionOutcome::Completed(ref run) if run.text == "shown"),
        "{outcome:?}"
    );
    assert_eq!(
        requests(&log, "tools/call").len(),
        1,
        "the model's display call reached the server"
    );
    let consulted = store.accesses();

    let observations = service
        .read_tool_application_observations(&session_id)
        .await
        .expect("the session's tool application observations");
    let record = observations
        .iter()
        .find(|observation| observation.tool_call_id == LAUNCH)
        .and_then(|observation| observation.host_metadata.get(MCP_APPS_EXTENSION))
        .cloned()
        .expect("the launch's MCP Apps host record");
    let control = meerkat_core::ToolApplicationControlRequest::from_trusted_ingress(
        session_id.clone(),
        read_request(OTHER_URI),
        Arc::new(Viewer),
    )
    .expect("trusted viewer ingress");
    let fresh = tokio::time::timeout(LIMIT, Arc::clone(&service).tool_application(control))
        .await
        .expect("the read settles");
    let reads = requests(&log, "resources/read");
    assert!(
        retained_view(&record).as_deref() == Some(APP_URI)
            && fresh
                .as_ref()
                .is_ok_and(|view| view["contents"][0]["uri"] == OTHER_URI),
        "the session reads its app's views; tool-call-time read: {:?}, fresh read: {fresh:?}, \
         resources/read requests that reached the server: {}",
        retained_view(&record),
        reads.len()
    );
    assert_eq!(reads.len(), 2, "one request per view: {reads:?}");
    assert!(
        reads
            .iter()
            .all(|line| !line.contains(CONNECTOR_CREDENTIAL_META_KEY)),
        "a read of an unbound server carries no connector credential: {reads:?}"
    );
    assert_eq!(
        store.accesses(),
        consulted,
        "the connector credential owner is never consulted"
    );
    service.try_shutdown().await.expect("shutdown");
}
