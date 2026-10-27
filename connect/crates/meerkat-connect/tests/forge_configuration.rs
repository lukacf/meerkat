//! Actual frozen Forge facade and Reading owner, never a deployed application.
#![cfg(unix)]

use meerkat_connect::SessionOptions;
use meerkat_connect::{
    approval::{ApprovalPresenter, LocalApprovals, present_approval},
    forge::{
        FORGE_INSPECT, FORGE_PREPARE, FORGE_REAUTHENTICATE, FORGE_SELECT_HOST, FORGE_STATUS,
        FORGE_SUBMIT, ForgePresenter, present_forge_selection,
    },
    paired_host_tool::PairedHostFrontend,
    service_preparation::LazySetup,
    terminal::TerminalInput,
};
use meerkat_core::{
    AgentToolDispatcher, ApprovalDecision, ExecutionPolicyGatedDispatcher, SessionId, ToolError,
    ToolExecutionPolicy, agent::ToolDispatchContext, types::ToolCallView,
};
use nix::{
    pty::openpty,
    sys::termios::{LocalFlags, tcgetattr},
    unistd::write,
};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    task::JoinHandle,
    time::{sleep, timeout},
};

#[allow(
    dead_code,
    reason = "shared fixture; its native owner serves cli_paired_host"
)]
#[path = "fixtures/forge-reading/runner.rs"]
mod owner;
const BOUND: Duration = Duration::from_secs(15);

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
impl Write for Output {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
async fn shown(output: &Output, text: &str) {
    timeout(BOUND, async {
        while !output.text().contains(text) {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
fn send(fd: &std::os::fd::OwnedFd, text: &str) {
    assert_eq!(write(fd, text.as_bytes()).unwrap(), text.len());
}

struct Fixture {
    dispatcher: Arc<dyn AgentToolDispatcher>,
    session: SessionId,
    approvals: LocalApprovals,
    decisions: ApprovalPresenter,
    presenter: ForgePresenter,
    _directory: tempfile::TempDir,
}
impl Fixture {
    fn new(realm: &Path) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("setup-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(realm)
            .unwrap();
        let (paired_host, _selection) = PairedHostFrontend::deferred();
        let (lazy, _services) = LazySetup::new(paired_host);
        lazy.set_forge_artifacts(directory.path()).unwrap();
        let presenter = lazy.take_forge_presenter().unwrap();
        let (approvals, decisions) = LocalApprovals::create_paired_host(directory.path()).unwrap();
        let mut options = lazy.session_options(SessionOptions::default()).unwrap();
        approvals.attach_lazy_setup(&mut options, &lazy).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let dispatcher = ExecutionPolicyGatedDispatcher::new(
            options.build.external_tools.unwrap(),
            ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap(),
        )
        .with_dispatch_admission(options.build.tool_dispatch_admission.unwrap());
        Self {
            dispatcher: Arc::new(dispatcher),
            session,
            approvals,
            decisions,
            presenter,
            _directory: directory,
        }
    }
    fn call(&self, id: &str, name: &str, args: Value) -> JoinHandle<Result<Value, ToolError>> {
        let (dispatcher, session, id, name) = (
            self.dispatcher.clone(),
            self.session.clone(),
            id.to_owned(),
            name.to_owned(),
        );
        tokio::spawn(async move {
            let args = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
            dispatcher
                .dispatch_with_context(
                    ToolCallView {
                        id: &id,
                        name: &name,
                        args: &args,
                    },
                    &ToolDispatchContext::default().with_runtime_identity(session, None),
                )
                .await
                .map(|outcome| serde_json::from_str(&outcome.result.text_content()).unwrap())
        })
    }
    async fn read(&self, id: &str, name: &str, args: Value) -> Result<Value, ToolError> {
        timeout(BOUND, self.call(id, name, args))
            .await
            .unwrap()
            .unwrap()
    }
    async fn sign_in(
        &mut self,
        origin: &str,
        recovery: bool,
        reauth: bool,
    ) -> Result<Value, ToolError> {
        let task = self.call(
            if reauth { "reauth" } else { "select" },
            if reauth {
                FORGE_REAUTHENTICATE
            } else {
                FORGE_SELECT_HOST
            },
            json!({}),
        );
        let prompt = timeout(BOUND, self.presenter.next())
            .await
            .unwrap()
            .unwrap();
        let pty = openpty(None, None).unwrap();
        let original = tcgetattr(&pty.slave).unwrap();
        let observed = pty.slave.try_clone().unwrap();
        let mut input = TerminalInput::from_owned_fd(pty.slave);
        let mut output = Output::default();
        let view = output.clone();
        let (result, ()) = timeout(BOUND, async {
            tokio::join!(
                present_forge_selection(prompt, &mut input, &mut output, std::future::pending()),
                async {
                    if recovery {
                        shown(&view, "Previous Forge changes").await;
                        send(&pty.master, "1\n");
                    } else if !reauth {
                        shown(&view, "Forge Console origin").await;
                        send(&pty.master, &format!("{origin}\n"));
                    }
                    shown(&view, "Forge Console password (hidden)").await;
                    assert!(
                        !tcgetattr(&observed)
                            .unwrap()
                            .local_flags
                            .intersects(LocalFlags::ECHO | LocalFlags::ICANON)
                    );
                    send(&pty.master, &format!("{}\n", owner::PASSWORD));
                }
            )
        })
        .await
        .unwrap();
        assert_eq!(tcgetattr(&observed).unwrap(), original);
        assert!(!view.text().contains(owner::PASSWORD));
        let response = timeout(BOUND, task).await.unwrap().unwrap();
        assert_eq!(result.is_ok(), response.is_ok());
        response
    }
    async fn operation(&mut self) {
        let prompt = timeout(BOUND, self.presenter.next())
            .await
            .unwrap()
            .unwrap();
        let pair = openpty(None, None).unwrap();
        let mut input = TerminalInput::from_owned_fd(pair.slave);
        assert!(
            timeout(
                BOUND,
                present_forge_selection(
                    prompt,
                    &mut input,
                    &mut Vec::new(),
                    std::future::pending()
                )
            )
            .await
            .unwrap()
            .unwrap()
        );
    }
    async fn prepare(&mut self, args: Value) -> Result<Value, ToolError> {
        let task = self.call("prepare", FORGE_PREPARE, args);
        self.operation().await;
        timeout(BOUND, task).await.unwrap().unwrap()
    }
    async fn approve(&mut self, host: &mut owner::Owner, expected_posts: usize) -> String {
        let prompt = timeout(BOUND, self.decisions.next())
            .await
            .unwrap()
            .unwrap();
        let id = prompt.record().approval_id.to_string();
        let pair = openpty(None, None).unwrap();
        let mut input = TerminalInput::from_owned_fd(pair.slave);
        let mut output = Output::default();
        let view = output.clone();
        let (result, ()) = timeout(BOUND, async {
            tokio::join!(
                present_approval(prompt, &mut input, &mut output, std::future::pending()),
                async {
                    shown(&view, "Type 'approve").await;
                    assert!(view.text().contains("messages already queued"));
                    send(&pair.master, "approve wrong-id\n");
                    shown(&view, "Use the exact approval ID").await;
                    assert_eq!(posts(&host.control("observe").await), expected_posts);
                    send(&pair.master, &format!("approve {id}\n"));
                }
            )
        })
        .await
        .unwrap();
        assert!(result.unwrap());
        view.text()
    }
}
fn arguments(inventory: &Value) -> Value {
    let observation = &inventory["observation"];
    json!({"participant_id":observation["participants"][0]["id"], "route_id":observation["routes"][0]["id"],
        "telegram_user_id":123456789, "exclusive_polling_acknowledged":true, "expected_revision":observation["revision"]})
}
fn posts(value: &Value) -> usize {
    value["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["operation"] == "submit")
        .count()
}
fn preparations(value: &Value) -> usize {
    value["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["operation"] == "prepare")
        .count()
}

#[tokio::test]
#[ignore = "requires the pinned existing Forge Python environment"]
async fn actual_owner_exact_approval_uncertain_reauth_and_historical_recovery() {
    let mut host = owner::Owner::start().await;
    let realm = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut fixture = Fixture::new(realm.path());
    fixture.sign_in(&host.origin, false, false).await.unwrap();
    let inventory = fixture
        .read("inventory", FORGE_INSPECT, json!({}))
        .await
        .unwrap();
    let args = arguments(&inventory);
    let prepared = fixture.prepare(args.clone()).await.unwrap();
    let key = prepared["proposal"]["change_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let exact = json!({"change_id":key});
    let denied = fixture.call("denied", FORGE_SUBMIT, exact.clone());
    fixture
        .decisions
        .next()
        .await
        .unwrap()
        .decide(ApprovalDecision::Deny)
        .unwrap();
    assert!(denied.await.unwrap().is_err());
    assert_eq!(posts(&host.control("observe").await), 0);
    host.control("fail-next-submit").await;
    let submit = fixture.call("approved", FORGE_SUBMIT, exact.clone());
    let review = fixture.approve(&mut host, 0).await;
    fixture.operation().await;
    assert!(submit.await.unwrap().is_err());
    assert!(fixture.approvals.has_uncertain_dispatch());
    assert!(fixture.read("blocked", FORGE_PREPARE, args).await.is_err());
    assert!(
        fixture
            .read("service-blocked", "toolkit_prepare_services", json!({}))
            .await
            .is_err()
    );
    host.control("expire").await;
    assert!(
        fixture
            .read("expired", FORGE_STATUS, exact.clone())
            .await
            .is_err()
    );
    fixture.sign_in(&host.origin, false, true).await.unwrap();
    let status = fixture
        .read("readback", FORGE_STATUS, exact.clone())
        .await
        .unwrap();
    assert_eq!(status["observation"]["status"], "committed");
    assert_eq!(status["observation"]["proposal"], prepared["proposal"]);
    assert!(fixture.approvals.has_uncertain_dispatch());
    assert!(
        fixture
            .read("no-replay", FORGE_SUBMIT, exact.clone())
            .await
            .is_err()
    );
    host.control("expire").await;
    host.control("source-change").await;
    assert!(
        fixture
            .read("expired-again", FORGE_STATUS, exact.clone())
            .await
            .is_err()
    );
    fixture.sign_in(&host.origin, false, true).await.unwrap();
    let historical = fixture
        .read("historical", FORGE_STATUS, exact.clone())
        .await
        .unwrap();
    assert_eq!(
        historical["observation"]["binding_matches_current_source"],
        false
    );
    let observed = host.control("observe").await;
    assert_eq!(posts(&observed), 1);
    assert_eq!(preparations(&observed), 1);
    assert_eq!(observed["state"]["revision"], 1);
    let submitted = observed["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["operation"] == "submit")
        .unwrap();
    assert_eq!(submitted["payload"]["proposal"], prepared["proposal"]);
    assert_eq!(submitted["approved"], true);
    let (final_owner, canaries) = host.finish().await;
    for private in canaries.iter().map(String::as_str).chain([owner::PASSWORD]) {
        for public in [&prepared, &status, &historical] {
            assert!(!public.to_string().contains(private));
        }
        assert!(!review.contains(private));
    }
    assert_eq!(posts(&final_owner), 1);
}

#[tokio::test]
#[ignore = "requires the pinned existing Forge Python environment"]
async fn actual_owner_http_success_unknown_keeps_all_effects_status_only() {
    let mut host = owner::Owner::start().await;
    let realm = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut fixture = Fixture::new(realm.path());
    fixture.sign_in(&host.origin, false, false).await.unwrap();
    let inventory = fixture
        .read("inventory", FORGE_INSPECT, json!({}))
        .await
        .unwrap();
    let prepared = fixture.prepare(arguments(&inventory)).await.unwrap();
    let exact = json!({"change_id":prepared["proposal"]["change_id"]});
    host.control("forget-changes").await;
    let task = fixture.call("missing", FORGE_SUBMIT, exact.clone());
    fixture.approve(&mut host, 0).await;
    fixture.operation().await;
    assert!(task.await.unwrap().is_err());
    assert!(fixture.approvals.has_uncertain_dispatch());
    let status = fixture
        .read("unknown", FORGE_STATUS, exact.clone())
        .await
        .unwrap();
    assert_eq!(status["observation"]["status"], "unknown");
    assert!(fixture.approvals.has_uncertain_dispatch());
    for (name, args) in [
        (FORGE_SUBMIT, exact),
        ("toolkit_prepare_services", json!({})),
        ("paired_host_select_host", json!({})),
    ] {
        assert!(fixture.read("blocked", name, args).await.is_err());
    }
    let (result, _) = host.finish().await;
    assert_eq!(posts(&result), 1);
    assert_eq!(result["state"]["revision"], 0);
    assert!(
        result["requests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["path"] == "/console/forge/frontends/submit" && r["status"] == 200)
    );
}

#[tokio::test]
#[ignore = "requires the pinned existing Forge Python environment"]
async fn actual_owner_lost_prepare_and_mismatched_principal_never_create_replay_authority() {
    for control in ["fail-next-prepare", "wrong-prepare-principal"] {
        let mut host = owner::Owner::start().await;
        let realm = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let mut fixture = Fixture::new(realm.path());
        fixture.sign_in(&host.origin, false, false).await.unwrap();
        let inventory = fixture
            .read("inventory", FORGE_INSPECT, json!({}))
            .await
            .unwrap();
        let args = arguments(&inventory);
        host.control(control).await;
        assert!(fixture.prepare(args.clone()).await.is_err());
        assert!(
            fixture
                .read("no-prepare-retry", FORGE_PREPARE, args)
                .await
                .is_err()
        );
        assert!(
            fixture
                .read(
                    "no-invented-id",
                    FORGE_SUBMIT,
                    json!({"change_id":"reading-telegram:invented"})
                )
                .await
                .is_err()
        );
        let (result, _) = host.finish().await;
        assert_eq!(preparations(&result), 1);
        assert_eq!(posts(&result), 0);
        assert_eq!(result["state"]["revision"], 0);
        assert!(
            !std::fs::read_dir(fixture._directory.path())
                .unwrap()
                .any(|p| p
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("forge-change-"))
        );
    }
}
