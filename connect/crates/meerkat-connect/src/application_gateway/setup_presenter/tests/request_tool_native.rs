//! Real registered RequestTool, native input and the gateway's account-result
//! handler. Only the model, package response and loopback OAuth issuer are
//! fixtures. This is not copied-CLI, Console-login or installation acceptance.

#[path = "request_tool_native/authorization.rs"]
mod authorization;

use super::super::*;
use authorization::FixtureAuthorization;
use async_trait::async_trait;
use futures_util::FutureExt;
use meerkat::{McpAdapterComposer, McpAdapterComposeRefusal, McpRouterAdapter};
use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore,
    PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore, TokenStoreError, TokenStoreObservation};
use meerkat_core::{AgentToolDispatcher, ApprovalId, ApprovalService, ApprovalStatus,
    AuthCredentialIdentity, CredentialAccountId, CredentialAccountRef,
    MobMemberBinding, RealmLocator, SessionBuildOptions, SessionId,
    ToolCallView, ToolDef, ToolDispatchContext, ToolError, ToolMutationClass,
    ToolResult, ops::ToolDispatchOutcome};
use meerkat_runtime::{Input, PromptInput, completion::CompletionOutcome};
use meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST;
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, io::{self, Write}, path::Path,
    pin::Pin, sync::{Arc, Mutex, OnceLock, atomic::{AtomicUsize, Ordering}},
    time::Duration};
use tokio::sync::{mpsc, oneshot, watch};
use crate::{ConnectSession, SessionOptions, approval::LocalApprovals,
    connector_auth::{ConnectorAuth, Control, Reply},
    installed_service_review::InstalledServiceReview, realm::RealmOptions,
    stdio::tests::{ConnectorInstallFixture, connector_fixture_with_owners,
        connector_http, connector_http_reply, connector_metadata,
        connector_operations, connector_unprimed_fixture}, terminal::TerminalInput};

#[path = "../../../../tests/support/native_model_peer.rs"]
mod native_model_peer;
use native_model_peer::ModelPeer;

const MODEL: &str = "claude-sonnet-4-5";
const SIBLING: &str = "account_request_locality_read";
const FIRST: &str = "account-request-1";
const SECOND: &str = "account-request-2";
const PRIVATE_ERROR: &str = "private-account-status-canary";

#[derive(Clone, Copy)]
enum Schedule { StatusUnavailable, ReviewEligibilityExpired }

// This wraps the actual file store used by the retained HostAuthService. It
// does not replace connector_status, RequestTool or a native authorization.
struct ReadFault {
    key: TokenKey,
    remaining: usize,
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}
struct Tokens {
    inner: FileTokenStore,
    fault: Mutex<Option<ReadFault>>,
    failed_reads: AtomicUsize,
    writes: AtomicUsize,
}
impl Tokens {
    fn new(path: &Path) -> Self {
        Self { inner: FileTokenStore::new(path), fault: Mutex::new(None),
            failed_reads: AtomicUsize::new(0), writes: AtomicUsize::new(0) }
    }
    fn fail_second_read(&self, key: TokenKey) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, observed) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let mut fault = self.fault.lock().unwrap();
        assert!(fault.is_none());
        *fault = Some(ReadFault { key, remaining: 2, entered, release: released });
        (observed, release)
    }
    // The deliberate fault applies to either actual read boundary. Normal
    // status observations still come from the retained audited file backend.
    async fn read_fault(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let held = {
            let mut fault = self.fault.lock().unwrap();
            match fault.as_mut() {
                Some(next) if &next.key == key => {
                    next.remaining -= 1;
                    if next.remaining == 0 { fault.take() } else { None }
                }
                _ => None,
            }
        };
        if let Some(held) = held {
            self.failed_reads.fetch_add(1, Ordering::SeqCst);
            held.entered.send(()).expect("actual retained account read acknowledged");
            // Dropping the observing test also releases this fixture wait. It
            // does not claim a dropped native operation is cleaned up.
            let _ = held.release.await;
            return Err(TokenStoreError::Unavailable(PRIVATE_ERROR.into()));
        }
        Ok(())
    }
}
#[async_trait]
impl TokenStore for Tokens {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.read_fault(key).await?;
        self.inner.load(key).await
    }
    async fn observe_status(&self, key: &TokenKey) -> Result<TokenStoreObservation, TokenStoreError> {
        self.read_fault(key).await?;
        self.inner.observe_status(key).await
    }
    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.save(key, tokens).await
    }
    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.clear(key).await
    }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> { self.inner.list().await }
    fn backend_name(&self) -> &'static str { "requesttool-fixture-file" }
}

// Native construction supplies the session ID. The same checked owner and
// actual registered account dispatcher are composed before actor publication.
struct Composer {
    realm: RealmLocator,
    auth: Arc<ConnectorAuth>,
    enrollment: LocalSetupApprovals,
    installed: OnceLock<Arc<ConnectorInstallFixture>>,
    approvals: OnceLock<Arc<LocalApprovals>>,
    calls: AtomicUsize,
}
impl McpAdapterComposer for Composer {
    fn compose(&self, _adapter: &Arc<McpRouterAdapter>, session: &SessionId,
        member: Option<&MobMemberBinding>) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal>
    {
        assert!(member.is_none(), "this fixture is one ordinary native session");
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        let installed = Arc::new(connector_unprimed_fixture(self.realm.clone(), session.clone(), false));
        let dispatcher = self.enrollment.create_installed(|approvals| {
            approvals.bind_session(session)?;
            assert!(self.approvals.set(approvals.clone()).is_ok());
            InstalledServiceReview::fixture_checked(self.realm.clone(), session.clone(),
                installed.owner.clone(), self.auth.clone(), approvals)
        }).map_err(|_| McpAdapterComposeRefusal::new("native account fixture composition refused"))?;
        assert!(self.installed.set(installed).is_ok());
        Ok(dispatcher)
    }
}

struct Sibling { calls: AtomicUsize, session: OnceLock<SessionId> }
#[async_trait]
impl AgentToolDispatcher for Sibling {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(SIBLING, "Read-only locality control",
            json!({"type":"object","additionalProperties":false})))].into()
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == SIBLING { ToolMutationClass::ReadOnly } else { ToolMutationClass::Unknown }
    }
    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::execution_failed("actual native context is required"))
    }
    async fn dispatch_with_context(&self, call: ToolCallView<'_>, context: &ToolDispatchContext)
        -> Result<ToolDispatchOutcome, ToolError>
    {
        assert_eq!(call.id, "permitted-sibling");
        assert_eq!(call.name, SIBLING);
        assert_eq!(context.origin_session_id(), self.session.get());
        assert!(context.work_authorization().is_some());
        assert!(context.retained_work().is_some());
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        Ok(ToolResult::new(call.id.into(), json!({"sibling_completed":true}).to_string(), false).into())
    }
}

// These are text/error observations from the actual native HTTP request,
// not fabricated native ToolResult or controller authority holders.
struct ObservedToolResult { is_error: bool, text: String }
impl ObservedToolResult {
    fn text_content(&self) -> String { self.text.clone() }
}

struct Model {
    proposal: Value,
    issued: AtomicUsize,
    results: Mutex<BTreeMap<String, ObservedToolResult>>,
}
impl Model {
    fn capture(&self, body: &Value) -> io::Result<()> {
        let messages = body["messages"].as_array()
            .ok_or_else(|| io::Error::other("Native account fixture request has no messages"))?;
        let mut seen = self.results.lock().unwrap();
        for message in messages.iter().filter(|message| message["role"] == "user") {
            let Some(blocks) = message["content"].as_array() else { continue; };
            for block in blocks.iter().filter(|block| block["type"] == "tool_result") {
                let id = block["tool_use_id"].as_str()
                    .ok_or_else(|| io::Error::other("Native account fixture result has no ID"))?;
                let text = block["content"].as_str()
                    .ok_or_else(|| io::Error::other("Native account fixture result is not text"))?;
                let is_error = block["is_error"].as_bool()
                    .ok_or_else(|| io::Error::other("Native account fixture result has no error status"))?;
                seen.insert(id.into(), ObservedToolResult { is_error, text: text.into() });
            }
        }
        Ok(())
    }

    fn reply(&self, body: Value) -> io::Result<String> {
        self.capture(&body)?;
        let next = self.issued.fetch_add(1, Ordering::SeqCst);
        let (id, name, args) = match next {
            0 => (FIRST, SETUP_ACCOUNT_REQUEST, self.proposal.clone()),
            1 => {
                let seen = self.results.lock().unwrap();
                let result = seen.get(FIRST).expect("request 1 feedback must precede the sibling");
                assert!(result.is_error, "the failed request must not become a successful account result");
                let text = result.text_content();
                let payload: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(payload["error"], "tool_unavailable");
                assert!(payload["message"].as_str().unwrap().contains("temporarily unavailable"));
                assert!(!text.contains(PRIVATE_ERROR));
                ("permitted-sibling", SIBLING, json!({}))
            }
            2 => {
                let seen = self.results.lock().unwrap();
                let result = seen.get("permitted-sibling").expect("sibling result precedes request 2");
                assert!(!result.is_error);
                assert_eq!(serde_json::from_str::<Value>(&result.text_content()).unwrap(),
                    json!({"sibling_completed":true}));
                (SECOND, SETUP_ACCOUNT_REQUEST, self.proposal.clone())
            }
            3 => {
                let seen = self.results.lock().unwrap();
                let result = seen.get(SECOND).expect("the second actual account request completed");
                assert!(!result.is_error);
                let result: Value = serde_json::from_str(&result.text_content()).unwrap();
                assert_eq!(result["state"], "denied");
                assert_eq!(result["access_reviewed"], true);
                for key in ["login_completed", "grant_issued", "ready", "fresh_instruction_retained"] {
                    assert_eq!(result[key], false);
                }
                return Ok(native_model_peer::text_reply("Account request locality schedule completed"));
            }
            _ => panic!("one original has exactly two account requests and one sibling"),
        };
        if !body["tools"].as_array().is_some_and(|tools|
            tools.iter().any(|tool| tool["name"].as_str() == Some(name))) {
            return Err(io::Error::other("Native account fixture tool is not offered"));
        }
        Ok(native_model_peer::tool_reply(id, name, &args))
    }
}

#[derive(Clone)]
struct Output(watch::Sender<String>);
impl Output {
    fn new() -> Self { Self(watch::channel(String::new()).0) }
    fn text(&self) -> String { self.0.borrow().clone() }
    async fn prompt(&self, ordinal: usize) -> ApprovalId {
        let mut updates = self.0.subscribe();
        loop {
            let id = {
                let text = updates.borrow_and_update();
                text.split("Type 'approve ").nth(ordinal + 1)
                    .and_then(|suffix| suffix.split_once('\''))
                    .map(|(id, _)| id.parse().expect("actual displayed native approval ID"))
            };
            if let Some(id) = id { return id; }
            updates.changed().await.expect("retained terminal output");
        }
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
        self.0.send_modify(|current| current.push_str(text));
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

fn pending_interrupt() -> impl Future<Output = io::Result<()>> + Send {
    std::future::pending()
}

async fn authorize_existing_account(control: &mut Control, installed: &ConnectorInstallFixture,
    target: meerkat::ConnectorOAuthTarget)
{
    assert!(matches!(control.start(target, "https://console.example/callback".into(),
        installed.selection(connector_operations()).unwrap()).await.unwrap(), Reply::Started(_)));
    let started = control.started().unwrap().clone();
    let (_live, mut retired) = watch::channel(false);
    let (_cancel, cancellation) = oneshot::channel();
    let (result, cancelled, uncertain) = control.complete(started.state,
        "private-code-canary".into(), started.host_attempt_handle, &mut retired, cancellation).await;
    assert!(matches!(result, Ok(Reply::Completed(_))));
    assert!(cancelled.is_none());
    assert!(!uncertain);
    assert!(control.completed().is_some());
}

// One existing loopback OAuth ceremony establishes the credential through the
// actual native owner. No token record or lifecycle marker is fabricated.
async fn issuer(listener: tokio::net::TcpListener, origin: String) {
    for expected in ["GET /.well-known/oauth-authorization-server ",
        "GET /.well-known/oauth-authorization-server ", "POST /token ",
        "GET /.well-known/openid-configuration ", "GET /userinfo "]
    {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (header, body) = connector_http(&mut socket).await;
        assert!(header.starts_with(expected));
        let response = if expected == "POST /token " {
            let body = String::from_utf8(body).unwrap();
            assert!(body.contains("code=private-code-canary"));
            assert!(body.contains("client_secret=private-profile-canary"));
            json!({"access_token":"private-token-canary", "token_type":"Bearer",
                "expires_in":3600, "scope":"openid"})
        } else if expected == "GET /userinfo " {
            assert!(header.to_ascii_lowercase().contains("authorization: bearer private-token-canary"));
            json!({"sub":"verified-subject"})
        } else { connector_metadata(&origin) };
        connector_http_reply(&mut socket, response).await;
    }
}

// Boxing the entire schedule preserves the existing default libtest stack.
// There is no new thread, runner, environment override or stack-size setting.
#[inline(never)]
fn schedule(mode: Schedule) -> Pin<Box<dyn Future<Output = ()>>> {
    Box::pin(async move {
        let root = tempfile::tempdir().unwrap();
        let storage = RealmOptions { context_root:root.path().into(),
            state_root:Some(root.path().join("state")),
            user_config_root:Some(root.path().join("user")) }.resolve("requesttool-locality").unwrap();
        let realm = storage.locator.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let proposal = json!({"job_id":"3".repeat(32), "connector":{
            "slot_id":"test-account", "issuer":origin, "client":"test-client",
            "resource":"https://service.example", "scopes":["openid"],
            "strategy_id":"oidc-userinfo-v1", "account_selection":null,
            "client_auth_profile":"test-profile"}, "operations":connector_operations(),
            "agent":"selected-agent", "expires_in_ms":60_000});
        let model = Arc::new(Model {
            proposal, issued:AtomicUsize::new(0), results:Mutex::new(BTreeMap::new()) });
        let observed = model.clone();
        let peer = ModelPeer::open(MODEL, Arc::new(move |body| {
            Box::pin(std::future::ready(observed.reply(body)))
        })).await.unwrap();
        let mut native_session = None;
        let mut native_control = None;
        let result = std::panic::AssertUnwindSafe(async {
        peer.configure(&storage).await.unwrap();
        let (authorization, configuration) = FixtureAuthorization::new(realm.realm.clone(), SIBLING);
        let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
            Arc::new(meerkat_store::MemoryStore::new()),
            Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()), configuration).unwrap();
        let tokens = Arc::new(Tokens::new(&root.path().join("credentials")));
        let provider = ProviderAuthPersistence::new(tokens.clone(),
            Arc::new(FileLockCoordinator::new(root.path().join("locks"))));
        let (mut startup, auth, control) = connector_fixture_with_owners(
            root.path(), &origin, storage.clone(), provider, Some(persistence)).await;
        let (factory, reviews) = channel(root.path().to_owned());
        let mut presenters = SetupPresenters::new(reviews);
        let composer = Arc::new(Composer { realm:realm.clone(), auth:auth.clone(), enrollment:factory,
            installed:OnceLock::new(), approvals:OnceLock::new(), calls:AtomicUsize::new(0) });
        startup.factory = startup.factory.with_mcp_adapter_composer(composer.clone());
        native_control = Some(control);
        let control = native_control.as_mut().unwrap();
        startup.config = crate::realm::read_config(&storage).await.unwrap();
        let sibling = Arc::new(Sibling { calls:AtomicUsize::new(0), session:OnceLock::new() });
        let options = SessionOptions {
            model:native_model_peer::model_request(realm.realm.as_str(), MODEL).unwrap(),
            build:SessionBuildOptions { external_tools:Some(sibling.clone()), ..Default::default() },
        };
        native_session = Some(ConnectSession::fixture_open_governed(startup, options).await.unwrap());
        let session = native_session.as_ref().unwrap();
        assert!(sibling.session.set(session.id().clone()).is_ok());
        let installed = composer.installed.get().expect("actual native adapter was composed");
        installed.prime(true).await;
        let approvals = composer.approvals.get().unwrap().clone();
        let approval_path = approvals.store_path().to_owned();
        let selected = installed.owner.installed_selection(&realm, session.id(), &"3".repeat(32)).unwrap().installed;
        let target = meerkat::ConnectorOAuthTarget {
            slot:CredentialAccountRef { realm:realm.realm.clone(),
                account:CredentialAccountId::parse("test-account").unwrap() },
            issuer:origin.clone(), client:"test-client".into(), resource:"https://service.example".into(),
            scopes:["openid".into()].into_iter().collect(), strategy_id:"oidc-userinfo-v1".into(),
            account:serde_json::from_value(Value::Null).unwrap(), client_auth_profile:Some("test-profile".into()),
        };
        // Join the existing native login and the exact loopback server before
        // the model starts. The tested requests must not start another login.
        let ((), ()) = tokio::join!(authorize_existing_account(control, installed, target.clone()),
            issuer(listener, origin));
        assert!(matches!(auth.facade.connector_status(&target.slot).await.unwrap().phase,
            meerkat::ConnectorAuthPhase::Authorized));
        let key = TokenKey::from_credential_identity(&AuthCredentialIdentity::Account(target.slot.clone()));
        let before = serde_json::to_vec(&tokens.load(&key).await.unwrap()).unwrap();
        let mut keys_before = tokens.list().await.unwrap(); keys_before.sort();
        let writes_before = tokens.writes.load(Ordering::SeqCst);
        let config_before = serde_json::to_vec(&crate::realm::read_config(&storage).await.unwrap()).unwrap();
        let fault = matches!(mode, Schedule::StatusUnavailable).then(|| tokens.fail_second_read(key.clone()));
        let controller = session.fixture_controller().await.unwrap();
        let input = authorization.bind_input(session.id(), Input::Prompt(PromptInput::new(
            "Try the first account request; after local refusal read the sibling and try the second account request.", None)), controller);
        let input_id = input.id().clone();
        let (admission, completion) = session.fixture_submit_bound(input).await.unwrap();
        assert!(matches!(&admission, meerkat_runtime::AcceptOutcome::Accepted { input_id: actual, .. }
            if actual == &input_id), "one fresh actual native admission, never replay");
        let (send_lines, receive_lines) = mpsc::channel(1);
        let mut terminal = TerminalInput::scripted(receive_lines);
        let captured = Output::new(); let mut output = captured.clone();
        let mut first_approval = None;
        let (mut enrollment, event) = presenters.next().await.expect("first actual RequestTool enrollment");
        let SetupEvent::Account(request) = event else { panic!("the native request must reach the account presenter"); };
        assert!(enrollment.presenter.is_some());
        let presenting = enrollment.accounts.as_mut().unwrap().present(request,
            &mut enrollment.presenter, &mut terminal, &mut output, pending_interrupt());
        let observe = async {
            match mode {
                Schedule::StatusUnavailable => {
                    let (entered, release) = fault.unwrap();
                    entered.await.expect("second actual account credential read after native entry");
                    assert_eq!(model.issued.load(Ordering::SeqCst), 1);
                    assert_eq!(sibling.calls.load(Ordering::SeqCst), 0);
                    assert_eq!(tokens.failed_reads.load(Ordering::SeqCst), 1);
                    release.send(()).unwrap();
                }
                Schedule::ReviewEligibilityExpired => {
                    let id = captured.prompt(0).await;
                    let store = ApprovalService::with_store(Arc::new(
                        meerkat_store::FileApprovalStore::open(&approval_path).unwrap())).unwrap();
                    assert_eq!(store.get(&id).unwrap().status, ApprovalStatus::Pending);
                    approvals.fixture_expire_waiting_review(&id).unwrap();
                    first_approval = Some(id.clone());
                    send_lines.send(Ok(format!("approve {id}"))).await.unwrap();
                }
            }
        };
        let (first_result, ()) = tokio::join!(presenting, observe);
        assert!(first_result.is_err(), "the actual request failed; the gateway decides its local scope");
        assert!(crate::application_gateway::finish_account_presentation(
            &mut presenters, enrollment, first_result).unwrap());
        assert!(approvals.check_session(session.id()).is_ok());
        assert_eq!(approvals.store_path(), approval_path);
        assert!(installed.selection(connector_operations()).unwrap().check_current().is_ok());

        let (mut enrollment, event) = presenters.next().await.expect("same enrollment must accept request 2");
        let SetupEvent::Account(request) = event else { panic!("second request must reach the actual account owner"); };
        assert_eq!(composer.calls.load(Ordering::SeqCst), 1, "no replacement owner or session");
        assert_eq!(sibling.calls.load(Ordering::SeqCst), 1);
        let presenting = enrollment.accounts.as_mut().unwrap().present(request,
            &mut enrollment.presenter, &mut terminal, &mut output, pending_interrupt());
        let deny = async {
            let ordinal = usize::from(matches!(mode, Schedule::ReviewEligibilityExpired));
            let id = captured.prompt(ordinal).await;
            assert_ne!(Some(&id), first_approval.as_ref());
            send_lines.send(Ok(format!("deny {id}"))).await.unwrap();
            id
        };
        let (second_result, second_approval) = tokio::join!(presenting, deny);
        assert!(matches!(second_result, Ok(true)));
        assert!(crate::application_gateway::finish_account_presentation(
            &mut presenters, enrollment, second_result).unwrap());
        assert!(matches!(completion.wait().await.unwrap(), CompletionOutcome::Completed(_)),
            "the original input completes without cancellation or another admission");
        assert_eq!(model.issued.load(Ordering::SeqCst), 4);
        assert_eq!(model.results.lock().unwrap().len(), 3);
        assert!(approvals.check_session(session.id()).is_ok());
        let store = ApprovalService::with_store(Arc::new(
            meerkat_store::FileApprovalStore::open(&approval_path).unwrap())).unwrap();
        if let Some(id) = first_approval {
            assert_eq!(store.get(&id).unwrap().status, ApprovalStatus::Pending,
                "expired invocation is not a fabricated durable denial or approval");
        }
        assert_eq!(store.get(&second_approval).unwrap().status, ApprovalStatus::Denied);
        assert_eq!(tokens.failed_reads.load(Ordering::SeqCst),
            usize::from(matches!(mode, Schedule::StatusUnavailable)));
        assert_eq!(tokens.writes.load(Ordering::SeqCst), writes_before,
            "both account requests leave the existing native credential unchanged");
        let after = serde_json::to_vec(&tokens.load(&key).await.unwrap()).unwrap();
        assert!(before == after, "credential material and native lifecycle marker are unchanged");
        let mut keys_after = tokens.list().await.unwrap(); keys_after.sort();
        assert!(keys_before == keys_after);
        let config_after = serde_json::to_vec(&crate::realm::read_config(&storage).await.unwrap()).unwrap();
        assert!(config_before == config_after, "selected realm and inherited configuration remain unchanged");
        assert_eq!(installed.owner.installed_selection(&realm, session.id(), &"3".repeat(32)).unwrap().installed, selected);
        let text = captured.text();
        assert!(!text.contains("Open this private sign-in URL"));
        for secret in [PRIVATE_ERROR, "private-code-canary", "private-profile-canary", "private-token-canary"] {
            assert!(!text.contains(secret));
            assert!(model.results.lock().unwrap().values().all(|result| !result.text_content().contains(secret)));
        }
        terminal.check_cleanup().unwrap();
        drop(presenters);
        }).catch_unwind().await;
        // Retain the same native owners through a failed assertion, then join
        // the HTTP peer only after native cleanup. No replacement pin is made.
        let control_cleanup = native_control.as_mut().map(Control::retire).transpose();
        let session_cleanup = match native_session.take() {
            Some(session) => session.close().await,
            None => Ok(()),
        };
        let peer_cleanup = peer.finish().await;
        if let Err(panic) = result {
            if let Err(error) = control_cleanup { eprintln!("fixture control cleanup: {error:?}"); }
            if let Err(error) = session_cleanup { eprintln!("fixture session cleanup: {error:?}"); }
            if let Err(error) = peer_cleanup { eprintln!("fixture peer cleanup: {error:?}"); }
            std::panic::resume_unwind(panic);
        }
        control_cleanup.unwrap();
        session_cleanup.unwrap();
        peer_cleanup.unwrap();
    })
}

#[tokio::test]
async fn native_account_status_failure_preserves_sibling_and_second_request() {
    tokio::time::timeout(Duration::from_secs(30), schedule(Schedule::StatusUnavailable))
        .await.expect("one admitted native account-locality schedule must settle");
}

#[tokio::test]
async fn native_account_review_eligibility_expiry_preserves_sibling_and_second_request() {
    tokio::time::timeout(Duration::from_secs(30), schedule(Schedule::ReviewEligibilityExpired))
        .await.expect("one admitted native account-locality schedule must settle");
}
