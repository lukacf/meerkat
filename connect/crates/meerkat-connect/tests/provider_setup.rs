//! Public configured-provider presentation with an isolated native file store.
//! OAuth endpoints are loopback fixtures. No real account, model invocation or
//! OS credential store is used.

use std::{
    io::{self, Write},
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use meerkat_connect::{
    RealmOptions, SessionOptions, SessionStartup, approval::LocalApprovals,
    paired_host_tool::PairedHostFrontend, provider_setup::present_provider_setup,
    service_preparation::LazySetup, terminal::TerminalInput,
};
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    pty::openpty,
    sys::termios::{Termios, tcgetattr},
    unistd::write,
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{sleep, timeout},
};

const BOUND: Duration = Duration::from_secs(10);
const CHILD: &str = "CONNECT_PROVIDER_SETUP_TEST_CHILD";
const ROOT: &str = "CONNECT_PROVIDER_SETUP_TEST_ROOT";
const READY_KEY: &str = "synthetic-provider-readiness-value";
const OAUTH_ORIGIN: &str = "CONNECT_PROVIDER_OAUTH_ORIGIN";
const CODE: &str = "synthetic-browser-code";
const ACCESS: &str = "synthetic-browser-access";
const REFRESH: &str = "synthetic-browser-refresh";
const MANAGED_KEY: &str = "synthetic-hidden-managed-api-key";

#[derive(Clone, Copy)]
enum BrowserProfile {
    OpenAi,
    Google,
}

impl BrowserProfile {
    fn alias(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Google => "google",
        }
    }

    fn provider(self) -> meerkat_core::Provider {
        match self {
            Self::OpenAi => meerkat_core::Provider::OpenAI,
            Self::Google => meerkat_core::Provider::Gemini,
        }
    }

    fn check_redirect(self, redirect: &reqwest::Url) {
        assert_eq!(redirect.scheme(), "http");
        assert_eq!(redirect.username(), "");
        assert!(redirect.password().is_none());
        assert!(redirect.query().is_none());
        assert!(redirect.fragment().is_none());
        match self {
            Self::OpenAi => {
                assert_eq!(redirect.host_str(), Some("localhost"));
                assert_eq!(redirect.path(), "/auth/callback");
                assert!(matches!(redirect.port(), Some(1455 | 1457)));
            }
            Self::Google => {
                assert_eq!(redirect.host_str(), Some("127.0.0.1"));
                assert_eq!(redirect.path(), "/callback");
                assert!(redirect.port().is_some_and(|port| port > 0));
            }
        }
    }
}

struct OAuthFixture {
    origin: String,
    calls: Arc<AtomicUsize>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<io::Result<()>>>,
}

impl OAuthFixture {
    async fn start(profile: BrowserProfile) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
            let calls = observed.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.method(), "POST");
                assert_eq!(request.uri().path(), format!("/{}/token", profile.alias()));
                assert!(request.uri().query().is_none());
                assert!(!request.headers().contains_key("cookie"));
                assert!(
                    request.headers()["content-type"]
                        .to_str()
                        .unwrap()
                        .starts_with("application/x-www-form-urlencoded")
                );
                let body = axum::body::to_bytes(request.into_body(), 8192)
                    .await
                    .expect("bounded synthetic exchange");
                let mut form = reqwest::Url::parse("http://fixture.invalid/").unwrap();
                form.set_query(Some(std::str::from_utf8(&body).expect("form UTF-8")));
                let fields = form
                    .query_pairs()
                    .into_owned()
                    .collect::<std::collections::BTreeMap<_, _>>();
                // Use value-free predicates for every private request field.
                assert!(
                    fields
                        .get("grant_type")
                        .is_some_and(|value| value == "authorization_code")
                );
                assert!(fields.get("code").is_some_and(|value| value == CODE));
                assert!(
                    fields
                        .get("code_verifier")
                        .is_some_and(|value| value.len() >= 43)
                );
                assert!(
                    fields
                        .get("client_id")
                        .is_some_and(|value| !value.is_empty())
                );
                assert!(!fields.contains_key("state"));
                if matches!(profile, BrowserProfile::Google) {
                    assert!(
                        fields
                            .get("client_secret")
                            .is_some_and(|value| !value.is_empty())
                    );
                }
                let redirect = fields.get("redirect_uri").expect("native redirect field");
                profile
                    .check_redirect(&reqwest::Url::parse(redirect).expect("native redirect URL"));
                axum::Json(serde_json::json!({
                    "access_token":ACCESS,"refresh_token":REFRESH,
                    "token_type":"Bearer","expires_in":3600,
                    "scope":"openid email profile"
                }))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = received.await;
                })
                .await
        });
        Self {
            origin,
            calls,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    async fn finish(mut self) -> usize {
        self.shutdown.take().unwrap().send(()).unwrap();
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.calls.load(Ordering::SeqCst)
    }
}

impl Drop for OAuthFixture {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Every native default-store access takes place only in this child process.
/// Never change the process-wide environment of the concurrent test runner.
async fn isolated(name: &str) -> bool {
    isolated_for(name, None).await
}

async fn isolated_for(name: &str, profile: Option<BrowserProfile>) -> bool {
    isolated_with(name, profile, None).await
}

async fn isolated_device(name: &str, case: DeviceCase) -> bool {
    isolated_with(name, None, Some(case)).await
}

async fn isolated_with(
    name: &str,
    profile: Option<BrowserProfile>,
    device_case: Option<DeviceCase>,
) -> bool {
    if std::env::var(CHILD).as_deref() == Ok(name) {
        let root = PathBuf::from(std::env::var_os(ROOT).expect("isolated root"));
        assert_eq!(
            PathBuf::from(std::env::var_os("HOME").unwrap()),
            root.join("home")
        );
        assert_eq!(
            PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()),
            root.join("config")
        );
        assert_eq!(std::env::current_dir().unwrap(), root);
        return true;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for name in ["home", "config", "data", "cache", "tmp"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    let oauth = match profile {
        Some(profile) => Some(OAuthFixture::start(profile).await),
        None => None,
    };
    let device = match device_case {
        Some(case) => Some(DeviceFixture::start(case).await),
        None => None,
    };
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", name, "--nocapture"]).env_clear();
    if cfg!(debug_assertions) {
        // Give only this owned debug-test child the explicit stack allowance.
        // Production stack qualification remains separate from this fixture.
        command.env("RUST_MIN_STACK", "8388608");
    }
    if let Some(oauth) = &oauth {
        command
            .env("MEERKAT_TEST_OAUTH_ENDPOINT_OVERRIDE", "1")
            .env("MEERKAT_TEST_OAUTH_BASE_URL", &oauth.origin)
            .env(OAUTH_ORIGIN, &oauth.origin);
    }
    if let Some(device) = &device {
        command
            .env("MEERKAT_TEST_OAUTH_ENDPOINT_OVERRIDE", "1")
            .env("MEERKAT_TEST_OAUTH_BASE_URL", &device.origin);
    }
    let mut child = command
        .env(CHILD, name)
        .env(ROOT, &root)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("TMPDIR", root.join("tmp"))
        .env("CONNECT_PROVIDER_READY_KEY", READY_KEY)
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = tokio::spawn(capture(stdout));
    let err = tokio::spawn(capture(stderr));
    let status = match timeout(Duration::from_secs(25), child.wait()).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            child.kill().await.unwrap();
            child.wait().await.unwrap();
            out.await.unwrap();
            err.await.unwrap();
            panic!("isolated provider presenter exceeded its bound");
        }
    };
    let out = out.await.unwrap();
    let err = err.await.unwrap();
    let calls = match oauth {
        Some(oauth) => Some(oauth.finish().await),
        None => None,
    };
    if let Some(device) = device {
        device.finish().await;
    }
    assert!(
        status.success(),
        "isolated presenter failed:\n{}\n{}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&err)
    );
    if let Some(calls) = calls {
        assert_eq!(
            calls, 1,
            "one token exchange, no model or project discovery request"
        );
    }
    false
}

async fn capture(stream: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    let mut bytes = Vec::new();
    stream
        .take(32 * 1024)
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    bytes
}

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
    initial: Termios,
    flags: i32,
    input: TerminalInput,
}
impl Pty {
    fn new() -> Self {
        let pair = openpty(None, None).unwrap();
        let observer = pair.slave.try_clone().unwrap();
        Self {
            initial: tcgetattr(&observer).unwrap(),
            flags: fcntl(&observer, FcntlArg::F_GETFL).unwrap(),
            observer,
            master: pair.master,
            input: TerminalInput::from_owned_fd(pair.slave),
        }
    }
    fn restored(&self) {
        assert_eq!(tcgetattr(&self.observer).unwrap(), self.initial);
        assert_eq!(
            fcntl(&self.observer, FcntlArg::F_GETFL).unwrap(),
            self.flags
        );
        self.input.check_cleanup().unwrap();
    }
}

fn configured(storage: &Path, realm: &str, key: &str) -> Vec<u8> {
    let text = format!(
        r#"[agent]
model = "gpt-6-astra"
[realm.{realm}]
default_binding = "configured"
[realm.{realm}.backend.configured]
provider = "openai"
backend_kind = "openai_api"
[realm.{realm}.auth.configured]
provider = "openai"
auth_method = "api_key"
source = {{ kind = "env", env = "{key}" }}
[realm.{realm}.binding.configured]
backend_profile = "configured"
auth_profile = "configured"
default_model = "gpt-6-astra"
"#
    );
    meerkat_core::Config::from_persisted_toml(&text)
        .unwrap()
        .0
        .validate(meerkat_models::canonical())
        .unwrap();
    std::fs::create_dir_all(storage.join(realm)).unwrap();
    std::fs::write(storage.join(realm).join("config.toml"), &text).unwrap();
    text.into_bytes()
}

fn storage(root: &Path, realm: &str) -> meerkat_core::ResolvedStorage {
    RealmOptions {
        context_root: root.to_owned(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    }
    .resolve(realm)
    .unwrap()
}

fn unchanged(root: &Path, realm: &str, config: &[u8]) {
    let realm = root.join("state").join(realm);
    assert_eq!(std::fs::read(realm.join("config.toml")).unwrap(), config);
    let mut names = std::fs::read_dir(&realm)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        [std::ffi::OsString::from("config.toml")],
        "read-only selection must not initialize native realm persistence"
    );
}

async fn waiting_for_input(observer: &OwnedFd) {
    timeout(BOUND, async {
        // Ordinary recv preserves canonical mode. Its owned read changes the
        // shared descriptor to nonblocking until completion or cancellation.
        while !OFlag::from_bits_retain(fcntl(observer, FcntlArg::F_GETFL).unwrap())
            .contains(OFlag::O_NONBLOCK)
        {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn ready_configured_provider_needs_no_prompt_or_realm_materialization() {
    if !isolated("ready_configured_provider_needs_no_prompt_or_realm_materialization").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let config = configured(&root.join("state"), "work", "CONNECT_PROVIDER_READY_KEY");
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    unchanged(&root, "work", &config);
    let mut pty = Pty::new();
    let mut output = Output::default();
    let request = timeout(
        BOUND,
        present_provider_setup(
            &mut startup,
            None,
            false,
            &mut pty.input,
            &mut output,
            std::future::pending(),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    let effective = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let resolved = meerkat::resolve_create_session_model(&effective, request.clone()).unwrap();
    assert_eq!(resolved.model, "gpt-6-astra");
    assert_eq!(resolved.provider, meerkat_core::Provider::OpenAI);
    assert_eq!(request.model.as_deref(), Some(resolved.model.as_str()));
    assert_eq!(request.provider, Some(resolved.provider));
    assert!(
        request.auth_binding.is_none(),
        "a ready native candidate is not an explicit binding selection"
    );
    // Exercise the actual public-presenter return that main retains for the
    // same setup conversation after finalization refreshes configuration.
    let mut refreshed = effective.clone();
    refreshed.agent.model = "claude-sonnet-4-5".into();
    let drifted = meerkat::resolve_create_session_model(&refreshed, Default::default()).unwrap();
    assert_eq!(drifted.model, "claude-sonnet-4-5");
    assert_eq!(drifted.provider, meerkat_core::Provider::Anthropic);
    let continued = meerkat::resolve_create_session_model(&refreshed, request).unwrap();
    assert_eq!(continued.model, resolved.model);
    assert_eq!(continued.provider, resolved.provider);
    assert!(continued.auth_binding.is_none());
    assert!(!output.text().contains(READY_KEY));
    assert!(!output.text().contains("http"));
    unchanged(&root, "work", &config);
    pty.restored();
}

#[tokio::test]
async fn explicit_account_menu_quit_and_interrupt_preserve_realm_and_input() {
    if !isolated("explicit_account_menu_quit_and_interrupt_preserve_realm_and_input").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let config = configured(&root.join("state"), "work", "CONNECT_PROVIDER_READY_KEY");
    for interrupted in [false, true] {
        let mut startup = SessionStartup::open(storage(&root, "work")).await.unwrap();
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let (sender, received) = tokio::sync::oneshot::channel();
        let human = async {
            waiting_for_input(&pty.observer).await;
            assert!(
                !observed.text().is_empty(),
                "selection must be presented to the human"
            );
            if interrupted {
                sender.send(()).unwrap();
            } else {
                assert_eq!(write(&pty.master, b"/quit\n").unwrap(), 6);
                // Retain the interrupt sender until the presenter returns.
                std::future::pending::<()>().await;
            }
        };
        tokio::pin!(human);
        let interrupt = async {
            received
                .await
                .map_err(|_| io::Error::other("test interrupt sender closed"))
        };
        let result = {
            let present = present_provider_setup(
                &mut startup,
                None,
                true,
                &mut pty.input,
                &mut output,
                interrupt,
            );
            tokio::pin!(present);
            timeout(BOUND, async {
                tokio::select! {
                    result = &mut present => result,
                    () = &mut human => present.await,
                }
            })
            .await
            .unwrap()
            .unwrap()
        };
        assert!(result.is_none());
        assert!(!output.text().contains(READY_KEY));
        unchanged(&root, "work", &config);
        pty.restored();
    }
}

#[tokio::test]
async fn inherited_account_selection_keeps_native_owner_and_child_realm() {
    if !isolated("inherited_account_selection_keeps_native_owner_and_child_realm").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let parent = configured(&root.join("state"), "owner", "CONNECT_PROVIDER_READY_KEY");
    let child = b"# Child-owned raw document must remain byte-for-byte intact.\n[realm.work]\nparent = \"owner\"\n";
    std::fs::create_dir_all(root.join("state/work")).unwrap();
    std::fs::write(root.join("state/work/config.toml"), child).unwrap();
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        waiting_for_input(&pty.observer).await;
        assert!(
            observed
                .text()
                .contains("1. Owner realm: owner; binding: configured")
        );
        assert_eq!(write(&pty.master, b"1\n").unwrap(), 2);
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                None,
                true,
                &mut pty.input,
                &mut output,
                std::future::pending(),
            ),
            human
        )
    })
    .await
    .unwrap();
    let request = result.unwrap().unwrap();
    let reference = request
        .auth_binding
        .as_ref()
        .expect("explicit selected target");
    assert_eq!(reference.realm.as_str(), "owner");
    assert_eq!(reference.binding.as_str(), "configured");
    assert!(reference.profile.is_none());
    assert_eq!(selected.locator.realm.as_str(), "work");
    let config = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let native = meerkat::resolve_create_session_model(&config, request).unwrap();
    assert_eq!(native.auth_binding.unwrap().realm.as_str(), "owner");
    assert_eq!(native.model, "gpt-6-astra");
    assert!(!output.text().contains(READY_KEY));
    unchanged(&root, "work", child);
    unchanged(&root, "owner", &parent);
    pty.restored();
}

fn configured_model_choices(root: &Path) -> (Vec<u8>, Vec<u8>) {
    let mut parent = configured(&root.join("state"), "owner", "CONNECT_PROVIDER_READY_KEY");
    parent.extend_from_slice(
        br#"
[realm.owner.backend.incompatible]
provider = "anthropic"
backend_kind = "anthropic_api"
[realm.owner.auth.incompatible]
provider = "anthropic"
auth_method = "api_key"
source = {kind = "env", env = "CONNECT_PROVIDER_ABSENT_KEY"}
[realm.owner.binding.a_incompatible]
backend_profile = "incompatible"
auth_profile = "incompatible"
default_model = "claude-opus-5-5"
"#,
    );
    meerkat_core::Config::from_persisted_toml(std::str::from_utf8(&parent).unwrap())
        .unwrap()
        .0
        .validate(meerkat_models::canonical())
        .unwrap();
    std::fs::write(root.join("state/owner/config.toml"), &parent).unwrap();
    let child =
        b"# Preserve the caller's realm document.\n[realm.work]\nparent = \"owner\"\n".to_vec();
    std::fs::create_dir_all(root.join("state/work")).unwrap();
    std::fs::write(root.join("state/work/config.toml"), &child).unwrap();
    (parent, child)
}

#[tokio::test]
async fn explicit_model_mismatch_recovery_preserves_selection_and_original_startup() {
    if !isolated("explicit_model_mismatch_recovery_preserves_selection_and_original_startup").await
    {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let (parent, child) = configured_model_choices(&root);
    let selected = storage(&root, "work");
    let effective = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let wrong = meerkat::CreateSessionModelResolutionRequest {
        model: Some("gpt-6-astra".into()),
        auth_binding: Some(meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("owner").unwrap(),
            binding: meerkat_core::BindingId::parse("a_incompatible").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }),
        ..Default::default()
    };
    assert!(
        matches!(
            meerkat::resolve_create_session_model(&effective, wrong),
            Err(meerkat::CreateSessionModelResolutionError::ProviderModelMismatch(_))
        ),
        "native resolution still refuses the incompatible choice"
    );
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let second_choice_written = AtomicUsize::new(0);
    let human = async {
        waiting_for_input(&pty.observer).await;
        assert!(
            observed
                .text()
                .contains("1. Owner realm: owner; binding: a_incompatible")
        );
        assert!(
            observed
                .text()
                .contains("2. Owner realm: owner; binding: configured")
        );
        write(&pty.master, b"1\n").unwrap();
        while observed
            .text()
            .matches("Model target (number, or /quit): ")
            .count()
            < 2
        {
            sleep(Duration::from_millis(2)).await;
        }
        unchanged(&root, "owner", &parent);
        unchanged(&root, "work", &child);
        assert!(
            !observed.text().contains("status:"),
            "mismatched target must not reach credential observation"
        );
        assert!(!observed.text().contains("http"));
        assert!(!observed.text().contains(READY_KEY));
        assert_eq!(write(&pty.master, b"2\n").unwrap(), 2);
        second_choice_written.store(1, Ordering::SeqCst);
    };
    tokio::pin!(human);
    let result = {
        let present = present_provider_setup(
            &mut startup,
            Some("gpt-6-astra".into()),
            true,
            &mut pty.input,
            &mut output,
            std::future::pending(),
        );
        tokio::pin!(present);
        timeout(BOUND, async {
            tokio::select! {
                result = &mut present => result,
                () = &mut human => present.await,
            }
        })
        .await
        .unwrap()
    };
    let request = result
        .expect("an incompatible choice should return to the existing picker")
        .expect("the compatible target was explicitly selected");
    assert_eq!(
        second_choice_written.load(Ordering::SeqCst),
        1,
        "the presenter must wait for the human to select the compatible target"
    );
    assert_eq!(request.model.as_deref(), Some("gpt-6-astra"));
    let reference = request.auth_binding.as_ref().unwrap();
    assert_eq!(reference.realm.as_str(), "owner");
    assert_eq!(reference.binding.as_str(), "configured");
    assert_eq!(selected.locator.realm.as_str(), "work");
    let native = meerkat::resolve_create_session_model(&effective, request.clone()).unwrap();
    assert_eq!(native.model, "gpt-6-astra");
    assert_eq!(native.provider, meerkat_core::Provider::OpenAI);
    assert_eq!(
        observed
            .text()
            .matches("Model target (number, or /quit): ")
            .count(),
        2
    );
    assert!(!observed.text().contains(READY_KEY));
    unchanged(&root, "owner", &parent);
    unchanged(&root, "work", &child);
    pty.restored();
    // Consume the original startup after correction, without a second auth
    // owner, PairedHost pairing, setup tool, model request or credential write.
    let (frontend, _host) = PairedHostFrontend::deferred();
    let (lazy, _service) = LazySetup::new(frontend);
    let (approvals, _decisions) = LocalApprovals::create_paired_host(&root).unwrap();
    let mut options = lazy
        .session_options(SessionOptions {
            model: request,
            ..Default::default()
        })
        .unwrap();
    approvals.attach_lazy_setup(&mut options, &lazy).unwrap();
    let session = timeout(BOUND, Box::pin(startup.open_lazy_setup(options, &lazy)))
        .await
        .unwrap()
        .unwrap();
    timeout(BOUND, session.close()).await.unwrap().unwrap();
    assert_eq!(
        std::fs::read(root.join("state/owner/config.toml")).unwrap(),
        parent
    );
    assert_eq!(
        std::fs::read(root.join("state/work/config.toml")).unwrap(),
        child
    );
}

#[tokio::test]
async fn explicit_model_mismatch_recovery_exit_keeps_realm_and_credentials_untouched() {
    if !isolated("explicit_model_mismatch_recovery_exit_keeps_realm_and_credentials_untouched")
        .await
    {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let (parent, child) = configured_model_choices(&root);
    for action in ["quit", "eof", "interrupt"] {
        let mut startup = SessionStartup::open(storage(&root, "work")).await.unwrap();
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let (sender, received) = tokio::sync::oneshot::channel();
        let human = async {
            waiting_for_input(&pty.observer).await;
            write(&pty.master, b"1\n").unwrap();
            while observed
                .text()
                .matches("Model target (number, or /quit): ")
                .count()
                < 2
            {
                sleep(Duration::from_millis(2)).await;
            }
            match action {
                "quit" => {
                    write(&pty.master, b"/quit\n").unwrap();
                }
                "eof" => {
                    write(&pty.master, b"\x04").unwrap();
                }
                "interrupt" => {
                    sender.send(()).unwrap();
                    return;
                }
                _ => unreachable!(),
            }
            // Keep the cancellation sender alive until the input exit settles.
            std::future::pending::<()>().await;
        };
        tokio::pin!(human);
        let result = {
            let present = present_provider_setup(
                &mut startup,
                Some("gpt-6-astra".into()),
                true,
                &mut pty.input,
                &mut output,
                async {
                    received
                        .await
                        .map_err(|_| io::Error::other("fixture interrupt sender closed"))
                },
            );
            tokio::pin!(present);
            timeout(BOUND, async {
                tokio::select! {
                    result = &mut present => result,
                    () = &mut human => present.await,
                }
            })
            .await
            .unwrap()
        };
        assert!(
            result
                .expect("the correction picker should permit an ordinary exit")
                .is_none()
        );
        assert_eq!(
            output
                .text()
                .matches("Model target (number, or /quit): ")
                .count(),
            2
        );
        assert!(!output.text().contains("status:"));
        assert!(!output.text().contains("http"));
        assert!(!output.text().contains(READY_KEY));
        unchanged(&root, "owner", &parent);
        unchanged(&root, "work", &child);
        pty.restored();
    }
}

#[tokio::test]
async fn configured_model_mismatch_is_not_a_recoverable_choice() {
    if !isolated("configured_model_mismatch_is_not_a_recoverable_choice").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let (parent, child) = configured_model_choices(&root);
    let parent = String::from_utf8(parent)
        .unwrap()
        .replace(
            "default_model = \"claude-opus-5-5\"",
            "default_model = \"gpt-6-astra\"",
        )
        .into_bytes();
    std::fs::write(root.join("state/owner/config.toml"), &parent).unwrap();
    let selected = storage(&root, "work");
    let effective = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let request = meerkat::CreateSessionModelResolutionRequest {
        auth_binding: Some(meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("owner").unwrap(),
            binding: meerkat_core::BindingId::parse("a_incompatible").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }),
        ..Default::default()
    };
    assert!(matches!(
        meerkat::resolve_create_session_model(&effective, request),
        Err(meerkat::CreateSessionModelResolutionError::ConfiguredProviderModelMismatch(_))
    ));
    let mut startup = SessionStartup::open(selected).await.unwrap();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let choice_written = AtomicUsize::new(0);
    let human = async {
        waiting_for_input(&pty.observer).await;
        assert_eq!(write(&pty.master, b"1\n").unwrap(), 2);
        choice_written.store(1, Ordering::SeqCst);
    };
    tokio::pin!(human);
    let result = {
        let present = present_provider_setup(
            &mut startup,
            None,
            true,
            &mut pty.input,
            &mut output,
            std::future::pending(),
        );
        tokio::pin!(present);
        timeout(BOUND, async {
            tokio::select! {
                result = &mut present => result,
                () = &mut human => present.await,
            }
        })
        .await
        .unwrap()
    };
    let error = result.expect_err("configured model faults must remain fatal");
    assert_eq!(choice_written.load(Ordering::SeqCst), 1);
    let meerkat_connect::ConnectError::Native { operation, source } = error else {
        panic!("configuration fault must retain its native error source")
    };
    assert_eq!(operation, "Resolve chosen native model target");
    assert!(matches!(
        source.downcast_ref::<meerkat::CreateSessionModelResolutionError>(),
        Some(meerkat::CreateSessionModelResolutionError::ConfiguredProviderModelMismatch(_))
    ));
    assert_eq!(
        output
            .text()
            .matches("Model target (number, or /quit): ")
            .count(),
        1
    );
    assert!(!output.text().contains("status:"));
    assert!(!output.text().contains("http"));
    assert!(!output.text().contains(READY_KEY));
    unchanged(&root, "owner", &parent);
    unchanged(&root, "work", &child);
    pty.restored();
}

fn configured_browser(root: &Path, profile: BrowserProfile, origin: &str) -> Vec<u8> {
    let provider = profile.provider().as_str();
    let model = meerkat_models::default_model(profile.provider()).unwrap();
    let (backend, method, options) = match profile {
        BrowserProfile::OpenAi => ("chatgpt_backend", "managed_chatgpt_oauth", ""),
        BrowserProfile::Google => (
            "google_code_assist",
            "google_oauth",
            "options = {project_id = \"synthetic-project\"}\n",
        ),
    };
    let text = format!(
        r#"[agent]
model = "{model}"
[realm.work]
default_binding = "browser"
[realm.work.backend.browser]
provider = "{provider}"
backend_kind = "{backend}"
base_url = "{origin}"
{options}[realm.work.auth.browser]
provider = "{provider}"
auth_method = "{method}"
source = {{kind = "managed_store"}}
[realm.work.binding.browser]
backend_profile = "browser"
auth_profile = "browser"
default_model = "{model}"
"#
    );
    meerkat_core::Config::from_persisted_toml(&text)
        .unwrap()
        .0
        .validate(meerkat_models::canonical())
        .unwrap();
    std::fs::create_dir_all(root.join("state/work")).unwrap();
    std::fs::write(root.join("state/work/config.toml"), &text).unwrap();
    text.into_bytes()
}

async fn shown(output: &Output, text: &str) {
    timeout(BOUND, async {
        while !output.text().contains(text) {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("protected presenter progress; private transcript withheld");
}

struct BrowserOutput {
    captured: Output,
    cancel_on_completion: bool,
    interrupt: Option<tokio::sync::oneshot::Sender<()>>,
}

impl BrowserOutput {
    fn text(&self) -> String {
        self.captured.text()
    }
}

impl Write for BrowserOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.captured.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.captured.flush()?;
        if self.cancel_on_completion
            && self
                .text()
                .contains("Native sign-in completed for the selected owner")
            && let Some(interrupt) = self.interrupt.take()
        {
            interrupt.send(()).map_err(|_| {
                io::Error::other("presenter dropped interruption before completion output")
            })?;
        }
        Ok(())
    }
}

async fn browser_login(profile: BrowserProfile, cancel_on_completion: bool) {
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let origin = std::env::var(OAUTH_ORIGIN).unwrap();
    let config = configured_browser(&root, profile, &origin);
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    unchanged(&root, "work", &config);
    let mut pty = Pty::new();
    let (interrupt_tx, interrupt_rx) = tokio::sync::oneshot::channel();
    let mut output = BrowserOutput {
        captured: Output::default(),
        cancel_on_completion,
        interrupt: Some(interrupt_tx),
    };
    let observed = output.captured.clone();
    let human = async {
        shown(&observed, "Model target (number, or /quit): ").await;
        waiting_for_input(&pty.observer).await;
        assert!(
            observed
                .text()
                .contains("1. Owner realm: work; binding: browser")
        );
        assert_eq!(write(&pty.master, b"1\n").unwrap(), 2);
        shown(&observed, "Type 'signin' to authenticate this exact owner").await;
        waiting_for_input(&pty.observer).await;
        assert_eq!(write(&pty.master, b"signin\n").unwrap(), 7);
        shown(&observed, "Waiting for the native browser callback").await;
        // Only the protected presenter sees this URL. Never include it in
        // panic diagnostics, model messages or persistent test evidence.
        let prefix = format!("{origin}/{}/authorize?", profile.alias());
        let private = observed.text();
        let authorize = private
            .lines()
            .find(|line| line.starts_with(&prefix))
            .expect("native private authorization URL");
        let authorize = reqwest::Url::parse(authorize).expect("valid native authorization URL");
        let query = authorize
            .query_pairs()
            .into_owned()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(
            query
                .get("code_challenge_method")
                .is_some_and(|value| value == "S256")
        );
        assert!(
            query
                .get("code_challenge")
                .is_some_and(|value| !value.is_empty())
        );
        let state = query.get("state").expect("native flow state");
        assert!(!state.is_empty());
        let redirect = reqwest::Url::parse(query.get("redirect_uri").expect("native redirect"))
            .expect("valid native redirect");
        profile.check_redirect(&redirect);
        let mut callback = redirect.clone();
        callback
            .query_pairs_mut()
            .append_pair("state", state)
            .append_pair("code", CODE);
        let response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(BOUND)
            .build()
            .unwrap()
            .get(callback)
            .send()
            .await
            .unwrap_or_else(|_| panic!("synthetic callback request failed; private URL withheld"));
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        response
            .bytes()
            .await
            .unwrap_or_else(|_| panic!("synthetic callback body failed; private URL withheld"));
        redirect
    };
    let (result, redirect) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                None,
                false,
                &mut pty.input,
                &mut output,
                async {
                    interrupt_rx
                        .await
                        .map_err(|_| io::Error::other("completion interruption sender closed"))
                },
            ),
            human,
        )
    })
    .await
    .expect("configured browser flow exceeded its observation bound");
    let request = result
        .unwrap_or_else(|_| panic!("native configured sign-in failed; private details withheld"));
    assert!(
        output
            .text()
            .contains("Native sign-in completed for the selected owner")
    );
    for private in [CODE, ACCESS, REFRESH] {
        assert!(
            !output.text().contains(private),
            "code and tokens never enter terminal output"
        );
    }
    pty.restored();
    assert!(
        timeout(
            BOUND,
            tokio::net::TcpStream::connect(("127.0.0.1", redirect.port().unwrap()))
        )
        .await
        .unwrap()
        .is_err(),
        "native callback must be joined before presentation completes"
    );

    if cancel_on_completion {
        assert!(
            output.interrupt.is_none(),
            "completion flush triggered interruption"
        );
        assert!(
            request.is_none(),
            "late interruption must return no continuation"
        );
        assert!(
            output
                .text()
                .contains("Connect stopped before PairedHost pairing or setup session creation.")
        );
        assert_eq!(
            std::fs::read(root.join("state/work/config.toml")).unwrap(),
            config
        );

        use meerkat_auth_core::auth_store::{PersistedAuthMode, TokenKey, TokenStoreBackend};
        let backend = TokenStoreBackend::default_auto().unwrap();
        let TokenStoreBackend::File { root: store_root } = &backend else {
            panic!("isolated native default must remain file-backed");
        };
        assert!(
            store_root.starts_with(&root),
            "native store stays inside isolated roots"
        );
        let store = backend.open().unwrap();
        let effective = meerkat_connect::realm::read_config(&selected)
            .await
            .unwrap();
        let target = meerkat_core::resolve_explicit_auth_binding_target(
            &effective,
            &meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("work").unwrap(),
                binding: meerkat_core::BindingId::parse("browser").unwrap(),
                profile: None,
                origin: meerkat_core::BindingOrigin::Configured,
            },
        )
        .unwrap();
        let key = TokenKey::from_credential_identity(&target.credential_identity);
        assert_eq!(
            timeout(BOUND, store.list()).await.unwrap().unwrap(),
            vec![key.clone()]
        );
        let stored = timeout(BOUND, store.load(&key))
            .await
            .unwrap()
            .unwrap()
            .expect("successful native exchange retains its published credential");
        assert_eq!(stored.auth_mode, PersistedAuthMode::GoogleOauth);
        assert!(stored.primary_secret.as_deref() == Some(ACCESS));
        assert!(stored.refresh_token.as_deref() == Some(REFRESH));
        return;
    }

    let request = request.expect("sign-in must return its exact selected target");
    let binding = request
        .auth_binding
        .as_ref()
        .expect("explicit native target");
    assert_eq!(binding.realm.as_str(), "work");
    assert_eq!(binding.binding.as_str(), "browser");
    let effective = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let resolution = meerkat::resolve_create_session_model(&effective, request.clone()).unwrap();
    assert_eq!(resolution.provider, profile.provider());
    assert_eq!(
        resolution.model,
        meerkat_models::default_model(profile.provider()).unwrap()
    );

    // Materialize through the same retained startup bundle and the public CLI
    // composition. No client injection, model request or credential copying.
    let (paired_host, _hosts) = PairedHostFrontend::deferred_with_research();
    let (lazy, _services) = LazySetup::new(paired_host);
    let (approvals, _decisions) = LocalApprovals::create_paired_host(&root).unwrap();
    let mut options = lazy
        .session_options(SessionOptions {
            model: request,
            ..Default::default()
        })
        .unwrap();
    approvals.attach_lazy_setup(&mut options, &lazy).unwrap();
    let session = timeout(BOUND, Box::pin(startup.open_lazy_setup(options, &lazy)))
        .await
        .expect("native session materialization bound")
        .unwrap_or_else(|_| {
            panic!("native session materialization failed; private details withheld")
        });
    approvals.bind_session(session.id()).unwrap();
    timeout(BOUND, session.close())
        .await
        .expect("native close bound")
        .unwrap_or_else(|_| panic!("native session close failed; private details withheld"));
    assert_eq!(
        std::fs::read(root.join("state/work/config.toml")).unwrap(),
        config
    );
}

#[tokio::test]
async fn configured_openai_browser_login_materializes_same_native_session() {
    if isolated_for(
        "configured_openai_browser_login_materializes_same_native_session",
        Some(BrowserProfile::OpenAi),
    )
    .await
    {
        browser_login(BrowserProfile::OpenAi, false).await;
    }
}

#[tokio::test]
async fn configured_google_browser_login_materializes_same_native_session() {
    if isolated_for(
        "configured_google_browser_login_materializes_same_native_session",
        Some(BrowserProfile::Google),
    )
    .await
    {
        browser_login(BrowserProfile::Google, false).await;
    }
}

#[tokio::test]
async fn browser_completion_output_interruption_preserves_credentials_without_continuation() {
    if isolated_for(
        "browser_completion_output_interruption_preserves_credentials_without_continuation",
        Some(BrowserProfile::Google),
    )
    .await
    {
        browser_login(BrowserProfile::Google, true).await;
    }
}

const DEVICE_CODE: &str = "synthetic-private-device-code";
const DEVICE_USER_CODE: &str = "SYNTHETIC-USER-CODE";
const DEVICE_ERROR_BODY: &str = "synthetic-private-device-response-body";

#[derive(Clone, Copy)]
enum DeviceCase {
    Denied,
    ProviderExpired,
    NativeWindow,
    OutputFailure,
    StartRedirect,
    PollRedirect,
}

impl DeviceCase {
    fn expects_poll(self) -> bool {
        matches!(
            self,
            Self::Denied | Self::ProviderExpired | Self::PollRedirect
        )
    }
}

struct DeviceFixture {
    origin: String,
    case: DeviceCase,
    calls: Arc<Mutex<Vec<&'static str>>>,
    redirected: Arc<AtomicUsize>,
    shutdown: Vec<tokio::sync::oneshot::Sender<()>>,
    tasks: Vec<tokio::task::JoinHandle<io::Result<()>>>,
}

impl DeviceFixture {
    async fn start(case: DeviceCase) -> Self {
        use axum::{http::StatusCode, response::IntoResponse};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sink_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let sink = format!(
            "http://{}/must-not-follow",
            sink_listener.local_addr().unwrap()
        );
        let verification = format!("{origin}/private-human-verification");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let app =
            axum::Router::new().fallback(move |request: axum::extract::Request| {
                let observed = observed.clone();
                let sink = sink.clone();
                let verification = verification.clone();
                async move {
                    let kind = match (request.method().as_str(), request.uri().path()) {
                        ("POST", "/copilot/device/code") => "start",
                        ("POST", "/copilot/token") => "poll",
                        _ => "unexpected",
                    };
                    observed.lock().unwrap().push(kind);
                    if kind == "unexpected" {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                    assert!(request.uri().query().is_none());
                    assert!(!request.headers().contains_key("cookie"));
                    assert!(!request.headers().contains_key("authorization"));
                    assert!(
                        request.headers()["content-type"]
                            .to_str()
                            .unwrap()
                            .starts_with("application/x-www-form-urlencoded")
                    );
                    let body = axum::body::to_bytes(request.into_body(), 8192)
                        .await
                        .expect("bounded synthetic device request");
                    let mut form = reqwest::Url::parse("http://fixture.invalid/").unwrap();
                    form.set_query(Some(
                        std::str::from_utf8(&body).expect("synthetic form UTF-8"),
                    ));
                    let fields = form
                        .query_pairs()
                        .into_owned()
                        .collect::<std::collections::BTreeMap<_, _>>();
                    assert!(
                        fields
                            .get("client_id")
                            .is_some_and(|value| !value.is_empty())
                    );
                    if kind == "start" {
                        assert_eq!(fields.len(), 2);
                        assert!(fields.contains_key("scope"));
                        if matches!(case, DeviceCase::StartRedirect) {
                            return (
                                StatusCode::TEMPORARY_REDIRECT,
                                [("location", sink)],
                                DEVICE_ERROR_BODY,
                            )
                                .into_response();
                        }
                        return axum::Json(serde_json::json!({
                            "device_code":DEVICE_CODE, "user_code":DEVICE_USER_CODE,
                            "verification_uri":verification,
                            "expires_in":if matches!(case, DeviceCase::NativeWindow) {2} else {120},
                            "interval":if matches!(case, DeviceCase::NativeWindow) {5} else {1}
                        }))
                        .into_response();
                    }
                    assert_eq!(fields.len(), 3);
                    assert!(
                        fields
                            .get("device_code")
                            .is_some_and(|value| value == DEVICE_CODE)
                    );
                    assert!(fields.get("grant_type").is_some_and(
                        |value| value == "urn:ietf:params:oauth:grant-type:device_code"
                    ));
                    match case {
                        DeviceCase::Denied => axum::Json(serde_json::json!({
                            "error":"access_denied"
                        }))
                        .into_response(),
                        DeviceCase::ProviderExpired => (
                            StatusCode::BAD_REQUEST,
                            axum::Json(serde_json::json!({"error":"expired_token"})),
                        )
                            .into_response(),
                        DeviceCase::PollRedirect => (
                            StatusCode::TEMPORARY_REDIRECT,
                            [("location", sink)],
                            axum::Json(serde_json::json!({"error":DEVICE_ERROR_BODY})),
                        )
                            .into_response(),
                        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                    }
                }
            });
        let redirected = Arc::new(AtomicUsize::new(0));
        let followed = redirected.clone();
        let sink_app = axum::Router::new().fallback(move || {
            let followed = followed.clone();
            async move {
                followed.fetch_add(1, Ordering::SeqCst);
                StatusCode::INTERNAL_SERVER_ERROR
            }
        });
        let mut shutdown = Vec::new();
        let mut tasks = Vec::new();
        for (listener, app) in [(listener, app), (sink_listener, sink_app)] {
            let (send, receive) = tokio::sync::oneshot::channel();
            shutdown.push(send);
            tasks.push(tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = receive.await;
                    })
                    .await
            }));
        }
        Self {
            origin,
            case,
            calls,
            redirected,
            shutdown,
            tasks,
        }
    }

    async fn finish(mut self) {
        for send in self.shutdown.drain(..) {
            send.send(()).unwrap();
        }
        for task in self.tasks.drain(..) {
            timeout(BOUND, task).await.unwrap().unwrap().unwrap();
        }
        let expected = if self.case.expects_poll() {
            vec!["start", "poll"]
        } else {
            vec!["start"]
        };
        assert_eq!(
            *self.calls.lock().unwrap(),
            expected,
            "exact native requests, with no retries or metadata/model call"
        );
        assert_eq!(
            self.redirected.load(Ordering::SeqCst),
            0,
            "no request reaches the different-origin redirect receiver"
        );
    }
}

impl Drop for DeviceFixture {
    fn drop(&mut self) {
        for send in self.shutdown.drain(..) {
            let _ = send.send(());
        }
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

fn configured_device(root: &Path) -> Vec<u8> {
    let text = r#"[agent]
model = "gpt-6-astra"
[realm.work]
default_binding = "device"
[realm.work.backend.device]
provider = "openai"
backend_kind = "copilot"
[realm.work.auth.device]
provider = "openai"
auth_method = "github_copilot_oauth"
source = {kind = "managed_store"}
constraints = {allow_interactive_login = true}
[realm.work.binding.device]
backend_profile = "device"
auth_profile = "device"
credential_account = "synthetic_copilot"
default_model = "gpt-6-astra"
"#;
    meerkat_core::Config::from_persisted_toml(text)
        .unwrap()
        .0
        .validate(meerkat_models::canonical())
        .unwrap();
    std::fs::create_dir_all(root.join("state/work")).unwrap();
    std::fs::write(root.join("state/work/config.toml"), text).unwrap();
    text.as_bytes().to_vec()
}

struct DeviceOutput {
    observed: Output,
    refuse_instructions: bool,
    failed: bool,
}

impl Write for DeviceOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.refuse_instructions {
            let mut candidate = self.observed.0.lock().unwrap().clone();
            candidate.extend_from_slice(bytes);
            if String::from_utf8_lossy(&candidate)
                .contains("Open this private device sign-in page:")
            {
                self.failed = true;
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "synthetic private output closed",
                ));
            }
        }
        self.observed.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

async fn device_refusal(case: DeviceCase) {
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let config = configured_device(&root);
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected).await.unwrap();
    let mut pty = Pty::new();
    let observed = Output::default();
    let mut output = DeviceOutput {
        observed: observed.clone(),
        refuse_instructions: matches!(case, DeviceCase::OutputFailure),
        failed: false,
    };
    let human = async {
        shown(&observed, "Model target (number, or /quit): ").await;
        waiting_for_input(&pty.observer).await;
        assert!(
            observed
                .text()
                .contains("1. Owner realm: work; binding: device")
        );
        assert_eq!(write(&pty.master, b"1\n").unwrap(), 2);
        shown(&observed, "Type 'signin' to authenticate this exact owner").await;
        waiting_for_input(&pty.observer).await;
        assert_eq!(write(&pty.master, b"signin\n").unwrap(), 7);
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                None,
                true,
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            human
        )
    })
    .await
    .expect("native device presenter bound; private transcript withheld");
    let error = result.expect_err("refusal must not return a selected session request");
    let public = error.to_string();
    let transcript = observed.text();
    for private in [DEVICE_CODE, DEVICE_ERROR_BODY, ACCESS, REFRESH] {
        assert!(
            !public.contains(private),
            "error display excludes private device data"
        );
        assert!(
            !transcript.contains(private),
            "protected notice excludes raw device code and errors"
        );
    }
    assert!(!transcript.contains("Native device sign-in completed"));
    assert!(!public.contains(DEVICE_USER_CODE));
    match case {
        DeviceCase::Denied => {
            assert!(matches!(
                error,
                meerkat_connect::ConnectError::InvalidInput(_)
            ));
            assert!(public.contains("was denied"));
        }
        DeviceCase::ProviderExpired => {
            assert!(matches!(
                error,
                meerkat_connect::ConnectError::InvalidInput(_)
            ));
            assert!(public.contains("sign-in expired"));
        }
        DeviceCase::NativeWindow => {
            assert!(matches!(
                error,
                meerkat_connect::ConnectError::InvalidInput(_)
            ));
            assert!(public.contains("window elapsed"));
            assert!(public.contains("not revoked"));
        }
        DeviceCase::OutputFailure => {
            assert!(
                output.failed,
                "writer failed only after native device admission"
            );
            let meerkat_connect::ConnectError::Native { source, .. } = error else {
                panic!("expected protected output failure");
            };
            assert_eq!(
                source.downcast_ref::<io::Error>().unwrap().kind(),
                io::ErrorKind::BrokenPipe
            );
            assert!(!transcript.contains(DEVICE_USER_CODE));
        }
        DeviceCase::StartRedirect | DeviceCase::PollRedirect => {
            assert!(matches!(
                error,
                meerkat_connect::ConnectError::ProviderSetup { .. }
            ));
            assert!(public.contains("provider details are withheld"));
        }
    }
    assert_eq!(
        std::fs::read(root.join("state/work/config.toml")).unwrap(),
        config
    );
    pty.restored();
    // No materialization is attempted: Copilot's native authorizer would prime
    // fixed HTTPS endpoints. These controls cover the public presenter only.
}

#[tokio::test]
async fn configured_device_denial_ends_without_retry() {
    if isolated_device(
        "configured_device_denial_ends_without_retry",
        DeviceCase::Denied,
    )
    .await
    {
        device_refusal(DeviceCase::Denied).await;
    }
}

#[tokio::test]
async fn configured_device_provider_expiry_ends_without_retry() {
    if isolated_device(
        "configured_device_provider_expiry_ends_without_retry",
        DeviceCase::ProviderExpired,
    )
    .await
    {
        device_refusal(DeviceCase::ProviderExpired).await;
    }
}

#[tokio::test]
async fn configured_device_native_window_stops_before_first_poll() {
    if isolated_device(
        "configured_device_native_window_stops_before_first_poll",
        DeviceCase::NativeWindow,
    )
    .await
    {
        device_refusal(DeviceCase::NativeWindow).await;
    }
}

#[tokio::test]
async fn configured_device_output_failure_after_admission_stops_polling() {
    if isolated_device(
        "configured_device_output_failure_after_admission_stops_polling",
        DeviceCase::OutputFailure,
    )
    .await
    {
        device_refusal(DeviceCase::OutputFailure).await;
    }
}

#[tokio::test]
async fn configured_device_start_redirect_is_not_followed() {
    if isolated_device(
        "configured_device_start_redirect_is_not_followed",
        DeviceCase::StartRedirect,
    )
    .await
    {
        device_refusal(DeviceCase::StartRedirect).await;
    }
}

#[tokio::test]
async fn configured_device_poll_redirect_is_not_followed() {
    if isolated_device(
        "configured_device_poll_redirect_is_not_followed",
        DeviceCase::PollRedirect,
    )
    .await
    {
        device_refusal(DeviceCase::PollRedirect).await;
    }
}

fn managed_config(root: &Path, realm: &str, source: &str, method: &str) -> Vec<u8> {
    let config = configured(&root.join("state"), realm, "CONNECT_PROVIDER_ABSENT_KEY");
    let text = String::from_utf8(config)
        .unwrap()
        .replace(
            "source = { kind = \"env\", env = \"CONNECT_PROVIDER_ABSENT_KEY\" }",
            source,
        )
        .replace(
            "auth_method = \"api_key\"",
            &format!("auth_method = \"{method}\""),
        )
        .replace(
            "auth_profile = \"configured\"",
            "auth_profile = \"configured\"\ncredential_account = \"selected_account\"",
        );
    meerkat_core::Config::from_persisted_toml(&text)
        .unwrap()
        .0
        .validate(meerkat_models::canonical())
        .unwrap();
    std::fs::write(root.join("state").join(realm).join("config.toml"), &text).unwrap();
    text.into_bytes()
}

async fn key_entry(master: &OwnedFd, observer: &OwnedFd, output: &Output) {
    shown(output, "Model target (number, or /quit): ").await;
    waiting_for_input(observer).await;
    write(master, b"1\n").unwrap();
    shown(output, "Type 'enter-key'").await;
    waiting_for_input(observer).await;
    write(master, b"enter-key\n").unwrap();
    assert!(output.text().contains("profile: configured"));
    assert!(
        output
            .text()
            .contains("Shared credential account: selected_account")
    );
    waiting_for_input(observer).await;
    shown(output, "Enter the API key (hidden)").await;
    assert!(
        !tcgetattr(observer)
            .unwrap()
            .local_flags
            .contains(nix::sys::termios::LocalFlags::ECHO)
    );
}

async fn stored_key(
    root: &Path,
    selected: &meerkat_core::ResolvedStorage,
    owner: &str,
    expected: bool,
) {
    use meerkat_auth_core::auth_store::{PersistedAuthMode, TokenKey, TokenStoreBackend};
    let backend = TokenStoreBackend::default_auto().unwrap();
    let TokenStoreBackend::File { root: store_root } = &backend else {
        panic!("fixture must use the isolated native file store");
    };
    assert!(store_root.starts_with(root));
    let store = backend.open().unwrap();
    let config = meerkat_connect::realm::read_config(selected).await.unwrap();
    let target = meerkat_core::resolve_explicit_auth_binding_target(
        &config,
        &meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse(owner).unwrap(),
            binding: meerkat_core::BindingId::parse("configured").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        },
    )
    .unwrap();
    let key = TokenKey::from_credential_identity(&target.credential_identity);
    assert_eq!(key.realm().as_str(), owner);
    assert_eq!(key.account().unwrap().as_str(), "selected_account");
    let keys = timeout(BOUND, store.list()).await.unwrap().unwrap();
    if expected {
        assert_eq!(
            keys,
            vec![key.clone()],
            "only the explicit native owner is written"
        );
        let tokens = timeout(BOUND, store.load(&key))
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(tokens.auth_mode, PersistedAuthMode::ApiKey);
        assert!(tokens.primary_secret.as_deref() == Some(MANAGED_KEY));
        assert!(!format!("{tokens:?}").contains(MANAGED_KEY));
    } else {
        assert!(keys.is_empty(), "no credential publication occurred");
    }
}

const KEY_PROMPT: &str = "Enter the API key (hidden), or /quit: ";
const REJECTED_KEY: &str = "synthetic-rejected-key\t";

struct KeyCorrectionOutput {
    captured: Output,
    observer: OwnedFd,
    hidden_prompts: usize,
}

impl Write for KeyCorrectionOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes == KEY_PROMPT.as_bytes() || bytes.starts_with(b"API key must be nonempty") {
            assert!(
                !tcgetattr(&self.observer)
                    .unwrap()
                    .local_flags
                    .contains(nix::sys::termios::LocalFlags::ECHO),
                "entry prompts and correction feedback must remain hidden"
            );
            if bytes == KEY_PROMPT.as_bytes() {
                self.hidden_prompts += 1;
            }
        }
        self.captured.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.captured.flush()
    }
}

async fn reject_key_entry(
    master: &OwnedFd,
    observer: &OwnedFd,
    output: &Output,
    invalid: &[u8],
    prompts: usize,
) {
    write(master, invalid).unwrap();
    timeout(BOUND, async {
        while output.text().matches(KEY_PROMPT).count() < prompts {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("local input correction must present another hidden prompt");
    assert!(
        !tcgetattr(observer)
            .unwrap()
            .local_flags
            .contains(nix::sys::termios::LocalFlags::ECHO)
    );
    assert!(!output.text().contains(REJECTED_KEY));
    assert!(!output.text().contains("synthetic-rejected-key"));
}

#[tokio::test]
async fn managed_api_key_local_correction_preserves_owner_and_startup() {
    if !isolated("managed_api_key_local_correction_preserves_owner_and_startup").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let config = managed_config(
        &root,
        "owner",
        "source = {kind = \"managed_store\"}",
        "api_key",
    );
    let child = b"[realm.work]\nparent = \"owner\"\n";
    std::fs::create_dir_all(root.join("state/work")).unwrap();
    std::fs::write(root.join("state/work/config.toml"), child).unwrap();
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    let mut pty = Pty::new();
    let observed = Output::default();
    let mut output = KeyCorrectionOutput {
        captured: observed.clone(),
        observer: pty.observer.try_clone().unwrap(),
        hidden_prompts: 0,
    };
    let human = async {
        key_entry(&pty.master, &pty.observer, &observed).await;
        reject_key_entry(&pty.master, &pty.observer, &observed, b"\n", 2).await;
        stored_key(&root, &selected, "owner", false).await;
        reject_key_entry(
            &pty.master,
            &pty.observer,
            &observed,
            format!("{REJECTED_KEY}\n").as_bytes(),
            3,
        )
        .await;
        stored_key(&root, &selected, "owner", false).await;
        unchanged(&root, "owner", &config);
        unchanged(&root, "work", child);
        write(&pty.master, format!("{MANAGED_KEY}\n").as_bytes()).unwrap();
    };
    tokio::pin!(human);
    let result = {
        let present = present_provider_setup(
            &mut startup,
            None,
            true,
            &mut pty.input,
            &mut output,
            std::future::pending(),
        );
        tokio::pin!(present);
        timeout(BOUND, async {
            tokio::select! { result = &mut present => result, () = &mut human => present.await }
        })
        .await
        .expect("managed key correction must settle within the fixture bound")
    };
    let request = result
        .expect("local input validation must allow correction instead of ending setup")
        .expect("corrected input must retain the selected target");
    assert_eq!(
        request.auth_binding.as_ref().unwrap().realm.as_str(),
        "owner"
    );
    assert_eq!(
        request.auth_binding.as_ref().unwrap().binding.as_str(),
        "configured"
    );
    assert_eq!(selected.locator.realm.as_str(), "work");
    assert_eq!(output.hidden_prompts, 3);
    assert_eq!(
        observed
            .text()
            .matches("API key stored by the native credential owner.")
            .count(),
        1
    );
    assert!(!observed.text().contains(MANAGED_KEY));
    assert!(!observed.text().contains(REJECTED_KEY));
    assert!(!observed.text().contains("synthetic-rejected-key"));
    pty.restored();
    stored_key(&root, &selected, "owner", true).await;
    // The corrected credential feeds the original native startup, without a
    // model invocation or a second authentication owner.
    let (frontend, _host) = PairedHostFrontend::deferred();
    let (lazy, _service) = LazySetup::new(frontend);
    let (approvals, _decisions) = LocalApprovals::create_paired_host(&root).unwrap();
    let mut options = lazy
        .session_options(SessionOptions {
            model: request,
            ..Default::default()
        })
        .unwrap();
    approvals.attach_lazy_setup(&mut options, &lazy).unwrap();
    let session = timeout(BOUND, Box::pin(startup.open_lazy_setup(options, &lazy)))
        .await
        .unwrap()
        .unwrap();
    timeout(BOUND, session.close()).await.unwrap().unwrap();
    assert_eq!(
        std::fs::read(root.join("state/owner/config.toml")).unwrap(),
        config
    );
    assert_eq!(
        std::fs::read(root.join("state/work/config.toml")).unwrap(),
        child
    );
}

#[tokio::test]
async fn managed_api_key_local_correction_exit_restores_terminal_without_publication() {
    if !isolated("managed_api_key_local_correction_exit_restores_terminal_without_publication")
        .await
    {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let config = managed_config(
        &root,
        "work",
        "source = {kind = \"managed_store\"}",
        "api_key",
    );
    let selected = storage(&root, "work");
    for exit in ["quit", "eof", "interrupt"] {
        let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
        let mut pty = Pty::new();
        let observed = Output::default();
        let mut output = KeyCorrectionOutput {
            captured: observed.clone(),
            observer: pty.observer.try_clone().unwrap(),
            hidden_prompts: 0,
        };
        let (signal, cancel) = tokio::sync::oneshot::channel();
        let human = async {
            key_entry(&pty.master, &pty.observer, &observed).await;
            reject_key_entry(&pty.master, &pty.observer, &observed, b"\n", 2).await;
            stored_key(&root, &selected, "work", false).await;
            match exit {
                "quit" => {
                    write(&pty.master, b"/quit\n").unwrap();
                }
                "eof" => {
                    write(&pty.master, b"\x04").unwrap();
                }
                _ => {
                    write(&pty.master, MANAGED_KEY.as_bytes()).unwrap();
                    signal.send(()).unwrap();
                }
            }
            std::future::pending::<()>().await;
        };
        tokio::pin!(human);
        let result = {
            let present = present_provider_setup(
                &mut startup,
                None,
                true,
                &mut pty.input,
                &mut output,
                async {
                    cancel
                        .await
                        .map_err(|_| io::Error::other("test interrupt closed"))
                },
            );
            tokio::pin!(present);
            timeout(BOUND, async {
                tokio::select! { result = &mut present => result, () = &mut human => present.await }
            })
            .await
            .expect("correction exit must settle within the fixture bound")
        };
        assert!(
            result
                .expect("local validation must allow exit from the correction prompt")
                .is_none()
        );
        assert_eq!(output.hidden_prompts, 2);
        assert!(!observed.text().contains(MANAGED_KEY));
        assert!(
            !observed
                .text()
                .contains("API key stored by the native credential owner.")
        );
        pty.restored();
        stored_key(&root, &selected, "work", false).await;
        unchanged(&root, "work", &config);
    }
}

#[tokio::test]
async fn configured_managed_api_key_uses_explicit_owner_and_same_native_session() {
    if !isolated("configured_managed_api_key_uses_explicit_owner_and_same_native_session").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let config = managed_config(
        &root,
        "owner",
        "source = {kind = \"managed_store\"}",
        "api_key",
    );
    let child = b"[realm.work]\nparent = \"owner\"\n";
    std::fs::create_dir_all(root.join("state/work")).unwrap();
    std::fs::write(root.join("state/work/config.toml"), child).unwrap();
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        key_entry(&pty.master, &pty.observer, &observed).await;
        assert!(
            observed
                .text()
                .contains("owner realm: owner; binding: configured")
        );
        write(&pty.master, format!("{MANAGED_KEY}\n").as_bytes()).unwrap();
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                None,
                true,
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            human
        )
    })
    .await
    .expect("managed key presenter bound; private transcript withheld");
    let request = result.unwrap().unwrap();
    assert_eq!(
        request.auth_binding.as_ref().unwrap().realm.as_str(),
        "owner"
    );
    assert_eq!(selected.locator.realm.as_str(), "work");
    assert!(!output.text().contains(MANAGED_KEY));
    pty.restored();
    stored_key(&root, &selected, "owner", true).await;
    // This is the original startup, not a second auth/runtime owner.
    let (frontend, _host) = PairedHostFrontend::deferred();
    let (lazy, _service) = LazySetup::new(frontend);
    let (approvals, _decisions) = LocalApprovals::create_paired_host(&root).unwrap();
    let mut options = lazy
        .session_options(SessionOptions {
            model: request,
            ..Default::default()
        })
        .unwrap();
    approvals.attach_lazy_setup(&mut options, &lazy).unwrap();
    let session = timeout(BOUND, Box::pin(startup.open_lazy_setup(options, &lazy)))
        .await
        .unwrap()
        .unwrap();
    timeout(BOUND, session.close()).await.unwrap().unwrap();
    assert_eq!(
        std::fs::read(root.join("state/owner/config.toml")).unwrap(),
        config
    );
    assert_eq!(
        std::fs::read(root.join("state/work/config.toml")).unwrap(),
        child
    );
}

async fn first_target_key_entry(
    master: &OwnedFd,
    observer: &OwnedFd,
    output: &Output,
    name: &[u8],
) {
    shown(output, "Connection name [").await;
    waiting_for_input(observer).await;
    write(master, name).unwrap();
    shown(output, "Type 'create'").await;
    waiting_for_input(observer).await;
    assert!(!output.text().contains("Enter the API key"));
    write(master, b"create\n").unwrap();
    shown(output, "Type 'enter-key'").await;
    waiting_for_input(observer).await;
    write(master, b"enter-key\n").unwrap();
    shown(output, KEY_PROMPT).await;
    waiting_for_input(observer).await;
    assert!(
        !tcgetattr(observer)
            .unwrap()
            .local_flags
            .contains(nix::sys::termios::LocalFlags::ECHO)
    );
}

#[tokio::test]
async fn empty_realm_without_environment_creates_first_target_and_uses_original_startup() {
    if !isolated("empty_realm_without_environment_creates_first_target_and_uses_original_startup")
        .await
    {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let selected = storage(&root, "work");
    let path = root.join("state/work/config.toml");
    assert!(!path.exists());
    assert!(
        !selected
            .layout
            .user_rkat_root()
            .unwrap()
            .join("config.toml")
            .exists()
    );
    for key in [
        "RKAT_OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "RKAT_ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "RKAT_GEMINI_API_KEY",
        "GEMINI_API_KEY",
        "RKAT_GOOGLE_API_KEY",
        "GOOGLE_API_KEY",
    ] {
        assert!(std::env::var_os(key).is_none());
    }
    let original_config = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let original_model =
        meerkat::resolve_create_session_model(&original_config, Default::default()).unwrap();
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        first_target_key_entry(&pty.master, &pty.observer, &observed, b"\n").await;
        assert!(
            path.is_file(),
            "configuration must settle before secret collection"
        );
        write(&pty.master, format!("{MANAGED_KEY}\n").as_bytes()).unwrap();
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                None,
                false,
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            human
        )
    })
    .await
    .expect("the actual no-env public first-target path must reach terminal input");
    let request = result.unwrap().unwrap();
    assert_eq!(
        request.model.as_deref(),
        Some(original_model.model.as_str()),
        "the returned continuation selection retains the same native effective model"
    );
    assert_eq!(request.provider, Some(original_model.provider));
    let binding = request.auth_binding.as_ref().unwrap();
    assert_eq!(binding.realm.as_str(), "work");
    assert_eq!(
        binding.binding.as_str(),
        format!("{}_api_key", original_model.provider.as_str())
    );
    let config = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let target = meerkat_core::resolve_explicit_auth_binding_target(&config, binding).unwrap();
    let ordinary_request = meerkat::CreateSessionModelResolutionRequest::default();
    assert!(
        ordinary_request.auth_binding.is_none(),
        "ordinary selection must not supply the returned override"
    );
    let ordinary = meerkat::resolve_create_session_model(&config, ordinary_request).unwrap();
    assert_eq!(ordinary.model, original_model.model);
    assert_eq!(ordinary.provider, original_model.provider);
    let candidates = meerkat_core::resolve_auth_binding_candidates_for_provider(
        &config,
        ordinary.provider,
        ordinary.auth_binding.as_ref(),
        Some(&selected.locator.realm),
        true,
    )
    .unwrap();
    assert_eq!(candidates.first(), Some(&target));
    use meerkat_auth_core::auth_store::{TokenKey, TokenStoreBackend};
    let backend = TokenStoreBackend::default_auto().unwrap();
    let TokenStoreBackend::File { root: store_root } = &backend else {
        panic!("isolated native file store required");
    };
    assert!(store_root.starts_with(&root));
    let store = backend.open().unwrap();
    let key = TokenKey::from_credential_identity(&target.credential_identity);
    assert_eq!(store.list().await.unwrap(), vec![key.clone()]);
    assert_eq!(
        store
            .load(&key)
            .await
            .unwrap()
            .unwrap()
            .primary_secret
            .as_deref(),
        Some(MANAGED_KEY)
    );
    assert!(!output.text().contains(MANAGED_KEY));
    pty.restored();
    // Consume this original startup into the existing native controller path.
    // Opening is deferred and makes no provider request in this control.
    let (frontend, _host) = PairedHostFrontend::deferred();
    let (lazy, _service) = LazySetup::new(frontend);
    let (approvals, _decisions) = LocalApprovals::create_paired_host(&root).unwrap();
    let mut options = lazy
        .session_options(SessionOptions {
            model: request,
            ..Default::default()
        })
        .unwrap();
    approvals.attach_lazy_setup(&mut options, &lazy).unwrap();
    let session = timeout(BOUND, Box::pin(startup.open_lazy_setup(options, &lazy)))
        .await
        .unwrap()
        .unwrap();
    timeout(BOUND, session.close()).await.unwrap().unwrap();
}

#[tokio::test]
async fn first_target_quit_before_creation_keeps_document_and_credentials_absent() {
    if !isolated("first_target_quit_before_creation_keeps_document_and_credentials_absent").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected).await.unwrap();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        shown(&observed, "Connection name [").await;
        waiting_for_input(&pty.observer).await;
        write(&pty.master, b"first_key\n").unwrap();
        shown(&observed, "Type 'create'").await;
        waiting_for_input(&pty.observer).await;
        write(&pty.master, b"/quit\n").unwrap();
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                None,
                false,
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_none());
    assert!(!root.join("state/work/config.toml").exists());
    assert!(!root.join("state/work/config_state.json").exists());
    let store = meerkat_auth_core::auth_store::TokenStoreBackend::default_auto()
        .unwrap()
        .open()
        .unwrap();
    assert!(store.list().await.unwrap().is_empty());
    assert!(!observed.text().contains(KEY_PROMPT));
    pty.restored();
}

#[tokio::test]
async fn first_target_document_change_during_hidden_input_refuses_native_save() {
    if !isolated("first_target_document_change_during_hidden_input_refuses_native_save").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human = async {
        first_target_key_entry(&pty.master, &pty.observer, &observed, b"first_key\n").await;
        use meerkat_core::ConfigStore;
        meerkat_core::FileConfigStore::new(
            root.join("state/work/config.toml"),
            meerkat_models::canonical(),
        )
        .patch(meerkat_core::ConfigDelta(
            serde_json::json!({"realm":{"work":{"auth":{"first_key":{
                "source":{"kind":"env","env":"CONNECT_PROVIDER_ABSENT_KEY"}
            }}}}}),
        ))
        .await
        .unwrap();
        write(&pty.master, format!("{MANAGED_KEY}\n").as_bytes()).unwrap();
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                None,
                false,
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            human
        )
    })
    .await
    .unwrap();
    assert!(
        result.is_err(),
        "the old retained Config must not authorize a write after input"
    );
    let store = meerkat_auth_core::auth_store::TokenStoreBackend::default_auto()
        .unwrap()
        .open()
        .unwrap();
    assert!(store.list().await.unwrap().is_empty());
    assert!(
        !observed
            .text()
            .contains("API key stored by the native credential owner.")
    );
    assert!(!observed.text().contains(MANAGED_KEY));
    let config = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let reference = meerkat_core::AuthBindingRef {
        realm: selected.locator.realm,
        binding: meerkat_core::BindingId::parse("first_key").unwrap(),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    };
    let target = meerkat_core::resolve_explicit_auth_binding_target(&config, &reference).unwrap();
    assert!(matches!(
        target.auth_profile.source,
        meerkat_core::CredentialSourceSpec::Env { .. }
    ));
    pty.restored();
}

#[tokio::test]
async fn first_target_explicit_model_reviews_durable_realm_default_and_preserves_parent() {
    if !isolated("first_target_explicit_model_reviews_durable_realm_default_and_preserves_parent")
        .await
    {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let selected = storage(&root, "work");
    let parent_path = selected
        .layout
        .user_rkat_root()
        .unwrap()
        .join("config.toml");
    std::fs::create_dir_all(parent_path.parent().unwrap()).unwrap();
    // Native implicit global inheritance requires its realm declaration.
    let parent = b"[agent]\nmodel = \"claude-opus-5-5\"\n[realm.global]\n";
    std::fs::write(&parent_path, parent).unwrap();
    let initial = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let initial_model =
        meerkat::resolve_create_session_model(&initial, Default::default()).unwrap();
    assert_eq!(initial_model.model, "claude-opus-5-5");
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let human =
        async {
            shown(&observed, "Connection name [").await;
            waiting_for_input(&pty.observer).await;
            write(&pty.master, b"first_key\n").unwrap();
            shown(&observed, "Type 'create'").await;
            waiting_for_input(&pty.observer).await;
            assert!(observed.text().contains(
                "Change this realm's default model from 'claude-opus-5-5' to 'gpt-6-astra'"
            ));
            assert!(
                observed
                    .text()
                    .contains("Parent and global documents are unchanged.")
            );
            assert!(!root.join("state/work/config.toml").exists());
            write(&pty.master, b"create\n").unwrap();
            shown(&observed, "Type 'enter-key'").await;
            waiting_for_input(&pty.observer).await;
            write(&pty.master, b"enter-key\n").unwrap();
            shown(&observed, KEY_PROMPT).await;
            waiting_for_input(&pty.observer).await;
            assert!(
                !tcgetattr(&pty.observer)
                    .unwrap()
                    .local_flags
                    .contains(nix::sys::termios::LocalFlags::ECHO)
            );
            write(&pty.master, format!("{MANAGED_KEY}\n").as_bytes()).unwrap();
        };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                Some("gpt-6-astra".into()),
                false,
                &mut pty.input,
                &mut output,
                std::future::pending()
            ),
            human
        )
    })
    .await
    .expect("explicit-model first target must finish the protected reviewed path");
    let request = result.unwrap().unwrap();
    assert_eq!(request.model.as_deref(), Some("gpt-6-astra"));
    let fresh = meerkat_connect::realm::read_config(&selected)
        .await
        .unwrap();
    let ordinary = meerkat::resolve_create_session_model(&fresh, Default::default()).unwrap();
    assert_eq!(
        ordinary.model, "gpt-6-astra",
        "no model, provider or binding override supplies this fresh choice"
    );
    assert_eq!(ordinary.provider, meerkat_core::Provider::OpenAI);
    let candidates = meerkat_core::resolve_auth_binding_candidates_for_provider(
        &fresh,
        ordinary.provider,
        ordinary.auth_binding.as_ref(),
        Some(&selected.locator.realm),
        true,
    )
    .unwrap();
    let target = meerkat_core::resolve_explicit_auth_binding_target(
        &fresh,
        request.auth_binding.as_ref().unwrap(),
    )
    .unwrap();
    assert_eq!(candidates.first(), Some(&target));
    assert_eq!(std::fs::read(&parent_path).unwrap(), parent);
    use meerkat_core::ConfigStore;
    let own = meerkat_core::FileConfigStore::new(
        root.join("state/work/config.toml"),
        meerkat_models::canonical(),
    )
    .get()
    .await
    .unwrap();
    assert_eq!(own.agent.model, "gpt-6-astra");
    assert!(!observed.text().contains(MANAGED_KEY));
    pty.restored();
}

#[tokio::test]
async fn managed_api_key_read_cancellation_restores_terminal_without_publication() {
    if !isolated("managed_api_key_read_cancellation_restores_terminal_without_publication").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let config = managed_config(
        &root,
        "work",
        "source = {kind = \"managed_store\"}",
        "api_key",
    );
    let selected = storage(&root, "work");
    for (quit, listener_failure) in [(false, false), (true, false), (false, true)] {
        let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let (signal, cancel) = tokio::sync::oneshot::channel();
        let human = async {
            key_entry(&pty.master, &pty.observer, &observed).await;
            if quit {
                write(&pty.master, b"/quit\n").unwrap();
                std::future::pending::<()>().await;
            } else {
                write(&pty.master, MANAGED_KEY.as_bytes()).unwrap();
                signal.send(()).unwrap();
            }
        };
        tokio::pin!(human);
        let result = {
            let present = present_provider_setup(
                &mut startup,
                None,
                true,
                &mut pty.input,
                &mut output,
                async {
                    cancel
                        .await
                        .map_err(|_| io::Error::other("test interrupt closed"))?;
                    if listener_failure {
                        Err(io::Error::other("synthetic interrupt listener failure"))
                    } else {
                        Ok(())
                    }
                },
            );
            tokio::pin!(present);
            timeout(BOUND, async {
                tokio::select! { result = &mut present => result, () = &mut human => present.await }
            })
            .await
            .unwrap()
        };
        if listener_failure {
            assert!(matches!(
                result,
                Err(meerkat_connect::ConnectError::Native {
                    operation: "Listen for API-key input cancellation",
                    ..
                })
            ));
        } else {
            assert!(result.unwrap().is_none());
        }
        assert!(!output.text().contains(MANAGED_KEY));
        pty.restored();
        stored_key(&root, &selected, "work", false).await;
        unchanged(&root, "work", &config);
    }
}

struct KeyCompletionOutput {
    captured: Output,
    interrupt: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Write for KeyCompletionOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.captured.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        if self
            .captured
            .text()
            .contains("API key stored by the native credential owner.")
        {
            if let Some(signal) = self.interrupt.take() {
                signal.send(()).unwrap();
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn managed_api_key_settled_save_interruption_keeps_native_result_without_continuation() {
    if !isolated(
        "managed_api_key_settled_save_interruption_keeps_native_result_without_continuation",
    )
    .await
    {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let config = managed_config(
        &root,
        "work",
        "source = {kind = \"managed_store\"}",
        "api_key",
    );
    let selected = storage(&root, "work");
    let mut startup = SessionStartup::open(selected.clone()).await.unwrap();
    let mut pty = Pty::new();
    let observed = Output::default();
    let (signal, cancel) = tokio::sync::oneshot::channel();
    let mut output = KeyCompletionOutput {
        captured: observed.clone(),
        interrupt: Some(signal),
    };
    let human = async {
        key_entry(&pty.master, &pty.observer, &observed).await;
        write(&pty.master, format!("{MANAGED_KEY}\n").as_bytes()).unwrap();
    };
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_provider_setup(
                &mut startup,
                None,
                true,
                &mut pty.input,
                &mut output,
                async {
                    cancel
                        .await
                        .map_err(|_| io::Error::other("test interrupt closed"))
                }
            ),
            human
        )
    })
    .await
    .unwrap();
    assert!(result.unwrap().is_none());
    assert!(
        output.interrupt.is_none(),
        "interruption follows actual native save settlement"
    );
    assert!(!observed.text().contains(MANAGED_KEY));
    pty.restored();
    stored_key(&root, &selected, "work", true).await;
    assert_eq!(
        std::fs::read(root.join("state/work/config.toml")).unwrap(),
        config
    );
}

#[tokio::test]
async fn managed_api_key_entry_refuses_env_and_non_key_profiles() {
    if !isolated("managed_api_key_entry_refuses_env_and_non_key_profiles").await {
        return;
    }
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    for (source, method) in [
        (
            "source = {kind = \"env\", env = \"CONNECT_PROVIDER_ABSENT_KEY\"}",
            "api_key",
        ),
        ("source = {kind = \"managed_store\"}", "static_bearer"),
    ] {
        let config = managed_config(&root, "work", source, method);
        let selected = storage(&root, "work");
        let mut startup = SessionStartup::open(selected).await.unwrap();
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let human = async {
            shown(&observed, "Model target (number, or /quit): ").await;
            waiting_for_input(&pty.observer).await;
            write(&pty.master, b"1\n").unwrap();
            shown(
                &observed,
                "This target's native credential source is not ready",
            )
            .await;
            waiting_for_input(&pty.observer).await;
            assert!(!observed.text().contains("enter-key"));
            write(&pty.master, b"/quit\n").unwrap();
        };
        let (result, ()) = timeout(BOUND, async {
            tokio::join!(
                present_provider_setup(
                    &mut startup,
                    None,
                    true,
                    &mut pty.input,
                    &mut output,
                    std::future::pending()
                ),
                human
            )
        })
        .await
        .unwrap();
        assert!(result.unwrap().is_none());
        pty.restored();
        unchanged(&root, "work", &config);
    }
}
