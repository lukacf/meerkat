//! Rows for personal accounts on MCP servers.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};
use meerkat_core::types::ToolCallView;
use meerkat_core::{
    AgentToolDispatcher, McpServerConfig, PrincipalKind, PrincipalRef, ToolDispatchContext,
    ToolError, TrustDomainId, TurnRequester,
};
use meerkat_mcp::McpAuthResolver;

use super::gate::PersonalAccountGate;
use super::*;

const ISSUER: &str = "https://login.example.test";

/// A personal server's configured accounts.
fn personal(names: &[&str]) -> McpAccountsConfig {
    McpAccountsConfig {
        servers: names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    McpServerAccounts {
                        accounts: McpAccountMode::Personal,
                    },
                )
            })
            .collect(),
    }
}

fn person(subject: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        subject,
        TrustDomainId::new(format!("oidc-issuer:{ISSUER}")).expect("domain"),
    )
    .expect("principal")
}

fn direct(subject: &str) -> ToolDispatchContext {
    ToolDispatchContext::default().with_requester(Some(TurnRequester::direct(person(subject))))
}

fn carried(subject: &str) -> ToolDispatchContext {
    ToolDispatchContext::default().with_requester(Some(TurnRequester::carried(person(subject))))
}

fn owner(subject: &str) -> PrivateOwner {
    PrivateOwner {
        key: private_owner_key(ISSUER, subject),
        access_principal: format!("{subject}@example.test"),
        opened_at: SystemTime::now(),
    }
}

/// What the `notes` endpoint saw.
#[derive(Default)]
struct Endpoint {
    calls: AtomicUsize,
}

impl Endpoint {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

/// A streamable-HTTP MCP endpoint serving one tool, `notes_search`, to any
/// caller, without OAuth. It counts tool calls.
async fn endpoint() -> (String, Arc<Endpoint>) {
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    async fn mcp(
        State(seen): State<Arc<Endpoint>>,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let request: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        let Some(id) = request.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let result = match request["method"].as_str() {
            Some("initialize") => serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "notes", "version": "0.1.0" },
            }),
            Some("tools/list") => serde_json::json!({
                "tools": [{
                    "name": "notes_search",
                    "description": "Search notes",
                    "inputSchema": { "type": "object", "properties": {} },
                }]
            }),
            Some("tools/call") => {
                seen.calls.fetch_add(1, Ordering::SeqCst);
                serde_json::json!({ "content": [{ "type": "text", "text": "notes result" }] })
            }
            _ => serde_json::json!({}),
        };
        axum::Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .into_response()
    }
    let seen = Arc::new(Endpoint::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}/mcp", listener.local_addr().expect("addr"));
    let app = axum::Router::new()
        .route("/mcp", axum::routing::post(mcp))
        .with_state(Arc::clone(&seen));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (url, seen)
}

/// A plain streamable HTTP server: connects without OAuth.
fn http_server(name: &str, url: &str) -> McpServerConfig {
    McpServerConfig::streamable_http(name, url.to_owned(), HashMap::new())
}

/// A streamable HTTP server whose account the person proves at sign-in: the
/// shape a personal server has.
fn personal_http_server(name: &str, url: &str) -> McpServerConfig {
    let mut server = http_server(name, url);
    if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut server.transport {
        http.oauth_account_selection =
            Some(meerkat_core::mcp_config::McpOAuthAccountSelection::Discover);
    }
    server
}

fn stdio_server(name: &str) -> McpServerConfig {
    McpServerConfig::stdio(name, "notes-server".to_string(), Vec::new(), HashMap::new())
}

/// A member's MCP adapter connected to the `notes` server at `url`.
async fn member_adapter(url: &str) -> Arc<meerkat_mcp::McpRouterAdapter> {
    let mut router = meerkat_mcp::McpRouter::new_with_surface_handle_and_stdio_profile(
        Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
    );
    router
        .stage_add(http_server("notes", url))
        .expect("stage add");
    router.apply_staged().await.expect("apply staged add");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let _ = router.take_external_updates();
        if router.active_server_names() == vec!["notes".to_string()] {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the notes server never connected"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Arc::new(meerkat_mcp::McpRouterAdapter::new(router))
}

fn gate(
    adapter: &Arc<meerkat_mcp::McpRouterAdapter>,
    accounts: &Arc<PersonalMcpAccounts>,
    owner: Option<PrivateOwner>,
) -> Arc<PersonalAccountGate> {
    Arc::new(PersonalAccountGate::new(
        Arc::clone(adapter) as Arc<dyn AgentToolDispatcher>,
        Arc::clone(accounts),
        owner,
    ))
}

fn notes_call(args: &serde_json::value::RawValue) -> ToolCallView<'_> {
    ToolCallView {
        id: "call-1",
        name: "notes_search",
        args,
    }
}

/// One `notes_search` call through `gate`, resolved and dispatched as the
/// agent loop does.
async fn call_notes(
    gate: &Arc<PersonalAccountGate>,
    context: &ToolDispatchContext,
) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
    let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("args");
    let call = notes_call(&args);
    let plan = gate
        .resolve_execution_plan(
            call,
            context,
            &crate::tool_application_test_support::resolution(),
        )
        .expect("a resolved plan");
    gate.dispatch_resolved_with_context(call, context, &plan)
        .await
}

/// The policy denial code a refused use ended with.
fn denial<T>(result: &Result<T, ToolError>) -> Option<String> {
    match result {
        Err(ToolError::PolicyDenied { denial }) => Some(denial.code.clone()),
        _ => None,
    }
}

fn request(
    labels: BTreeMap<String, String>,
    servers: Vec<McpServerConfig>,
    fork_source: Option<meerkat_core::ForkBuildSource>,
) -> CreateSessionRequest {
    request_on(labels, servers, fork_source, meerkat_core::Session::new())
}

fn request_on(
    labels: BTreeMap<String, String>,
    servers: Vec<McpServerConfig>,
    fork_source: Option<meerkat_core::ForkBuildSource>,
    session: meerkat_core::Session,
) -> CreateSessionRequest {
    let build = SessionBuildOptions {
        mcp_servers: servers,
        fork_source,
        resume_session: Some(session),
        ..Default::default()
    };
    CreateSessionRequest {
        model: "m".to_string(),
        prompt: meerkat_core::ContentInput::Text("noop".to_string()),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        build: Some(build),
        labels: Some(labels),
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
        injected_context: Vec::new(),
    }
}

fn server_names(request: &CreateSessionRequest) -> Vec<String> {
    request
        .build
        .as_ref()
        .map(|build| {
            build
                .mcp_servers
                .iter()
                .map(|server| server.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn session_of(request: &CreateSessionRequest) -> meerkat_core::types::SessionId {
    request
        .build
        .as_ref()
        .and_then(|build| build.resume_session.as_ref())
        .map(|session| session.id().clone())
        .expect("session")
}

/// The credential source a build carries for its owner, decoded.
fn carried_source(request: &CreateSessionRequest) -> Option<Arc<dyn McpAuthResolver>> {
    request
        .build
        .as_ref()
        .and_then(|build| build.mcp_auth_resolver_override.as_ref())
        .and_then(meerkat::decode_mcp_auth_resolver_override_from_service)
}

fn fork_source_of(source: &str) -> meerkat_core::ForkBuildSource {
    meerkat_core::ForkBuildSource::new(
        meerkat_core::MobMemberBinding {
            mob_id: "team".to_string(),
            role: "assistant".to_string(),
            member: source.to_string(),
        },
        meerkat_core::SessionId::new(),
    )
}

/// The labels and fork source of a console side chat of `source` that
/// `subject` opened on a console with sign-in.
fn private_side_chat(
    source: &str,
    subject: &str,
) -> (BTreeMap<String, String>, meerkat_core::ForkBuildSource) {
    let labels = BTreeMap::from([
        (
            crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
            source.to_string(),
        ),
        (
            crate::side_chat::SIDE_CHAT_CREATOR_LABEL.to_string(),
            format!("{subject}@example.test"),
        ),
        (
            crate::access::PRIVATE_TO_LABEL.to_string(),
            private_owner_key(ISSUER, subject),
        ),
    ]);
    (labels, fork_source_of(source))
}

/// A composition whose `notes` server is personal, with the owner check
/// installed on its member factories.
fn accounts() -> Arc<PersonalMcpAccounts> {
    let accounts = PersonalMcpAccounts::new(personal(&["notes"]));
    let _ = accounts.composer(None);
    accounts
}

/// Where a test composition keeps its people's accounts: a credential store
/// in a temporary directory and a runtime's credential authority, kept alive
/// with it.
struct AccountStore {
    _dir: tempfile::TempDir,
    _runtime: meerkat_runtime::MeerkatMachine,
}

fn keep_accounts(
    accounts: &PersonalMcpAccounts,
    system: Option<Arc<dyn McpAuthResolver>>,
) -> AccountStore {
    let dir = tempfile::tempdir().expect("dir");
    let persistence = meerkat::TokenStoreBackend::File {
        root: dir.path().join("credentials"),
    }
    .open_with_refresh_authority()
    .expect("credential store");
    let runtime = crate::mob_handle_runtime::persistent_runtime_machine(
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
        None,
    )
    .expect("runtime machine");
    accounts.bind_credentials(PersonalAccountCredentials::new(
        meerkat::HostAuthService::new(persistence, runtime.provider_auth_runtime_authority()),
        meerkat_core::RealmId::parse("meta").expect("realm"),
        system,
    ));
    AccountStore {
        _dir: dir,
        _runtime: runtime,
    }
}

/// A credential source that counts what it is asked and holds a system
/// credential for every server.
#[derive(Default)]
struct CountingSource {
    reads: AtomicUsize,
    logins: AtomicUsize,
}

impl CountingSource {
    fn asked(&self) -> usize {
        self.reads.load(Ordering::SeqCst) + self.logins.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl McpAuthResolver for CountingSource {
    async fn stored_bearer_token(
        &self,
        _target: &meerkat::McpServerIdentity,
    ) -> Result<Option<String>, meerkat::McpOAuthError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some("system-token".to_string()))
    }

    async fn interactive_login(
        &self,
        _target: &meerkat::McpServerIdentity,
        _www_authenticate: Option<&str>,
    ) -> Result<String, meerkat::McpOAuthError> {
        self.logins.fetch_add(1, Ordering::SeqCst);
        Ok("system-token".to_string())
    }
}

fn identity(server: &McpServerConfig) -> meerkat::McpServerIdentity {
    meerkat::McpServerIdentity::from_config(server)
        .expect("identity")
        .in_realm(meerkat_core::RealmId::parse("meta").expect("realm"))
}

#[test]
fn mcp_accounts_config_refuses_unknown_modes_and_keys() {
    let config = McpAccountsConfig::from_toml_str(
        "[servers.notes]\naccounts = \"personal\"\n\n[servers.wiki]\naccounts = \"system\"\n",
    )
    .expect("a valid configuration");
    assert!(config.is_personal("notes"));
    assert!(!config.is_personal("wiki"));
    assert!(
        !config.is_personal("unnamed"),
        "an unnamed server is system"
    );
    assert_eq!(config.personal_servers().collect::<Vec<_>>(), vec!["notes"]);

    for invalid in [
        "[servers.notes]\naccounts = \"team\"\n",
        "[servers.notes]\naccounts = \"personal\"\nowner = \"someone\"\n",
        "[servers.notes]\n",
        "personal = [\"notes\"]\n",
        "[servers.\"\"]\naccounts = \"personal\"\n",
        "[servers.\"no spaces\"]\naccounts = \"personal\"\n",
    ] {
        assert!(
            McpAccountsConfig::from_toml_str(invalid).is_err(),
            "refused: {invalid}"
        );
    }

    let json = McpAccountsConfig::from_json_value(serde_json::json!({
        "servers": { "notes": { "accounts": "personal" } }
    }))
    .expect("a valid JSON configuration");
    assert!(json.is_personal("notes"));
    assert!(
        McpAccountsConfig::from_json_value(serde_json::json!({
            "servers": { "notes": { "accounts": "everyone" } }
        }))
        .is_err()
    );
}

#[test]
fn private_owner_key_of_a_principal_is_the_whoami_caller_scope() {
    let key = private_owner_key(ISSUER, "alice");
    assert!(key.starts_with("console-caller:v1:"), "{key}");
    assert_eq!(
        private_owner_key_of_principal(&person("alice")).as_deref(),
        Some(key.as_str()),
        "a console person's qualified principal owns what their caller scope owns"
    );
    assert_ne!(
        private_owner_key_of_principal(&person("bob")).as_deref(),
        Some(key.as_str())
    );
    let service = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "alice",
        TrustDomainId::new(format!("oidc-issuer:{ISSUER}")).expect("domain"),
    )
    .expect("principal");
    assert_eq!(private_owner_key_of_principal(&service), None);
    let other_domain = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "alice",
        TrustDomainId::new("chat-ingress:team").expect("domain"),
    )
    .expect("principal");
    assert_eq!(private_owner_key_of_principal(&other_domain), None);
    let unqualified = PrincipalRef::new(PrincipalKind::Human, "alice").expect("principal");
    assert_eq!(private_owner_key_of_principal(&unqualified), None);
}

/// R2. A shared agent's build gets no personal server (its system server
/// stays), and a call that still reaches a personal server in a shared
/// member is refused before the router: nothing reaches the server and no
/// other account is tried.
#[tokio::test]
async fn personal_account_refused_in_shared_thread() {
    let accounts = accounts();
    let _store = keep_accounts(&accounts, None);
    let (url, notes) = endpoint().await;
    let mut shared = request(
        BTreeMap::from([("team".to_string(), "review".to_string())]),
        vec![
            personal_http_server("notes", &url),
            http_server("wiki", &url),
        ],
        None,
    );
    accounts.classify_build(&mut shared);
    assert_eq!(
        server_names(&shared),
        vec!["wiki".to_string()],
        "a shared member's build keeps only its system servers"
    );
    assert!(
        carried_source(&shared).is_none(),
        "and carries no owner's source"
    );

    let adapter = member_adapter(&url).await;
    let refused = call_notes(&gate(&adapter, &accounts, None), &direct("alice")).await;
    assert_eq!(
        denial(&refused).as_deref(),
        Some("personal_account_shared_thread"),
        "{refused:?}"
    );
    assert_eq!(notes.calls(), 0, "nothing reached the server");
    adapter.shutdown().await;
}

/// R3. In a member private to Alice, a run for anyone else (an access admin
/// included) is refused; Alice's own run reaches the server.
#[tokio::test]
async fn admin_in_private_member_cannot_use_owners_account() {
    let accounts = accounts();
    let (url, notes) = endpoint().await;
    let adapter = member_adapter(&url).await;
    let alices = gate(&adapter, &accounts, Some(owner("alice")));

    let refused = call_notes(&alices, &direct("admin")).await;
    assert_eq!(
        denial(&refused).as_deref(),
        Some("personal_account_not_owner"),
        "{refused:?}"
    );
    assert_eq!(notes.calls(), 0);

    let served = call_notes(&alices, &direct("alice")).await;
    assert!(served.is_ok(), "{served:?}");
    assert_eq!(notes.calls(), 1, "the owner's run reached the server");
    adapter.shutdown().await;
}

/// R5. A requester carried across a peer hop never selects an account: in
/// the owner's own member it is not the owner, in a shared member the
/// thread is shared. A run without a requester acts for nobody.
#[tokio::test]
async fn carried_requester_never_selects_an_account() {
    let accounts = accounts();
    let (url, notes) = endpoint().await;
    let adapter = member_adapter(&url).await;

    let into_owners = call_notes(
        &gate(&adapter, &accounts, Some(owner("alice"))),
        &carried("alice"),
    )
    .await;
    assert_eq!(
        denial(&into_owners).as_deref(),
        Some("personal_account_not_owner"),
        "{into_owners:?}"
    );
    let into_shared = call_notes(&gate(&adapter, &accounts, None), &carried("alice")).await;
    assert_eq!(
        denial(&into_shared).as_deref(),
        Some("personal_account_shared_thread"),
        "{into_shared:?}"
    );
    let nobody = call_notes(
        &gate(&adapter, &accounts, Some(owner("alice"))),
        &ToolDispatchContext::default(),
    )
    .await;
    assert_eq!(
        denial(&nobody).as_deref(),
        Some("personal_account_not_owner"),
        "{nobody:?}"
    );
    assert_eq!(notes.calls(), 0, "nothing reached the server");
    adapter.shutdown().await;
}

/// R6. Approved work runs in an approval branch, which is private to no
/// person: its build gets no personal server even when it carries every
/// label of its owner's private side chat and a fork source the side chat
/// marker names, and a call there is refused as a shared thread.
#[tokio::test]
async fn approved_work_in_an_approval_branch_refuses_personal_servers() {
    let accounts = accounts();
    let _store = keep_accounts(&accounts, None);
    let (url, notes) = endpoint().await;
    let (mut labels, fork_source) = private_side_chat("assistant", "alice");
    labels.insert(APPROVAL_BRANCH_LABEL.to_string(), "decision-1".to_string());
    let mut branch = request(
        labels,
        vec![personal_http_server("notes", &url)],
        Some(fork_source),
    );
    let session = session_of(&branch);
    accounts.classify_build(&mut branch);
    assert!(
        server_names(&branch).is_empty(),
        "{:?}",
        server_names(&branch)
    );
    assert!(carried_source(&branch).is_none());
    assert_eq!(
        accounts.take_plan(&session),
        None,
        "no owner for the branch"
    );

    let adapter = member_adapter(&url).await;
    let refused = call_notes(&gate(&adapter, &accounts, None), &direct("alice")).await;
    assert_eq!(
        denial(&refused).as_deref(),
        Some("personal_account_shared_thread"),
        "{refused:?}"
    );
    assert_eq!(notes.calls(), 0);
    adapter.shutdown().await;
}

/// R7 (shared target). A schedule that fires into a shared agent carries
/// its sender as the run's requester; the shared agent's build has no
/// personal server, and a call there is refused.
#[tokio::test]
async fn schedule_into_shared_agent_refuses() {
    let accounts = accounts();
    let _store = keep_accounts(&accounts, None);
    let (url, notes) = endpoint().await;
    let mut target = request(
        BTreeMap::new(),
        vec![personal_http_server("notes", &url)],
        None,
    );
    accounts.classify_build(&mut target);
    assert!(
        server_names(&target).is_empty(),
        "{:?}",
        server_names(&target)
    );

    let adapter = member_adapter(&url).await;
    let fired = call_notes(&gate(&adapter, &accounts, None), &direct("alice")).await;
    assert_eq!(
        denial(&fired).as_deref(),
        Some("personal_account_shared_thread"),
        "{fired:?}"
    );
    assert_eq!(notes.calls(), 0);
    adapter.shutdown().await;
}

/// R11 (Meta). A connector-bound or stdio server cannot be personal, nor
/// can a definition with a fixed account, a static Authorization header or
/// no account the person proves: the configuration is refused, and a build
/// withholds such a server even in its owner's private member.
#[tokio::test]
async fn personal_mode_refused_for_connector_servers() {
    let config = personal(&["notes"]);
    let url = "https://notes.example.test/mcp";
    let mut connector_bound = stdio_server("notes");
    connector_bound.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
        realm_id: "team".to_string(),
        slot_id: "notes".to_string(),
        issuer: "https://issuer.example.test".to_string(),
        client: "client".to_string(),
        resource: "https://notes.example.test".to_string(),
        strategy_id: "oidc-userinfo-v1".to_string(),
        account: "shared".to_string(),
        operations: BTreeMap::new(),
    });
    let mut fixed = http_server("notes", url);
    if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut fixed.transport {
        http.oauth_account = Some("operator".to_string());
    }
    let static_header = McpServerConfig::streamable_http(
        "notes",
        url.to_string(),
        HashMap::from([("Authorization".to_string(), "Bearer shared".to_string())]),
    );
    for (definition, reason) in [
        (connector_bound, UnsupportedPersonalServer::ConnectorBound),
        (stdio_server("notes"), UnsupportedPersonalServer::Stdio),
        (fixed, UnsupportedPersonalServer::FixedAccount),
        (
            static_header,
            UnsupportedPersonalServer::StaticAuthorization,
        ),
        (
            http_server("notes", url),
            UnsupportedPersonalServer::NoAccountProof,
        ),
    ] {
        let refused = config.check_definitions([&definition]);
        assert_eq!(
            refused,
            Err(PersonalAccountRefusal::Unsupported {
                server: "notes".to_string(),
                reason,
            })
        );
        assert_eq!(
            refused.err().map(|refusal| refusal.kind()),
            Some("personal_accounts_unsupported")
        );
        let accounts = PersonalMcpAccounts::new(config.clone());
        let _ = accounts.composer(None);
        let _store = keep_accounts(&accounts, None);
        let (labels, fork_source) = private_side_chat("assistant", "alice");
        let mut build = request(labels, vec![definition], Some(fork_source));
        accounts.classify_build(&mut build);
        assert!(server_names(&build).is_empty(), "withheld at the build");
    }
    assert_eq!(
        config.check_definitions([&personal_http_server("notes", url)]),
        Ok(()),
        "a streamable HTTP server the person signs in to can be personal"
    );
    assert_eq!(
        config.check_definitions([&stdio_server("wiki")]),
        Ok(()),
        "a system server may be anything"
    );
}

/// The member a console side chat fork opened for Alice keeps its personal
/// server, carries Alice's credential source, and its MCP adapter
/// composition takes Alice as owner. A label the fork did not write selects
/// no one: a private-owner label on a member that is not a side chat, or a
/// side chat marker that names another source than its typed fork source.
#[tokio::test]
async fn a_forged_private_owner_label_selects_no_owner() {
    let accounts = accounts();
    let _store = keep_accounts(&accounts, None);
    let url = "https://notes.example.test/mcp";
    let (labels, fork_source) = private_side_chat("assistant", "alice");

    let mut genuine = request(
        labels.clone(),
        vec![personal_http_server("notes", url)],
        Some(fork_source),
    );
    let session = session_of(&genuine);
    accounts.classify_build(&mut genuine);
    assert_eq!(server_names(&genuine), vec!["notes".to_string()]);
    assert!(carried_source(&genuine).is_some(), "the owner's source");
    assert_eq!(
        accounts.take_plan(&session).map(|owner| owner.key),
        Some(private_owner_key(ISSUER, "alice")),
        "the composition takes the owner the build established"
    );

    let mut label_only = request(
        BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            private_owner_key(ISSUER, "alice"),
        )]),
        vec![personal_http_server("notes", url)],
        None,
    );
    accounts.classify_build(&mut label_only);
    assert!(server_names(&label_only).is_empty());
    assert!(carried_source(&label_only).is_none());

    let mut other_source = request(
        labels,
        vec![personal_http_server("notes", url)],
        Some(fork_source_of("helper")),
    );
    accounts.classify_build(&mut other_source);
    assert!(server_names(&other_source).is_empty());
    assert!(carried_source(&other_source).is_none());
}

/// A build keeps a personal server only when the owner check is installed
/// on its member factory and the composition keeps its people's accounts.
#[tokio::test]
async fn a_build_without_the_owner_check_keeps_no_personal_server() {
    let url = "https://notes.example.test/mcp";
    let (labels, fork_source) = private_side_chat("assistant", "alice");

    let unchecked = PersonalMcpAccounts::new(personal(&["notes"]));
    let _store = keep_accounts(&unchecked, None);
    let mut build = request(
        labels.clone(),
        vec![personal_http_server("notes", url)],
        Some(fork_source.clone()),
    );
    unchecked.classify_build(&mut build);
    assert!(server_names(&build).is_empty());
    assert!(carried_source(&build).is_none());

    let unkept = accounts();
    let mut build = request(
        labels,
        vec![personal_http_server("notes", url)],
        Some(fork_source),
    );
    unkept.classify_build(&mut build);
    assert!(
        server_names(&build).is_empty(),
        "no account store, no server"
    );
    assert!(carried_source(&build).is_none());
}

/// Every build of a member private to Alice carries her credential source
/// again, which no build keeps from an earlier one: the fork that seats the
/// side chat, each resume of its session (a warm revival, or the wake of a
/// dormant member, which run the same create path), and a build on a fresh
/// session (a respawn). The owner each keeps for the owner check is opened
/// when the member's session was created, not when it was rebuilt.
#[tokio::test]
async fn every_build_of_a_private_member_carries_its_owners_source() {
    let accounts = accounts();
    let system = Arc::new(CountingSource::default());
    let _store = keep_accounts(
        &accounts,
        Some(Arc::clone(&system) as Arc<dyn McpAuthResolver>),
    );
    let slot = build::PersonalAccountsSlot::default();
    slot.fill(Arc::clone(&accounts));
    let hook = slot.after_hook(Arc::new(|_req: &mut CreateSessionRequest| {
        Box::pin(async { Ok(()) })
    }));
    let url = "https://notes.example.test/mcp";
    let (labels, fork_source) = private_side_chat("assistant", "alice");
    let session = meerkat_core::Session::new();
    let created = session.created_at();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let seat = request_on(
        labels.clone(),
        vec![personal_http_server("notes", url)],
        Some(fork_source.clone()),
        session.clone(),
    );
    let mut resume = request_on(
        labels.clone(),
        vec![personal_http_server("notes", url)],
        Some(fork_source.clone()),
        session,
    );
    if let Some(build) = resume.build.as_mut() {
        build.resume_session_intent = meerkat_core::SessionBuildIntent::Resume;
    }
    let respawn = request(
        labels,
        vec![personal_http_server("notes", url)],
        Some(fork_source),
    );
    for (kind, mut build) in [("seat", seat), ("resume", resume), ("respawn", respawn)] {
        let session = session_of(&build);
        hook(&mut build).await.expect("the hook runs");
        assert_eq!(server_names(&build), vec!["notes".to_string()], "{kind}");
        let source = carried_source(&build).unwrap_or_else(|| panic!("{kind}: the owner's source"));
        let plan = accounts
            .take_plan(&session)
            .unwrap_or_else(|| panic!("{kind}: the owner kept"));
        assert_eq!(plan.key, private_owner_key(ISSUER, "alice"), "{kind}");
        if kind != "respawn" {
            assert_eq!(plan.opened_at, created, "{kind}: opened with its session");
        }
        // The owner's source never asks the system source for the
        // personal server, and asks it for every other.
        let before = system.asked();
        let _ = source
            .stored_bearer_token(&identity(&personal_http_server("notes", url)))
            .await;
        assert_eq!(
            system.asked(),
            before,
            "{kind}: personal target stays the owner's"
        );
        let wiki = source
            .stored_bearer_token(&identity(&http_server("wiki", url)))
            .await
            .expect("a read");
        assert_eq!(wiki.as_deref(), Some("system-token"), "{kind}");
        assert_eq!(system.asked(), before + 1, "{kind}");
    }
}

/// A member private to its owner but built without its owner's source (a
/// build that did not pass the classification) gets no shared credential:
/// the member factory's own source holds nothing for a personal server, so
/// its connection awaits the owner's sign-in, and the owner check refuses
/// every call, since no owner was established for that build.
#[tokio::test]
async fn a_private_member_built_without_its_owners_source_refuses_personal_servers() {
    let accounts = accounts();
    let system = Arc::new(CountingSource::default());
    let source = accounts.member_credential_source(Arc::clone(&system) as Arc<dyn McpAuthResolver>);
    let notes = identity(&personal_http_server(
        "notes",
        "https://notes.example.test/mcp",
    ));
    assert_eq!(
        source.stored_bearer_token(&notes).await.expect("a read"),
        None,
        "no system credential serves a personal server"
    );
    assert!(matches!(
        source.interactive_login(&notes, None).await,
        Err(meerkat::McpOAuthError::HumanAuthorizationRequired { .. })
    ));
    assert_eq!(system.asked(), 0, "the system source was never asked");
    let wiki = identity(&http_server("wiki", "https://wiki.example.test/mcp"));
    assert_eq!(
        source.stored_bearer_token(&wiki).await.expect("a read"),
        Some("system-token".to_string()),
        "a system server keeps the system credential"
    );

    let (url, endpoint) = endpoint().await;
    let adapter = member_adapter(&url).await;
    let composer = accounts.composer(None);
    let unplanned = composer
        .compose(&adapter, &meerkat_core::SessionId::new(), None)
        .expect("composed");
    let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("args");
    let refused = unplanned
        .dispatch_with_context(notes_call(&args), &direct("alice"))
        .await;
    assert_eq!(
        denial(&refused).as_deref(),
        Some("personal_account_shared_thread"),
        "{refused:?}"
    );
    assert_eq!(endpoint.calls(), 0);
    adapter.shutdown().await;
}

/// No call path to a personal server bypasses the owner check: a resolved
/// call, an unresolved call, a call without a dispatch context, and a tool
/// application (an MCP app's resource read) are each refused for a run that
/// does not act for the owner, and none reaches the server. The catalog
/// paths only read what the connection listed.
#[tokio::test]
async fn no_personal_server_call_path_bypasses_the_owner_check() {
    let accounts = accounts();
    let (url, notes) = endpoint().await;
    let adapter = member_adapter(&url).await;
    let alices = gate(&adapter, &accounts, Some(owner("alice")));
    let admin = direct("admin");

    let resolved = call_notes(&alices, &admin).await;
    assert_eq!(
        denial(&resolved).as_deref(),
        Some("personal_account_not_owner")
    );

    let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("args");
    let unresolved = alices
        .dispatch_with_context(notes_call(&args), &admin)
        .await;
    assert_eq!(
        denial(&unresolved).as_deref(),
        Some("personal_account_not_owner")
    );

    let bare = alices.dispatch(notes_call(&args)).await;
    assert_eq!(denial(&bare).as_deref(), Some("personal_account_not_owner"));

    let read = alices
        .resolve_tool_application(
            "notes_search",
            &meerkat_core::ToolApplicationRequest {
                tool_call_id: "call-1".to_string(),
                extension: meerkat_mcp::apps::MCP_APPS_EXTENSION.to_string(),
                operation: meerkat_core::ToolApplicationOperation::ReadResource {
                    uri: "ui://notes/view".to_string(),
                },
            },
            &serde_json::json!({
                "registration": { "server": "notes", "connection": "connection-1" },
            }),
            &admin,
        )
        .await;
    assert_eq!(denial(&read).as_deref(), Some("personal_account_not_owner"));

    assert!(
        alices
            .tools()
            .iter()
            .any(|tool| tool.name.as_str() == "notes_search"),
        "the catalog still lists the tool"
    );
    let _ = alices.tool_catalog();
    assert_eq!(notes.calls(), 0, "nothing reached the server");
    adapter.shutdown().await;
}

/// An access admin's revocation of the owner's sessions after the private
/// member was opened suspends the accounts there; one before it does not.
#[tokio::test]
async fn a_revoked_owner_s_accounts_are_suspended_in_earlier_private_members() {
    let accounts = accounts();
    let dir = tempfile::tempdir().expect("dir");
    let fences = crate::auth::session_fence::ConsoleSessionFences::open(dir.path().join("fences"))
        .expect("fences");
    accounts.bind_session_fences(fences.clone());
    let (url, notes) = endpoint().await;
    let adapter = member_adapter(&url).await;
    let opened = SystemTime::now() - Duration::from_mins(10);
    let earlier = PrivateOwner {
        opened_at: opened,
        ..owner("alice")
    };

    fences
        .revoke_sessions("alice@example.test", opened - Duration::from_mins(1))
        .expect("revoked before the member was opened");
    let served = call_notes(
        &gate(&adapter, &accounts, Some(earlier.clone())),
        &direct("alice"),
    )
    .await;
    assert!(served.is_ok(), "{served:?}");

    fences
        .revoke_sessions("alice@example.test", SystemTime::now())
        .expect("revoked after");
    let suspended = call_notes(&gate(&adapter, &accounts, Some(earlier)), &direct("alice")).await;
    assert_eq!(
        denial(&suspended).as_deref(),
        Some("personal_account_suspended"),
        "{suspended:?}"
    );
    let reopened = PrivateOwner {
        opened_at: SystemTime::now() + Duration::from_secs(5),
        ..owner("alice")
    };
    let fresh = call_notes(&gate(&adapter, &accounts, Some(reopened)), &direct("alice")).await;
    assert!(
        fresh.is_ok(),
        "a member opened after the revocation: {fresh:?}"
    );
    assert_eq!(
        notes.calls(),
        2,
        "the call between the revocations was not sent"
    );
    adapter.shutdown().await;
}

/// Each owner's personal accounts live in a credential realm of their own,
/// below the installation's realm: one token slot per owner and server,
/// never the installation's own slot.
#[tokio::test]
async fn each_owner_keeps_personal_accounts_in_a_realm_of_their_own() {
    let accounts = accounts();
    let _store = keep_accounts(&accounts, None);
    let credentials = accounts.credentials().expect("kept");
    let alice = credentials
        .owner_realm(&private_owner_key(ISSUER, "alice"))
        .expect("alice's realm");
    let bob = credentials
        .owner_realm(&private_owner_key(ISSUER, "bob"))
        .expect("bob's realm");
    assert!(alice.as_str().starts_with("meta.personal."), "{alice}");
    assert_ne!(alice, bob);
    assert_eq!(
        credentials.owner_realm("console-caller:v1:not-a-digest"),
        None
    );
    assert_eq!(credentials.owner_realm("someone"), None);

    let notes = personal_http_server("notes", "https://notes.example.test/mcp");
    let installation = identity(&notes);
    let alices = installation.clone().in_realm(alice);
    let bobs = installation.clone().in_realm(bob);
    let keys = [
        installation.token_key().expect("key"),
        alices.token_key().expect("key"),
        bobs.token_key().expect("key"),
    ];
    assert_ne!(keys[0], keys[1]);
    assert_ne!(keys[1], keys[2]);
    assert_ne!(keys[0], keys[2]);
}

const NOTES_URL: &str = "https://notes.example.test/mcp";

/// A stock Meta composition of `definition` over `storage`, with the agent
/// mob tools installed as both gateways install them, whose `notes` server
/// is personal; kept alive with its working directory.
struct Hosted {
    runtime: crate::MobRuntime,
    _dir: tempfile::TempDir,
}

async fn hosted(
    definition: meerkat_mob::MobDefinition,
    storage: meerkat_mob::MobStorage,
) -> Hosted {
    let dir = tempfile::tempdir().expect("dir");
    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(crate::blob_store::Base64BlobStoreAdapter::new(Arc::new(
            crate::blob_store::ObjectStoreBlobStore::memory(),
        )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
            .expect("runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(dir.path())
            .builtins(false)
            .comms(true),
        meerkat::Config::default(),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        4,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    let spec = crate::MobBootstrapSpec::new(definition, storage, service)
        .with_session_runtime_adapter(machine)
        .expect("adopt the runtime owner")
        .with_runtime_delivery(crate::MobRuntimeDelivery::new(store, jobs))
        .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
        .expect("install the agent mob tools")
        .with_personal_mcp_accounts(accounts());
    let runtime = Box::pin(crate::MobRuntime::bootstrap(spec))
        .await
        .expect("bootstrap");
    Hosted { runtime, _dir: dir }
}

/// A spawn of `profile` as `member`, placed on another host.
fn placed(profile: &str, member: &str) -> meerkat_mob::SpawnMemberSpec {
    let mut spec = meerkat_mob::SpawnMemberSpec::new(profile, member);
    spec.placement = Some(meerkat_mob::machines::mob_machine::HostId::from(
        "12D3KooWRemoteHost",
    ));
    spec
}

/// The refusal a member placed on another host gets for the `notes` server.
fn hosted_elsewhere() -> PersonalAccountRefusal {
    PersonalAccountRefusal::Unsupported {
        server: "notes".to_string(),
        reason: UnsupportedPersonalServer::HostedElsewhere,
    }
}

/// Whether a placed spawn ended with that refusal: it stopped at the check
/// of the profile it resolved, before Core compiled the member's portable
/// spec or opened its host ladder, so nothing reached the other host. Past
/// that check the spawn ends otherwise: the compile refuses a personal
/// shape's account selection, and the host ladder refuses this placement.
fn is_hosted_refusal(result: &Result<meerkat_mob::SpawnResult, meerkat_mob::MobError>) -> bool {
    matches!(
        result,
        Err(meerkat_mob::MobError::WiringError(message))
            if *message == hosted_elsewhere().to_string()
    )
}

/// Assert `result` is that refusal, typed: kind
/// `personal_accounts_unsupported`, reason `HostedElsewhere`, server `notes`.
fn assert_hosted_refusal(
    result: &Result<meerkat_mob::SpawnResult, meerkat_mob::MobError>,
    what: &str,
) {
    let expected = hosted_elsewhere();
    assert_eq!(expected.kind(), "personal_accounts_unsupported");
    assert!(
        is_hosted_refusal(result),
        "{what}: expected `{expected}`, got {result:?}"
    );
}

/// The members a mob holds.
async fn members(handle: &meerkat_mob::MobHandle) -> Vec<String> {
    handle
        .list_members()
        .await
        .into_iter()
        .map(|entry| entry.agent_identity.to_string())
        .collect()
}

/// The `notes` server's name in each shape personal accounts do not take.
fn renamed_shapes() -> [(&'static str, McpServerConfig); 3] {
    let mut fixed = http_server("notes", NOTES_URL);
    if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut fixed.transport {
        http.oauth_account = Some("bob@example.test".to_string());
    }
    [
        ("no account selection", http_server("notes", NOTES_URL)),
        ("a fixed oauth_account", fixed),
        (
            "another URL",
            http_server("notes", "https://other.example.test/mcp"),
        ),
    ]
}

/// A member placed on another host is built there from a portable spec that
/// carries its profile's MCP servers, and that host would serve a personal
/// server with its own credentials: a placed spawn whose definition profile,
/// or whose own override profile, declares one is refused, typed, before
/// anything is compiled or sent. A placed spawn whose profile declares no
/// personal server is not refused for one.
#[tokio::test]
async fn hosted_member_refuses_personal_server() {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "team"

[profiles.assistant]
model = "gpt-5.5"

[[profiles.assistant.tools.mcp_servers]]
name = "notes"
url = "{NOTES_URL}"
oauth_account_selection = "discover"

[profiles.plain]
model = "gpt-5.5"
"#
    ))
    .expect("definition");
    let plain = definition
        .profiles
        .get(&meerkat_mob::ProfileName::from("plain"))
        .and_then(meerkat_mob::ProfileBinding::as_inline)
        .expect("the plain profile")
        .clone();
    let hosted = hosted(definition, meerkat_mob::MobStorage::in_memory()).await;
    let handle = hosted.runtime.handle();

    assert_hosted_refusal(
        &handle.spawn_spec(placed("assistant", "member-1")).await,
        "the definition's profile",
    );

    let mut overridden = placed("plain", "member-2");
    let mut profile = plain;
    profile.tools.mcp_servers = vec![personal_http_server("notes", NOTES_URL)];
    overridden.override_profile = Some(profile);
    assert_hosted_refusal(
        &handle.spawn_spec(overridden).await,
        "the spawn's own profile",
    );

    let unrelated = handle.spawn_spec(placed("plain", "member-3")).await;
    assert!(
        !is_hosted_refusal(&unrelated),
        "no personal server: {unrelated:?}"
    );
    assert!(members(&handle).await.is_empty());
    let _ = handle.shutdown().await;
}

/// A realm profile's own MCP servers ride a placed member's portable spec as
/// the definition's do: a placed spawn of a role whose realm profile declares
/// a personal server in its personal shape is refused, typed, before anything
/// is compiled or sent.
#[tokio::test]
async fn hosted_member_refuses_a_realm_profiles_personal_server() {
    use meerkat_mob::store::RealmProfileStore as _;

    let definition = meerkat_mob::MobDefinition::from_toml(
        r#"
[mob]
id = "team"

[profiles.plain]
model = "gpt-5.5"

[profiles.shared]
realm_profile = "org-assistant"
"#,
    )
    .expect("definition");
    let mut profile = definition
        .profiles
        .get(&meerkat_mob::ProfileName::from("plain"))
        .and_then(meerkat_mob::ProfileBinding::as_inline)
        .expect("the plain profile")
        .clone();
    profile.tools.mcp_servers = vec![personal_http_server("notes", NOTES_URL)];
    let realm = Arc::new(meerkat_mob::store::InMemoryRealmProfileStore::new());
    realm
        .create("org-assistant", &profile)
        .await
        .expect("the realm profile");
    let storage = meerkat_mob::MobStorage::in_memory().with_realm_profile_store(Some(realm));
    let hosted = hosted(definition, storage).await;
    let handle = hosted.runtime.handle();

    assert_hosted_refusal(
        &handle.spawn_spec(placed("shared", "member-1")).await,
        "the realm profile",
    );
    assert!(members(&handle).await.is_empty());
    let _ = handle.shutdown().await;
}

/// The same for a realm profile that declares a server under a personal
/// server's name in another shape: with no account selection, with a fixed
/// `oauth_account`, or at another URL. A portable profile carries each of
/// them, and the other host would serve it with its own account, so each is
/// refused, typed, before anything is compiled or sent.
#[tokio::test]
async fn hosted_member_refuses_a_realm_profile_naming_a_personal_server_in_another_shape() {
    use meerkat_mob::store::RealmProfileStore as _;

    let mut toml = String::from("[mob]\nid = \"team\"\n\n[profiles.plain]\nmodel = \"gpt-5.5\"\n");
    for index in 0..3 {
        toml.push_str(&format!(
            "\n[profiles.shared-{index}]\nrealm_profile = \"org-{index}\"\n"
        ));
    }
    let definition = meerkat_mob::MobDefinition::from_toml(&toml).expect("definition");
    let plain = definition
        .profiles
        .get(&meerkat_mob::ProfileName::from("plain"))
        .and_then(meerkat_mob::ProfileBinding::as_inline)
        .expect("the plain profile")
        .clone();
    let realm = Arc::new(meerkat_mob::store::InMemoryRealmProfileStore::new());
    let shapes = renamed_shapes();
    for (index, (_, server)) in shapes.iter().enumerate() {
        let mut profile = plain.clone();
        profile.tools.mcp_servers = vec![server.clone()];
        realm
            .create(&format!("org-{index}"), &profile)
            .await
            .expect("the realm profile");
    }
    let storage = meerkat_mob::MobStorage::in_memory().with_realm_profile_store(Some(realm));
    let hosted = hosted(definition, storage).await;
    let handle = hosted.runtime.handle();

    for (index, (shape, _)) in shapes.iter().enumerate() {
        assert_hosted_refusal(
            &handle
                .spawn_spec(placed(
                    &format!("shared-{index}"),
                    &format!("member-{index}"),
                ))
                .await,
            shape,
        );
    }
    assert!(members(&handle).await.is_empty());
    let _ = handle.shutdown().await;
}

/// An agent's child mob declares its own profiles (the agent mob tools create
/// it as an agent's `mob_create` does): a placed spawn there whose profile
/// declares a personal server, in its personal shape or under its name in
/// another shape, is refused, typed, before anything is compiled or sent.
#[tokio::test]
async fn hosted_member_refuses_an_agent_created_child_mobs_personal_server() {
    let root = meerkat_mob::MobDefinition::from_toml(
        "[mob]\nid = \"team\"\n\n[profiles.plain]\nmodel = \"gpt-5.5\"\n",
    )
    .expect("root definition");
    let hosted = hosted(root, meerkat_mob::MobStorage::in_memory()).await;
    let state = hosted
        .runtime
        .agent_mob_mcp_state()
        .expect("the agent mob tools");

    let mut toml = format!(
        r#"
[mob]
id = "child"

[profiles.personal]
model = "gpt-5.5"

[[profiles.personal.tools.mcp_servers]]
name = "notes"
url = "{NOTES_URL}"
oauth_account_selection = "discover"
"#
    );
    toml.push_str("\n[profiles.renamed]\nmodel = \"gpt-5.5\"\n");
    let mut child_definition =
        meerkat_mob::MobDefinition::from_toml(&toml).expect("child definition");
    if let Some(meerkat_mob::ProfileBinding::Inline(profile)) = child_definition
        .profiles
        .get_mut(&meerkat_mob::ProfileName::from("renamed"))
    {
        profile.tools.mcp_servers = vec![http_server("notes", NOTES_URL)];
    }
    let child_id = state
        .mob_create_definition_with_owner_bridge_session(
            child_definition,
            meerkat_core::SessionId::new(),
            true,
            false,
        )
        .await
        .expect("create the child mob");
    let child = state.handle_for(&child_id).await.expect("the child mob");

    assert_hosted_refusal(
        &child.spawn_spec(placed("personal", "member-1")).await,
        "the child mob's profile, in the personal shape",
    );
    assert_hosted_refusal(
        &child.spawn_spec(placed("renamed", "member-2")).await,
        "the child mob's profile, under the name in another shape",
    );
    assert!(members(&child).await.is_empty());
    let _ = hosted.runtime.handle().shutdown().await;
}
