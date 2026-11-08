use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn scope(root: &std::path::Path) -> CredentialScope {
    CredentialScope::File {
        root: root.join("credentials"),
    }
}
fn storage(root: &std::path::Path) -> meerkat_core::ResolvedStorage {
    crate::RealmOptions {
        context_root: root.to_owned(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    }
    .resolve("provider-control")
    .unwrap()
}
async fn startup(root: &std::path::Path) -> SessionStartup {
    SessionStartup::open_with_provider_store(storage(root), root.join("credentials"))
        .await
        .unwrap()
}
fn bridge() -> (Bridge, mpsc::Receiver<Message>, watch::Sender<bool>) {
    let (messages, receive) = mpsc::channel(8);
    let (retire, retired) = watch::channel(false);
    (
        Bridge {
            messages,
            retired,
            deadline: wire::now() + 120_000,
            expires: tokio::time::Instant::now() + Duration::from_secs(120),
        },
        receive,
        retire,
    )
}
async fn key_target(
    startup: &mut SessionStartup,
    bridge: &Bridge,
) -> (
    ResolvedConnectionTarget,
    CreateSessionModelResolutionRequest,
) {
    let choices = target::catalog(startup).await.unwrap();
    let choice = choices
        .iter()
        .find(|c| c.enabled && c.provider == meerkat_core::Provider::OpenAI && c.auth == "api_key")
        .unwrap();
    target::prepare(startup, choice)
        .await
        .unwrap()
        .commit(startup, bridge)
        .await
        .unwrap()
}
fn init(root: &std::path::Path) -> Value {
    json!({"method":"initialize","id":"initialize","binding":{
        "protocol":wire::PROTOCOL,"application_id":"fixture","instance_id":"instance",
        "principal_id":"provider-user","login_scope_id":"login","expires_at_ms":wire::now()+120_000,
        "realm_id":"provider-control","context_root":root,"state_root":root.join("state"),
        "user_config_root":root.join("home"),"credential_scope":scope(root),"browser":"manual_return"}})
}
async fn write<W: AsyncWrite + Unpin>(writer: &mut W, value: Value) {
    tokio::time::timeout(Duration::from_secs(30), async {
        writer.write_all(format!("{}\n", value).as_bytes()).await?;
        writer.flush().await
    })
    .await
    .unwrap()
    .unwrap();
}
async fn read<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Value {
    let mut line = String::new();
    assert!(
        tokio::time::timeout(Duration::from_secs(30), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap()
            > 0
    );
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn first_target_uses_native_preview_cas_and_preserves_competing_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut startup = startup(dir.path()).await;
    let (bridge, _messages, _retire) = bridge();
    let choices = target::catalog(&mut startup).await.unwrap();
    let choice = choices
        .iter()
        .find(|c| c.enabled && c.provider == meerkat_core::Provider::OpenAI && c.auth == "api_key")
        .unwrap();
    let first = target::prepare(&mut startup, choice).await.unwrap();
    let stale = target::prepare(&mut startup, choice).await.unwrap();
    assert!(
        startup.persistence.is_none(),
        "catalog/config preview must not open a session owner"
    );
    let (target, _) = first.commit(&mut startup, &bridge).await.unwrap();
    assert!(stale.commit(&mut startup, &bridge).await.is_err());
    let actual = crate::realm::read_config(&startup.storage).await.unwrap();
    assert_eq!(
        resolve_explicit_auth_binding_target(&actual, &target.auth_binding).unwrap(),
        target
    );
    assert_eq!(configured_choices(&startup).unwrap().len(), 1);
    assert!(startup.persistence.is_none());
}

#[tokio::test]
async fn fresh_copilot_uses_real_native_shared_account_routes() {
    let dir = tempfile::tempdir().unwrap();
    let mut startup = startup(dir.path()).await;
    let (bridge, _messages, _retire) = bridge();
    let choices = target::catalog(&mut startup).await.unwrap();
    let choice = choices
        .iter()
        .find(|c| {
            c.enabled
                && c.provider == meerkat_core::Provider::OpenAI
                && NormalizedAuthMethod::parse_for_provider(c.provider, &c.auth)
                    .and_then(NormalizedAuthMethod::persisted_auth_mode)
                    == Some(PersistedAuthMode::GithubCopilotOauth)
        })
        .unwrap();
    let prepared = target::prepare(&mut startup, choice).await.unwrap();
    let (target, _) = prepared.commit(&mut startup, &bridge).await.unwrap();
    let account = target.credential_identity.account().unwrap();
    let bindings = configured_choices(&startup).unwrap();
    assert!(!bindings.is_empty());
    for binding in bindings {
        let selected = resolve_explicit_auth_binding_target(&startup.config, &binding).unwrap();
        assert_eq!(selected.credential_identity.account(), Some(account));
        validate_oauth_login_connection_target(&selected, OAuthProviderIdentity::GitHubCopilot)
            .unwrap();
    }
    assert!(
        startup.persistence.is_none(),
        "configuration is not a model/session or sign-in"
    );
}

#[tokio::test]
async fn two_roots_same_realm_binding_keep_native_credentials_separate_on_reopen() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut first = startup(a.path()).await;
    let mut other = startup(b.path()).await;
    let (bridge, _messages, _retire) = bridge();
    let (target, request) = key_target(&mut first, &bridge).await;
    // Same nonsecret native binding graph, different explicit store owner.
    let section =
        serde_json::to_value(first.config.realm.get("provider-control").unwrap()).unwrap();
    crate::realm::own_config_runtime(&other.storage)
        .unwrap()
        .patch(
            meerkat_core::ConfigDelta(json!({"realm":{"provider-control":section}})),
            None,
        )
        .await
        .unwrap();
    current(&mut other, &target, &request).await.unwrap();
    let persistence = first
        .factory
        .resolution_provider_auth_persistence()
        .unwrap()
        .unwrap();
    let interrupt = std::future::pending::<io::Result<()>>();
    tokio::pin!(interrupt);
    assert!(
        save_api_key(
            &mut first,
            &target,
            persistence,
            meerkat_core::auth::token_store::PersistedTokens::api_key("isolated-fixture-key"),
            &mut io::sink(),
            interrupt.as_mut()
        )
        .await
        .unwrap()
    );
    let key = meerkat_core::auth::token_store::TokenKey::from_credential_identity(
        &target.credential_identity,
    );
    let first_store = first
        .factory
        .resolution_provider_auth_persistence()
        .unwrap()
        .unwrap()
        .token_store();
    let other_store = other
        .factory
        .resolution_provider_auth_persistence()
        .unwrap()
        .unwrap()
        .token_store();
    assert!(first_store.load(&key).await.unwrap().is_some());
    assert!(other_store.load(&key).await.unwrap().is_none());
    drop(first);
    let reopened = startup(a.path()).await;
    assert_eq!(
        reopened.provider_store_root,
        Some(a.path().join("credentials"))
    );
    assert!(
        reopened
            .factory
            .resolution_provider_auth_persistence()
            .unwrap()
            .unwrap()
            .token_store()
            .load(&key)
            .await
            .unwrap()
            .is_some()
    );
    assert!(other_store.load(&key).await.unwrap().is_none());
}

#[tokio::test]
async fn actual_provider_pipe_empty_realm_cancels_without_session_or_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let selected_scope = scope(dir.path());
    let (parent, child) = tokio::io::duplex(1024 * 1024);
    let (input, output) = tokio::io::split(child);
    let task = tokio::spawn(run_io(input, output, selected_scope));
    let (input, mut output) = tokio::io::split(parent);
    let mut input = BufReader::new(input);
    let observed = std::panic::AssertUnwindSafe(async {
        write(&mut output, init(dir.path())).await;
        assert_eq!(read(&mut input).await["kind"], "initialized");
        let prompt = read(&mut input).await;
        assert_eq!(prompt["prompt"]["kind"], "choices");
        assert!(
            prompt["prompt"]["choices"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["configured"] == false)
        );
        write(&mut output, json!({"method":"cancel","id":"stop"})).await;
        loop {
            if read(&mut input).await["kind"] == "cancelled" {
                break;
            }
        }
    })
    .catch_unwind()
    .await;
    // EOF/error retirement is still joined if an assertion or read fails.
    drop(output);
    drop(input);
    let joined = task.await;
    if let Err(panic) = observed {
        std::panic::resume_unwind(panic);
    }
    joined.unwrap().unwrap();
    let reopened = startup(dir.path()).await;
    assert!(configured_choices(&reopened).unwrap().is_empty());
    assert!(reopened.persistence.is_none());
}

#[tokio::test]
async fn provider_pipe_rejects_browser_store_rebinding_before_native_open() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let (mut parent, child) = tokio::io::duplex(16384);
    let (input, output) = tokio::io::split(child);
    let task = tokio::spawn(run_io(input, output, scope(dir.path())));
    let observed = std::panic::AssertUnwindSafe(write(&mut parent, init(other.path())))
        .catch_unwind()
        .await;
    drop(parent);
    let joined = task.await;
    if let Err(panic) = observed {
        std::panic::resume_unwind(panic);
    }
    assert!(joined.unwrap().is_err());
    assert!(std::fs::read_dir(other.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn manual_callback_winner_closes_actual_native_listener_and_rejects_substitution() {
    let binding = bind_loopback_callback_with_redirect("/callback", "127.0.0.1", &[0])
        .await
        .unwrap();
    let redirect = binding.redirect_url.clone();
    let handle = binding.expect_state("retained-state".into());
    let (bridge, mut messages, retire) = bridge();
    let raw = format!("{redirect}?state=retained-state&code=private-code");
    let owner = auth::select_callback(
        handle,
        &redirect,
        "retained-state",
        "https://native.invalid/authorize",
        BrowserTransport::ManualReturn,
        &bridge,
    );
    let answer = async {
        let observed = std::panic::AssertUnwindSafe(async {
            let Message::Prompt(prompt) =
                tokio::time::timeout(Duration::from_secs(5), messages.recv())
                    .await
                    .unwrap()
                    .unwrap()
            else {
                panic!("expected prompt")
            };
            prompt
                .reply
                .send(Answer::Callback { url: raw })
                .ok()
                .unwrap();
        })
        .catch_unwind()
        .await;
        if observed.is_err() {
            retire.send_replace(true);
        }
        observed
    };
    let (selected, answered) = tokio::join!(owner, answer);
    if let Err(panic) = answered {
        std::panic::resume_unwind(panic);
    }
    assert_eq!(selected.unwrap().unwrap().0, "retained-state");
    let url = reqwest::Url::parse(&redirect).unwrap();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", url.port().unwrap()))
        .await
        .unwrap();
    drop(listener);
    for invalid in [
        format!("{redirect}?state=wrong&code=private-code"),
        format!("{redirect}?state=retained-state&code=one&code=two"),
        format!("{redirect}/other?state=retained-state&code=private-code"),
        format!("{redirect}?state=retained-state&code=private-code#fragment"),
    ] {
        assert!(auth::callback(&invalid, &redirect, "retained-state").is_err());
    }
}

#[tokio::test]
async fn cancelled_entered_device_request_joins_same_future_before_returning() {
    let (release, entered) = oneshot::channel::<()>();
    let (cancel, retired) = oneshot::channel::<()>();
    let (started, starting) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let request = async {
            started.send(()).unwrap();
            entered.await.unwrap();
            Ok::<_, meerkat::HostAuthError>(17u8)
        };
        let interrupt = async {
            retired.await.unwrap();
            Ok(())
        };
        tokio::pin!(interrupt);
        settle_device_request(
            request,
            "fixture native request",
            &mut io::sink(),
            interrupt.as_mut(),
        )
        .await
    });
    starting.await.unwrap();
    cancel.send(()).unwrap();
    tokio::task::yield_now().await;
    assert!(
        !task.is_finished(),
        "retiring observer must not drop entered request"
    );
    release.send(()).unwrap();
    let result = task.await.unwrap().unwrap().unwrap();
    assert_eq!(result.0, 17);
    assert!(result.1.is_some());
}

#[tokio::test]
async fn immutable_viewer_deadline_retires_bridge_without_driver_watch_progress() {
    let (mut bridge, _messages, _retire) = bridge();
    bridge.deadline = wire::now() + 25;
    bridge.expires = tokio::time::Instant::now() + Duration::from_millis(25);
    // Sender remains live and unchanged, as when the output owner is blocked.
    tokio::time::timeout(Duration::from_secs(2), bridge.closed())
        .await
        .unwrap();
    assert!(bridge.retired());
    assert!(matches!(
        bridge.ask(json!({"kind":"key"})).await.unwrap(),
        Answer::Cancel
    ));
}

#[tokio::test]
async fn accepted_cancel_retires_native_bridge_before_backpressured_output() {
    let (bridge, _messages, retire) = bridge();
    let (mut output, mut input) = tokio::io::duplex(1);
    let observer = retire_before_output(&retire, &mut output, json!({"kind":"cancelling"}));
    tokio::pin!(observer);
    assert!(observer.as_mut().now_or_never().is_none());
    tokio::time::timeout(Duration::from_secs(1), bridge.closed())
        .await
        .unwrap();
    assert!(bridge.retired());
    // Release this actual output owner and join it, rather than abandon the
    // pending write just because retirement was already observable.
    let drain = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut buf = [0; 64];
            use tokio::io::AsyncReadExt;
            let expected = format!("{}\n", json!({"kind":"cancelling"})).len();
            let mut count = 0;
            while count < expected {
                let read = input.read(&mut buf).await?;
                if read == 0 {
                    return Err(io::Error::other("output ended before cancellation frame"));
                }
                count += read;
            }
            Ok::<_, io::Error>(())
        })
        .await
    };
    let (result, drained) = tokio::join!(observer, drain);
    // The emitting owner has settled before any assertion can unwind.
    result.unwrap();
    drained.unwrap().unwrap();
}

#[tokio::test]
async fn inherited_target_added_during_review_refuses_before_own_cas() {
    let dir = tempfile::tempdir().unwrap();
    let mut startup = startup(dir.path()).await;
    let (bridge, _messages, _retire) = bridge();
    let choices = target::catalog(&mut startup).await.unwrap();
    let choice = choices
        .iter()
        .find(|c| c.enabled && c.provider == meerkat_core::Provider::OpenAI && c.auth == "api_key")
        .unwrap();
    let prepared = target::prepare(&mut startup, choice).await.unwrap();
    let source = crate::realm::config_source(&startup.storage).unwrap();
    let own_path = source.config_doc_path(&startup.storage.locator.realm);
    let before = std::fs::read(&own_path).ok();
    let parent = meerkat_core::FileConfigStore::new(
        startup
            .storage
            .layout
            .user_rkat_root()
            .unwrap()
            .join("config.toml"),
        meerkat_models::canonical(),
    );
    use meerkat_core::ConfigStore;
    parent.patch(meerkat_core::ConfigDelta(json!({"realm":{"global":{
        "backend":{"operator":{"provider":"openai","backend_kind":choice.backend}},
        "auth":{"operator":{"provider":"openai","auth_method":"api_key","source":{"kind":"managed_store"}}},
        "binding":{"operator":{"backend_profile":"operator","auth_profile":"operator","default_model":choice.model}},
        "default_binding":"operator"}}}))).await.unwrap();
    // Prove the actual reader sees the inherited target before the refusal.
    startup.config = crate::realm::read_config(&startup.storage).await.unwrap();
    assert!(!configured_choices(&startup).unwrap().is_empty());
    assert!(prepared.commit(&mut startup, &bridge).await.is_err());
    assert_eq!(std::fs::read(&own_path).ok(), before);
}

#[tokio::test]
async fn offered_new_targets_pass_native_construction_preview_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut startup = startup(dir.path()).await;
    let choices = target::catalog(&mut startup).await.unwrap();
    assert!(choices.iter().any(|c| c.enabled));
    for choice in choices.iter().filter(|c| c.enabled) {
        assert!(choice.target.is_none());
        target::prepare(&mut startup, choice).await.unwrap();
    }
    assert!(configured_choices(&startup).unwrap().is_empty());
    assert!(startup.persistence.is_none());
}

// A GitHub Copilot account configured once, by a `[copilot]` section. Core
// lowers it into the account's three routes. The token variable stays unset:
// no credential is read, requested or stored.
const COPILOT_SECTION: &str =
    "source = {kind = \"env\", env = \"CONNECT_COPILOT_SECTION_TOKEN\"}\n";

fn copilot_routes(realm: &meerkat_core::RealmId) -> Vec<meerkat_core::AuthBindingRef> {
    ["copilot_anthropic", "copilot_gemini", "copilot_openai"]
        .into_iter()
        .map(|route| meerkat_core::AuthBindingRef {
            realm: realm.clone(),
            binding: BindingId::parse(route).unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        })
        .collect()
}

#[tokio::test]
async fn copilot_section_routes_are_configured_targets() {
    let dir = tempfile::tempdir().unwrap();
    let selected = storage(dir.path());
    let own = crate::realm::config_source(&selected)
        .unwrap()
        .config_doc_path(&selected.locator.realm);
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::write(
        &own,
        format!("[realm.provider-control.copilot]\n{COPILOT_SECTION}"),
    )
    .unwrap();
    let mut startup = startup(dir.path()).await;
    let routes = copilot_routes(&selected.locator.realm);
    assert_eq!(configured_choices(&startup).unwrap(), routes);
    // The browser presentation offers the routes as configured targets, each
    // with its own family's models, and no first target next to them.
    let choices = target::catalog(&mut startup).await.unwrap();
    for route in &routes {
        assert!(
            choices
                .iter()
                .any(|c| c.target.as_ref().is_some_and(|t| t.auth_binding == *route)),
            "{route:?} is not offered"
        );
    }
    assert!(choices.iter().all(|c| {
        c.target
            .as_ref()
            .is_some_and(|t| t.backend.provider == c.provider)
    }));
    assert!(startup.persistence.is_none());
}

#[tokio::test]
async fn copilot_section_added_during_review_refuses_first_target() {
    let dir = tempfile::tempdir().unwrap();
    let mut startup = startup(dir.path()).await;
    let (bridge, _messages, _retire) = bridge();
    let choices = target::catalog(&mut startup).await.unwrap();
    let choice = choices
        .iter()
        .find(|c| c.enabled && c.provider == meerkat_core::Provider::OpenAI && c.auth == "api_key")
        .unwrap();
    let prepared = target::prepare(&mut startup, choice).await.unwrap();
    let own = crate::realm::config_source(&startup.storage)
        .unwrap()
        .config_doc_path(&startup.storage.locator.realm);
    let before = std::fs::read(&own).ok();
    // The account is then configured once, in the global document the realm
    // inherits. The realm has model targets, so no first target is added.
    let global = startup
        .storage
        .layout
        .user_rkat_root()
        .unwrap()
        .join("config.toml");
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::write(
        &global,
        format!("[realm.global.copilot]\n{COPILOT_SECTION}"),
    )
    .unwrap();
    assert!(target::prepare(&mut startup, choice).await.is_err());
    assert!(prepared.commit(&mut startup, &bridge).await.is_err());
    assert_eq!(std::fs::read(&own).ok(), before);
    startup.config = crate::realm::read_config(&startup.storage).await.unwrap();
    assert_eq!(
        configured_choices(&startup).unwrap(),
        copilot_routes(&meerkat_core::RealmId::global())
    );
    assert!(startup.persistence.is_none());
}

#[test]
fn initialization_timer_refuses_platform_overflow_without_panicking() {
    let now = tokio::time::Instant::now();
    assert_eq!(
        checked_presentation_expiry(now, Duration::from_millis(25)).unwrap() - now,
        Duration::from_millis(25)
    );
    assert!(matches!(
        checked_presentation_expiry(now, Duration::MAX),
        Err(ConnectError::InvalidInput(_))
    ));
}

#[test]
fn launch_environment_keeps_auth_and_config_excluded_with_only_scoped_runtime_metadata() {
    use std::ffi::OsStr;
    let accepts = |native_default, key, value| {
        provider_environment_entry(native_default, OsStr::new(key), OsStr::new(value))
    };
    for key in ["PATH", "LANG", "LC_ALL", "TMPDIR"] {
        assert!(accepts(false, key, "host-value"));
    }
    for key in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "RKAT_CONFIG",
        "MEERKAT_CONFIG",
        "HOME",
        "XDG_CONFIG_HOME",
    ] {
        assert!(!accepts(false, key, "forbidden"));
    }
    assert!(accepts(true, "HOME", "/fixed-host-home"));
    for key in [
        "OPENAI_API_KEY",
        "RKAT_CONFIG",
        "__CF_USER_TEXT_ENCODING_EXTRA",
    ] {
        assert!(!accepts(true, key, "forbidden"));
    }
    let key = "__CF_USER_TEXT_ENCODING";
    for value in [
        "",
        "0x0",
        "0x0:0x0:0x0:0x0",
        "0x:0x0:0x0",
        "0x100000000:0x0:0x0",
        "0x0:0xg:0x0",
        "0x0:0x0:0x0\n",
    ] {
        assert!(!accepts(false, key, value));
    }
    #[cfg(target_os = "macos")]
    {
        let uid = nix::unistd::geteuid().as_raw();
        let actual = format!("0x{uid:X}:0x0:0x0");
        assert_eq!(
            accepts(false, key, &actual),
            nix::unistd::getuid() == nix::unistd::geteuid()
        );
        assert!(!accepts(
            false,
            key,
            &format!("0x{:X}:0x0:0x0", uid.wrapping_add(1))
        ));
    }
    #[cfg(not(target_os = "macos"))]
    assert!(!accepts(false, key, "0x1F5:0x0:0x0"));
}
