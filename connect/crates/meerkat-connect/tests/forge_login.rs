//! Protected PTY and loopback-only Forge sign-in. The ignored owner test runs
//! the frozen Python ConsoleFront, rather than substituting a Rust login mock.

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
};
use meerkat_connect::{
    ConnectError, ConnectSession,
    forge::{FORGE_INSPECT, FORGE_REAUTHENTICATE, FORGE_SELECT_HOST, ForgeFrontend, ForgePrompt, present_forge_selection},
    terminal::TerminalInput,
};
use meerkat_core::{AgentToolDispatcher, ToolError, ToolMutationClass, types::ToolCallView};
use futures_util::FutureExt;
use nix::{
    fcntl::{FcntlArg, fcntl},
    pty::openpty,
    sys::termios::{LocalFlags, Termios, tcgetattr},
    unistd::write,
};
use serde_json::json;
use std::{
    io::{self, Write},
    os::fd::OwnedFd,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Notify, oneshot},
    time::{sleep, timeout},
};

#[path = "fixtures/forge-auth/runner.rs"]
mod forge_owner;

#[path = "forge_login/turn.rs"]
mod turn;

// The same Toolkit-owned helper called by the real CLI main loop.
#[path = "../src/main_connector.rs"]
mod main_connector;

const BOUND: Duration = Duration::from_secs(10);
const PASSWORD: &str = "  fixture läsbar 密码 +&= password  ";
const NONCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SESSION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const COOKIE_NAME: &str = "mobkit_console_0123456789abcdef";

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Pty {
    master: OwnedFd,
    observer: OwnedFd,
    original: Termios,
    flags: i32,
    input: TerminalInput,
}
impl Pty {
    fn new() -> Self {
        let pair = openpty(None, None).unwrap();
        let observer = pair.slave.try_clone().unwrap();
        Self {
            original: tcgetattr(&observer).unwrap(),
            flags: fcntl(&observer, FcntlArg::F_GETFL).unwrap(),
            master: pair.master,
            observer,
            input: TerminalInput::from_owned_fd(pair.slave),
        }
    }
    fn restored(&self) {
        assert_eq!(tcgetattr(&self.observer).unwrap(), self.original);
        assert_eq!(
            fcntl(&self.observer, FcntlArg::F_GETFL).unwrap(),
            self.flags
        );
    }
}
fn send(fd: &OwnedFd, value: &str) {
    assert_eq!(write(fd, value.as_bytes()).unwrap(), value.len());
}
async fn shown(out: &Output, text: &str) {
    timeout(BOUND, async {
        while !out.text().contains(text) {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
async fn enter(master: &OwnedFd, observer: &OwnedFd, out: &Output, origin: &str, password: &str) {
    shown(out, "Forge Console origin").await;
    send(master, &format!("{origin}\n"));
    shown(out, "Forge Console password (hidden)").await;
    assert!(
        !tcgetattr(observer)
            .unwrap()
            .local_flags
            .contains(LocalFlags::ECHO)
    );
    send(master, &format!("{password}\n"));
}
fn raw(s: &str) -> Box<serde_json::value::RawValue> {
    serde_json::value::RawValue::from_string(s.into()).unwrap()
}
async fn request(
    front: &ForgeFrontend,
    id: &str,
) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
    front
        .dispatch(ToolCallView {
            id,
            name: FORGE_SELECT_HOST,
            args: &raw("{}"),
        })
        .await
}
async fn private_present(
    prompt: ForgePrompt,
    origin: &str,
    password: &str,
) -> Result<bool, meerkat_connect::ConnectError> {
    let mut pty = Pty::new();
    let mut out = Output::default();
    let observed = out.clone();
    let result = timeout(BOUND, async {
        tokio::join!(
            present_forge_selection(prompt, &mut pty.input, &mut out, std::future::pending()),
            enter(&pty.master, &pty.observer, &observed, origin, password),
        )
    })
    .await
    .unwrap()
    .0;
    pty.restored();
    for value in [password, NONCE, SESSION, "private refused body canary", "private response canary"] {
        assert!(!out.text().contains(value));
    }
    result
}

#[derive(Clone)]
struct Host {
    origin: String,
    scenario: &'static str,
    password: String,
    calls: Arc<Mutex<Vec<String>>>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
struct Server {
    host: Host,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn new(scenario: &'static str) -> Self {
        Self::with_password(scenario, PASSWORD.into()).await
    }
    async fn with_password(scenario: &'static str, password: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = Host {
            origin: format!("http://{}", listener.local_addr().unwrap()),
            scenario,
            password,
            calls: Default::default(),
            entered: Default::default(),
            release: Default::default(),
        };
        let (stop, receive) = oneshot::channel();
        let app = Router::new().fallback(handler).with_state(host.clone());
        let task = tokio::spawn(async {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = receive.await;
                })
                .await
                .unwrap();
        });
        Self {
            host,
            stop: Some(stop),
            task,
        }
    }
    async fn finish(mut self) -> Vec<String> {
        self.stop.take().unwrap().send(()).unwrap();
        timeout(BOUND, self.task).await.unwrap().unwrap();
        self.host.calls.lock().unwrap().clone()
    }
}
async fn handler(
    State(host): State<Host>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    host.calls
        .lock()
        .unwrap()
        .push(format!("{method} {}", uri.path()));
    assert_eq!(
        headers.get("host").unwrap(),
        host.origin.strip_prefix("http://").unwrap()
    );
    assert!(uri.query().is_none());
    assert!(!headers.contains_key("authorization"));
    if method == Method::GET && uri.path() == "/login" {
        assert!(!headers.contains_key("cookie"));
        assert!(body.is_empty());
        let nonce = if host.scenario == "nonce" {
            SESSION
        } else {
            NONCE
        };
        let html = format!(
            "<form method=\"post\" action=\"/login\"><input type=\"hidden\" name=\"nonce\" value=\"{nonce}\"><input type=\"password\" name=\"password\"></form>"
        );
        let mut response = (
            StatusCode::OK,
            [
                ("content-type", "text/html; charset=utf-8"),
                ("cache-control", "no-store"),
            ],
            html,
        )
            .into_response();
        let cookie = format!(
            "{COOKIE_NAME}_login={NONCE}; HttpOnly; Max-Age=300; Path=/; SameSite=Strict{}",
            if host.scenario == "secure-nonce" {
                "; Secure"
            } else {
                ""
            }
        );
        response
            .headers_mut()
            .insert("set-cookie", cookie.parse().unwrap());
        return response;
    }
    if method == Method::POST && uri.path() == "/login" {
        assert_eq!(headers.get("origin").unwrap(), host.origin.as_str());
        assert_eq!(
            headers.get("cookie").unwrap(),
            format!("{COOKIE_NAME}_login={NONCE}").as_str()
        );
        assert_eq!(
            headers.get("content-type").unwrap(),
            "application/x-www-form-urlencoded"
        );
        let form = reqwest::Url::parse(&format!(
            "http://127.0.0.1/?{}",
            std::str::from_utf8(&body).unwrap()
        ))
        .unwrap();
        assert_eq!(
            form.query_pairs().collect::<Vec<_>>(),
            vec![
                ("nonce".into(), NONCE.into()),
                ("password".into(), host.password.as_str().into())
            ]
        );
        if host.scenario == "held" {
            host.entered.notify_one();
            host.release.notified().await;
        }
        let refusal = match host.scenario {
            "wrong" => Some(StatusCode::UNAUTHORIZED),
            "wrong-forbidden" => Some(StatusCode::FORBIDDEN),
            "wrong-rate-limited" => Some(StatusCode::TOO_MANY_REQUESTS),
            "reauth-wrong" if host.calls.lock().unwrap().iter().filter(|call| call.as_str() == "POST /login").count() == 2 => Some(StatusCode::UNAUTHORIZED),
            _ => None,
        };
        if let Some(status) = refusal {
            return (status, "private refused body canary").into_response();
        }
        let status = if host.scenario == "redirect" {
            StatusCode::FOUND
        } else {
            StatusCode::SEE_OTHER
        };
        let mut response = (
            status,
            [("location", "/console"), ("cache-control", "no-store")],
            "",
        )
            .into_response();
        response.headers_mut().append(
            "set-cookie",
            format!(
                "{COOKIE_NAME}={SESSION}; HttpOnly; Max-Age=28800; Path=/; SameSite=Strict{}",
                if host.scenario == "secure-session" {
                    "; Secure"
                } else {
                    ""
                }
            )
            .parse()
            .unwrap(),
        );
        response.headers_mut().append(
            "set-cookie",
            format!("{COOKIE_NAME}_login=\"\"; Max-Age=0; Path=/; SameSite=Lax")
                .parse()
                .unwrap(),
        );
        return response;
    }
    assert_eq!(method, Method::GET);
    assert!(body.is_empty());
    assert_eq!(
        headers.get("cookie").unwrap(),
        format!("{COOKIE_NAME}={SESSION}").as_str()
    );
    if uri.path() == "/console/forge/frontends/descriptor" {
        let (status, text) = match host.scenario {
            "descriptor-missing" => (StatusCode::NOT_FOUND,
                r#"{"detail":"private response canary"}"#),
            "descriptor-malformed" => (StatusCode::OK,
                r#"{"protocol":1,"host":{},"private_fields":[],"private":"private response canary"}"#),
            other => panic!("unexpected descriptor fixture: {other}"),
        };
        return (status, [("content-type", "application/json"), ("cache-control", "no-store")], text)
            .into_response();
    }
    assert_eq!(uri.path(), "/console/forge/frontends");
    let support_reads = host.calls.lock().unwrap().iter()
        .filter(|call| call.as_str() == "GET /console/forge/frontends").count();
    let (status, text) = match host.scenario {
        "bad-support" => (501, r#"{"detail":"private response canary","host":{}}"#.to_owned()),
        "duplicate" => (501, r#"{"detail":"ignored","detail":"This application has not declared frontend management support."}"#.to_owned()),
        "expired" => (401, r#"{"error":"Console login required."}"#.to_owned()),
        "reauth-wrong" if support_reads > 1 => (401, r#"{"error":"Console login required."}"#.to_owned()),
        "declared" | "reauth-wrong" => (200, json!({"protocol":1,"host":{"protocol":1,"host_id":format!("forge:{}","c".repeat(64)),"application_id":"fixture","source_digest":"d".repeat(64)},"observation":{"private_uninterpreted_field":"not projected"}}).to_string()),
        _ => (501, r#"{"detail":"This application has not declared frontend management support."}"#.to_owned()),
    };
    (
        StatusCode::from_u16(status).unwrap(),
        [
            ("content-type", "application/json"),
            ("cache-control", "no-store"),
        ],
        text,
    )
        .into_response()
}

#[tokio::test]
async fn forge_sign_in_is_exact_once_and_support_is_not_readiness() {
    for scenario in ["unsupported", "declared"] {
        let server = Server::new(scenario).await;
        let (front, mut presenter) = ForgeFrontend::new();
        assert_eq!(
            front.tool_mutation_class(FORGE_SELECT_HOST),
            ToolMutationClass::Mutating
        );
        let (result, presented) = timeout(BOUND, async {
            tokio::join!(request(&front, "first"), async {
                private_present(
                    presenter.next().await.unwrap(),
                    &server.host.origin,
                    PASSWORD,
                )
                .await
            })
        })
        .await
        .unwrap();
        assert!(presented.unwrap());
        let text = format!("{:?}", result.unwrap());
        assert!(text.contains(if scenario == "unsupported" {
            "unsupported"
        } else {
            "declared"
        }));
        if scenario == "unsupported" {
            assert!(!text.contains("host_id"));
        }
        for private in [
            PASSWORD,
            NONCE,
            SESSION,
            &server.host.origin,
            "private_uninterpreted_field",
        ] {
            assert!(!text.contains(private));
        }
        assert!(matches!(
            request(&front, "again").await,
            Err(ToolError::AccessDenied { .. })
        ));
        assert_eq!(
            server.finish().await,
            ["GET /login", "POST /login", "GET /console/forge/frontends"]
        );
    }
}

#[tokio::test]
async fn forge_refuses_bad_nonce_redirect_and_unrecognized_responses_without_retry() {
    for scenario in [
        "nonce",
        "secure-nonce",
        "secure-session",
        "redirect",
        "bad-support",
        "duplicate",
    ] {
        let server = Server::new(scenario).await;
        let (front, mut presenter) = ForgeFrontend::new();
        let (result, error) = timeout(BOUND, async {
            tokio::join!(request(&front, "first"), async {
                let prompt = presenter.next().await.unwrap();
                if matches!(scenario, "nonce" | "secure-nonce") {
                    let mut pty = Pty::new();
                    let mut out = Output::default();
                    let observed = out.clone();
                    let (error, ()) = tokio::join!(
                        present_forge_selection(
                            prompt,
                            &mut pty.input,
                            &mut out,
                            std::future::pending()
                        ),
                        async {
                            shown(&observed, "Forge Console origin").await;
                            send(&pty.master, &format!("{}\n", server.host.origin));
                        }
                    );
                    pty.restored();
                    assert!(!out.text().contains("password (hidden)"));
                    error
                } else {
                    private_present(prompt, &server.host.origin, PASSWORD).await
                }
            })
        })
        .await
        .unwrap();
        assert!(result.is_err());
        let error = error.unwrap_err().to_string();
        assert!(!error.contains("canary"));
        assert!(!error.contains(PASSWORD));
        assert!(request(&front, "retry").await.is_err());
        let calls = server.finish().await;
        assert_eq!(
            calls.iter().filter(|s| s.starts_with("POST")).count(),
            usize::from(!matches!(scenario, "nonce" | "secure-nonce"))
        );
        assert_eq!(
            calls.iter().filter(|s| s.contains("/console")).count(),
            usize::from(matches!(scenario, "bad-support" | "duplicate"))
        );
    }
}

type DirectToolResult = Result<meerkat_core::ops::ToolDispatchOutcome, ToolError>;

// These direct controls retain observations until their original loopback
// server is joined. Assertion or PTY-driver panic must not skip that cleanup.
struct DirectSelection {
    timed_out: bool,
    result: std::thread::Result<DirectToolResult>,
    presented: std::thread::Result<Result<bool, ConnectError>>,
    entered: std::thread::Result<()>,
    restored: std::thread::Result<()>,
    output: String,
}
impl DirectSelection {
    fn ready(&self) -> bool {
        !self.timed_out && matches!(&self.result, Ok(Ok(_)))
            && matches!(&self.presented, Ok(Ok(true)))
            && self.entered.is_ok() && self.restored.is_ok()
    }
    fn finish(self) -> (DirectToolResult, Result<bool, ConnectError>, String) {
        let result = self.result.unwrap_or_else(|panic| resume_unwind(panic));
        let presented = self.presented.unwrap_or_else(|panic| resume_unwind(panic));
        self.entered.unwrap_or_else(|panic| resume_unwind(panic));
        self.restored.unwrap_or_else(|panic| resume_unwind(panic));
        assert!(!self.timed_out, "direct Forge selection exceeded its observation bound; original work was joined");
        for private in [PASSWORD, NONCE, SESSION, "private refused body canary", "private response canary"] {
            assert!(!self.output.contains(private));
        }
        (result, presented, self.output)
    }
}

async fn observe_selection(
    front: &ForgeFrontend, presenter: &mut meerkat_connect::forge::ForgePresenter,
    origin: &str, id: &str, reauth: bool,
) -> DirectSelection {
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let arguments = raw("{}");
    let (stop, stopped) = oneshot::channel();
    let request_finished = Notify::new();
    let requesting = async {
        let result = AssertUnwindSafe(front.dispatch(ToolCallView {
            id, name:if reauth { FORGE_REAUTHENTICATE } else { FORGE_SELECT_HOST }, args:&arguments,
        })).catch_unwind().await;
        request_finished.notify_one();
        result
    };
    let presenting = AssertUnwindSafe(async {
        let interrupted = async { stopped.await.map_err(|_| io::Error::other("fixture stop source ended")) };
        tokio::pin!(interrupted);
        let prompt = tokio::select! { biased;
            _ = interrupted.as_mut() => {
                // Cancellation can win before this sole receiver takes the
                // queued selection. Drop that unentered prompt, or observe
                // that the original request already ended, before joining.
                tokio::select! { biased;
                    prompt = presenter.next() => drop(prompt),
                    _ = request_finished.notified() => {},
                }
                return Ok(false);
            },
            _ = request_finished.notified() => return Ok(false),
            prompt = presenter.next() => prompt,
        };
        match prompt {
            Some(prompt) => present_forge_selection(prompt, &mut pty.input, &mut output, interrupted.as_mut()).await,
            None => Err(ConnectError::InvalidInput("fixture presenter ended".into())),
        }
    }).catch_unwind();
    let entering = AssertUnwindSafe(async {
        if reauth {
            shown(&observed, "Forge Console password (hidden)").await;
            assert!(!observed.text().contains("Forge Console origin"));
            assert!(!tcgetattr(&pty.observer).unwrap().local_flags.contains(LocalFlags::ECHO));
            send(&pty.master, &format!("{PASSWORD}\n"));
        } else {
            enter(&pty.master, &pty.observer, &observed, origin, PASSWORD).await;
        }
    }).catch_unwind();
    let mut original = Box::pin(async { tokio::join!(requesting, presenting, entering) });
    let (timed_out, (result, presented, entered)) = match timeout(BOUND, original.as_mut()).await {
        Ok(result) => (false, result),
        Err(_) => {
            let _ = stop.send(());
            // Keep the actual presenter alive: it settles any entered POST
            // before returning. The original request waits on that handoff.
            (true, original.as_mut().await)
        }
    };
    drop(original);
    DirectSelection { timed_out, result, presented, entered,
        restored:catch_unwind(AssertUnwindSafe(|| pty.restored())), output:output.text() }
}

struct DirectRead {
    timed_out: bool,
    result: std::thread::Result<DirectToolResult>,
}
impl DirectRead {
    fn execution_refused(&self) -> bool {
        !self.timed_out && matches!(&self.result, Ok(Err(ToolError::ExecutionFailed { .. })))
    }
    fn finish(self) -> DirectToolResult {
        let result = self.result.unwrap_or_else(|panic| resume_unwind(panic));
        assert!(!self.timed_out, "direct Forge read/refusal exceeded its observation bound; original waiter was joined");
        result
    }
}
async fn observe_read_or_refusal(
    front: &ForgeFrontend, presenter: &mut meerkat_connect::forge::ForgePresenter,
    id: &str, name: &str,
) -> DirectRead {
    let arguments = raw("{}");
    let mut original = Box::pin(AssertUnwindSafe(front.dispatch(ToolCallView {
        id, name, args:&arguments,
    })).catch_unwind());
    let (timed_out, result) = match timeout(BOUND, original.as_mut()).await {
        Ok(result) => (false, result),
        Err(_) => {
            // A regression may reopen a prompt instead of refusing the retry.
            // Drop only that unpresented prompt, never begin another login,
            // and retain the original read/request future until it settles.
            let result = loop {
                tokio::select! { biased;
                    result = original.as_mut() => break result,
                    prompt = presenter.next() => drop(prompt),
                }
            };
            (true, result)
        }
    };
    DirectRead { timed_out, result }
}

async fn finish_direct_server(mut server: Server) -> Vec<String> {
    let stopped = server.stop.take().unwrap().send(());
    let (timed_out, finished) = match timeout(BOUND, &mut server.task).await {
        Ok(result) => (false, result),
        Err(_) => (true, server.task.await),
    };
    // Even a failed observation must not detach the server task.
    stopped.unwrap();
    finished.unwrap();
    assert!(!timed_out, "direct Forge server exceeded its observation bound; same task was joined");
    server.host.calls.lock().unwrap().clone()
}

// This exercises the real protected selection and original tool waiter. The
// loopback host is a protocol fixture, not a native session or a live login.
// Same-input model feedback and permitted siblings are separate controls.
async fn local_selection_refusal(scenario: &'static str, expected: &str) {
    let server = Server::new(scenario).await;
    let origin = server.host.origin.clone();
    let (front, mut presenter) = ForgeFrontend::new();
    let observed = AssertUnwindSafe(async {
        let selection = observe_selection(&front, &mut presenter, &origin, "first", false).await;
        // Refusal spends only this Forge selection. It must not reopen a
        // login prompt or retry the POST.
        let retry = observe_read_or_refusal(&front, &mut presenter, "retry", FORGE_SELECT_HOST).await;
        (selection, retry)
    }).catch_unwind().await;
    let calls = AssertUnwindSafe(finish_direct_server(server)).catch_unwind().await;
    let (selection, retry) = observed.unwrap_or_else(|panic| resume_unwind(panic));
    let (result, presented, _) = selection.finish();
    let retry = retry.finish();
    let calls = calls.unwrap_or_else(|panic| resume_unwind(panic));
    assert!(matches!(presented, Ok(true)), "ordinary refusal must keep setup available: {presented:?}");
    let error = match result {
        Err(ToolError::ExecutionFailed { message }) => message,
        other => panic!("the original tool must receive a safe execution refusal: {other:?}"),
    };
    assert!(error.contains(expected), "unexpected refusal: {error}");
    for private in [PASSWORD, NONCE, SESSION, origin.as_str(), "canary"] {
        assert!(!error.contains(private));
    }
    assert!(matches!(retry, Err(ToolError::AccessDenied { .. })),
        "a refused selection must not restore authority or request another sign-in");
    let expected_calls = if scenario == "expired" {
        vec!["GET /login", "POST /login", "GET /console/forge/frontends"]
    } else {
        vec!["GET /login", "POST /login"]
    };
    assert_eq!(calls, expected_calls, "exactly one login POST and no Forge prepare/submit");
}

#[tokio::test]
async fn forge_wrong_login_refusal_keeps_presenter_successful_without_retry() {
    for scenario in ["wrong", "wrong-forbidden", "wrong-rate-limited"] {
        local_selection_refusal(scenario, "Forge Console sign-in was refused").await;
    }
}

#[tokio::test]
async fn forge_expired_support_refusal_keeps_presenter_successful_without_retry() {
    local_selection_refusal("expired", "new protected interaction is required").await;
}

#[tokio::test]
async fn forge_reauthentication_login_refusal_keeps_same_origin_and_presenter_successful() {
    let server = Server::new("reauth-wrong").await;
    let origin = server.host.origin.clone();
    let (front, mut presenter) = ForgeFrontend::new();
    let observed = AssertUnwindSafe(async {
        let initial = observe_selection(&front, &mut presenter, &origin, "first", false).await;
        // A failed prerequisite stops later test actions, but its observation
        // is propagated only after terminal work and the server are joined.
        let expired = if initial.ready() {
            Some(observe_read_or_refusal(&front, &mut presenter, "expired", FORGE_INSPECT).await)
        } else { None };
        let reauth = if expired.as_ref().is_some_and(DirectRead::execution_refused) {
            Some(observe_selection(&front, &mut presenter, &origin, "reauth", true).await)
        } else { None };
        let retry = if reauth.is_some() {
            Some(observe_read_or_refusal(&front, &mut presenter, "reauth-retry", FORGE_REAUTHENTICATE).await)
        } else { None };
        (initial, expired, reauth, retry)
    }).catch_unwind().await;
    let calls = AssertUnwindSafe(finish_direct_server(server)).catch_unwind().await;
    let (initial, expired, reauth, retry) = observed.unwrap_or_else(|panic| resume_unwind(panic));
    let (selected, initial_presented, _) = initial.finish();
    assert!(selected.is_ok() && matches!(initial_presented, Ok(true)));
    let expired = expired.expect("successful initial selection reaches expiry readback").finish();
    assert!(matches!(expired, Err(ToolError::ExecutionFailed { .. })));
    let (result, presented, output) = reauth.expect("observed expiry reaches explicit reauthentication").finish();
    let retry = retry.expect("settled reauthentication reaches the exact refusal check").finish();
    let calls = calls.unwrap_or_else(|panic| resume_unwind(panic));
    assert!(matches!(presented, Ok(true)), "refused reauthentication is operation-local: {presented:?}");
    let error = match result {
        Err(ToolError::ExecutionFailed { message }) => message,
        other => panic!("original reauthentication waiter must receive refusal: {other:?}"),
    };
    assert!(error.contains("Forge Console sign-in was refused"));
    assert!(matches!(retry, Err(_)), "spent reauthentication must not start another prompt");
    for private in [PASSWORD, NONCE, SESSION, origin.as_str(), "canary"] {
        assert!(!error.contains(private));
        assert!(!output.contains(private));
    }
    assert!(!output.contains("Forge Console origin"));
    assert_eq!(calls, ["GET /login", "POST /login", "GET /console/forge/frontends",
        "GET /console/forge/frontends", "GET /login", "POST /login"],
        "one initial login and one explicitly requested same-host reauthentication, with no retry or Forge effect");
}

#[tokio::test]
async fn forge_hidden_password_cancellation_restores_input_and_never_posts() {
    for cancel in ["/quit\n", "\x04", "interrupt"] {
        let server = Server::new("unsupported").await;
        let (front, mut presenter) = ForgeFrontend::new();
        let mut pty = Pty::new();
        let mut out = Output::default();
        let observed = out.clone();
        let (signal, receive) = oneshot::channel();
        let (result, presented, ()) = timeout(BOUND, async {
            tokio::join!(
                request(&front, "first"),
                async {
                    present_forge_selection(
                        presenter.next().await.unwrap(),
                        &mut pty.input,
                        &mut out,
                        async {
                            match receive.await {
                                Ok(()) => Ok(()),
                                Err(_) => std::future::pending().await,
                            }
                        },
                    )
                    .await
                },
                async {
                    shown(&observed, "Forge Console origin").await;
                    send(&pty.master, &format!("{}\n", server.host.origin));
                    shown(&observed, "password (hidden)").await;
                    assert!(
                        !tcgetattr(&pty.observer)
                            .unwrap()
                            .local_flags
                            .contains(LocalFlags::ECHO)
                    );
                    if cancel == "interrupt" {
                        signal.send(()).unwrap();
                    } else {
                        send(&pty.master, cancel);
                    }
                }
            )
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(!presented.unwrap());
        pty.restored();
        assert_eq!(server.finish().await, ["GET /login"]);
    }
}

#[tokio::test]
async fn forge_cancelled_post_is_joined_without_probe_or_replay() {
    let server = Server::new("held").await;
    let (front, mut presenter) = ForgeFrontend::new();
    let mut pty = Pty::new();
    let mut out = Output::default();
    let observed = out.clone();
    let (signal, receive) = oneshot::channel();
    let (result, presented, ()) = timeout(BOUND, async {
        tokio::join!(
            request(&front, "first"),
            async {
                present_forge_selection(
                    presenter.next().await.unwrap(),
                    &mut pty.input,
                    &mut out,
                    async {
                        match receive.await {
                            Ok(()) => Ok(()),
                            Err(_) => std::future::pending().await,
                        }
                    },
                )
                .await
            },
            async {
                enter(
                    &pty.master,
                    &pty.observer,
                    &observed,
                    &server.host.origin,
                    PASSWORD,
                )
                .await;
                server.host.entered.notified().await;
                signal.send(()).unwrap();
                shown(&observed, "Waiting for the Forge sign-in request to settle").await;
                assert!(request(&front, "parallel").await.is_err());
                assert_eq!(server.host.calls.lock().unwrap().len(), 2);
                server.host.release.notify_one();
            }
        )
    })
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(!presented.unwrap());
    pty.restored();
    assert_eq!(server.finish().await, ["GET /login", "POST /login"]);
}

#[tokio::test]
async fn forge_model_arguments_never_select_a_host_or_supply_credentials() {
    let (front, _) = ForgeFrontend::new();
    for value in [
        r#"{"origin":"http://127.0.0.1:9"}"#,
        r#"{"password":"private"}"#,
        "null",
        "[]",
    ] {
        assert!(
            front
                .dispatch(ToolCallView {
                    id: "forged",
                    name: FORGE_SELECT_HOST,
                    args: &raw(value)
                })
                .await
                .is_err()
        );
    }
    assert!(
        front
            .dispatch(ToolCallView {
                id: "forged",
                name: "forge_submit",
                args: &raw("{}")
            })
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires CONNECT_FORGE_TEST_PYTHON with the existing Forge Starlette/httpx/uvicorn/PyJWT environment"]
async fn forge_frozen_auth_owner_accepts_private_unicode_password_then_reports_unsupported() {
    let mut owner = forge_owner::Owner::start().await;
    let (front, mut presenter) = ForgeFrontend::new();
    let result = timeout(BOUND, async {
        tokio::join!(request(&front, "real-owner"), async {
            private_present(
                presenter.next().await.unwrap(),
                &owner.origin,
                forge_owner::PASSWORD,
            )
            .await
        })
    })
    .await;
    owner.finish().await;
    let (result, presented) = result.unwrap();
    assert!(presented.unwrap());
    assert!(format!("{:?}", result.unwrap()).contains("unsupported"));
    assert!(request(&front, "retry").await.is_err());
    println!(
        "Frozen Forge auth owner: GET login 200, one POST 303, support GET 501 before host identity; hidden Unicode/whitespace preserved; server joined."
    );
}

#[tokio::test]
async fn forge_caller_read_only_policy_refuses_login_before_private_prompt() {
    use meerkat_core::{
        ExecutionPolicyGatedDispatcher, ToolExecutionPolicy, ops::ToolAccessPolicy,
    };
    let (front, mut presenter) = ForgeFrontend::new();
    let policy = ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly).unwrap();
    let gated = ExecutionPolicyGatedDispatcher::new(Arc::new(front), policy);
    assert!(matches!(
        gated
            .dispatch(ToolCallView {
                id: "read-only",
                name: FORGE_SELECT_HOST,
                args: &raw("{}")
            })
            .await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert!(
        timeout(Duration::from_millis(20), presenter.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn forge_local_password_correction_keeps_one_hidden_guard_and_one_post() {
    let server = Server::new("unsupported").await;
    let (front, mut presenter) = ForgeFrontend::new();
    let mut pty = Pty::new();
    let mut out = Output::default();
    let observed = out.clone();
    let (result, presented, ()) = timeout(BOUND, async {
        tokio::join!(
            request(&front, "first"),
            async {
                present_forge_selection(
                    presenter.next().await.unwrap(),
                    &mut pty.input,
                    &mut out,
                    std::future::pending(),
                )
                .await
            },
            async {
                enter(
                    &pty.master,
                    &pty.observer,
                    &observed,
                    &server.host.origin,
                    "short",
                )
                .await;
                shown(&observed, "12-4096 UTF-8 characters").await;
                assert!(
                    !tcgetattr(&pty.observer)
                        .unwrap()
                        .local_flags
                        .contains(LocalFlags::ECHO)
                );
                send(&pty.master, &format!("{PASSWORD}\n"));
            }
        )
    })
    .await
    .unwrap();
    assert!(result.is_ok());
    assert!(presented.unwrap());
    pty.restored();
    assert!(!out.text().contains(PASSWORD));
    assert_eq!(
        server.finish().await,
        ["GET /login", "POST /login", "GET /console/forge/frontends"]
    );
}

#[tokio::test]
async fn forge_long_multibyte_password_and_editing_preserve_bytes_and_discard_typeahead() {
    let password = format!(" \t{}  ", "密".repeat(4092));
    assert_eq!(password.chars().count(), 4096);
    let server = Server::with_password("unsupported", password.clone()).await;
    let (front, mut presenter) = ForgeFrontend::new();
    let mut pty = Pty::new();
    let mut out = Output::default();
    let observed = out.clone();
    let (result, presented, ()) = timeout(BOUND, async {
        tokio::join!(
            request(&front, "long"),
            async {
                present_forge_selection(
                    presenter.next().await.unwrap(),
                    &mut pty.input,
                    &mut out,
                    std::future::pending(),
                )
                .await
            },
            async {
                shown(&observed, "Forge Console origin").await;
                send(&pty.master, &format!("{}\n", server.host.origin));
                shown(&observed, "Forge Console password (hidden)").await;
                let mode = tcgetattr(&pty.observer).unwrap();
                assert!(
                    !mode
                        .local_flags
                        .intersects(LocalFlags::ECHO | LocalFlags::ICANON)
                );
                // Chunk only the test writer so a full PTY output queue cannot block
                // the single-threaded test executor while the actual reader drains it.
                for bytes in password.as_bytes().chunks(128) {
                    assert_eq!(write(&pty.master, bytes).unwrap(), bytes.len());
                    sleep(Duration::from_millis(1)).await;
                }
                send(&pty.master, "错\u{7f}\nprivate-typeahead\n");
            }
        )
    })
    .await
    .unwrap();
    assert!(presented.unwrap());
    assert!(result.is_ok());
    pty.restored();
    assert!(
        timeout(Duration::from_millis(20), pty.input.recv())
            .await
            .is_err()
    );
    assert!(!out.text().contains(&password));
    assert!(!out.text().contains("private-typeahead"));
    assert_eq!(
        server.finish().await,
        ["GET /login", "POST /login", "GET /console/forge/frontends"]
    );
}
