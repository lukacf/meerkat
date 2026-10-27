//! Narrow local controls. Executable owner composition is tested separately.
use super::*;
use std::os::unix::fs::PermissionsExt;
use tokio::{
    sync::Notify,
    time::{Duration, timeout},
};

fn host() -> ForgeHost {
    ForgeHost {
        protocol: 1,
        host_id: format!("forge:{}", "a".repeat(64)),
        application_id: "reading-fixture".into(),
        source_digest: "b".repeat(64),
    }
}
fn activation() -> Value {
    json!({"state":"not-started","binding":"reading-private-text-dm","application_started":false,
        "active_revision":null,"provider_observation":null,"native_admission":null,"reply_receipt":null,
        "ownership_scope":"same-application-local-locks-and-explicit-human-acknowledgement",
        "retained_history":null,"unfinished_management":["pause","disable","remove","pending-work-revocation"]})
}
fn inventory() -> Value {
    json!({"storage":"available","revision":0,"configuration_status":"not-configured","configuration":null,
        "binding":null,"binding_matches_current_source":null,"owner_principal":"fixture-human",
        "participants":[{"id":"reading-owner","label":"Reading owner","principal":"fixture-human","telegram_user_id":null,"allowed_profile":"private-text-dm"}],
        "routes":[{"id":"reading-lead-dm","label":"Reading guide","participant_id":"reading-owner","native_identity":"lead:main","destination_rule":"same-authorized-human-private-dm","telegram_dm_chat_id":null}],
        "component":{"id":"reading-telegram","profile":"private-text-dm","support":"private-text-dm-source-candidate","native_identity":"lead:main","private_reference":{"private_field":"READING_TELEGRAM_BOT_TOKEN"},"participant":"Explicit human ID","outside_first_profile":["groups","topics","media","multiple-participants"]},"activation":activation()})
}
fn args() -> Value {
    json!({"participant_id":"reading-owner","route_id":"reading-lead-dm","telegram_user_id":123456,"exclusive_polling_acknowledged":true,"expected_revision":0})
}
pub(super) fn reference() -> RecoveryReference {
    let body = reading::prepare_body(&args().to_string(), &inventory()).unwrap();
    let key = format!("reading-telegram:{}", "c".repeat(32));
    let proposal = json!({"change_id":key,"configuration":body["configuration"],"expected_revision":0,"host":host(),"principal":"fixture-human","expires_at":now()+300});
    RecoveryReference {
        schema: 1,
        origin: "http://127.0.0.1:12345".into(),
        host: host(),
        change_id: key,
        proposal: RawValue::from_string(proposal.to_string()).unwrap(),
        application_private_fields: None,
    }
}

#[test]
fn forge_reading_choices_and_new_ids_are_closed_without_losing_historical_ids() {
    let observed = inventory();
    let expected = reading::prepare_body(&args().to_string(), &observed).unwrap();
    assert_eq!(
        expected["configuration"]["native_identity"],
        observed["routes"][0]["native_identity"]
    );
    assert_eq!(
        expected["configuration"]["bot_token"],
        observed["component"]["private_reference"]
    );
    for (key, value) in [
        ("telegram_user_id", json!(0)),
        ("telegram_user_id", json!(1_u64 << 52)),
        ("telegram_user_id", json!(true)),
        ("telegram_user_id", json!(1.0)),
        ("participant_id", json!("other")),
        ("route_id", json!("other")),
        ("exclusive_polling_acknowledged", json!(false)),
        ("expected_revision", json!(1)),
    ] {
        let mut value_args = args();
        value_args[key] = value;
        assert!(
            reading::prepare_body(&value_args.to_string(), &observed).is_err(),
            "{key}"
        );
    }
    let mut extra = args();
    extra["bot_token"] = json!("private-canary");
    assert!(reading::prepare_body(&extra.to_string(), &observed).is_err());
    let duplicate = args()
        .to_string()
        .replacen('{', "{\"telegram_\\u0075ser_id\":9,", 1);
    assert!(reading::prepare_body(&duplicate, &observed).is_err());
    let mut historical = reference();
    let mut proposal: Value = parse(historical.proposal.get()).unwrap();
    proposal["configuration"]["telegram_user_id"] = json!((1_u64 << 53) - 1);
    proposal["configuration"]["telegram_dm_chat_id"] =
        proposal["configuration"]["telegram_user_id"].clone();
    historical.proposal = RawValue::from_string(proposal.to_string()).unwrap();
    reading::retained(&proposal, &historical).unwrap();
    assert!(reading::proposal(&proposal, &expected, &host()).is_err());
}

#[test]
fn forge_status_preserves_exact_proposal_source_and_historical_result() {
    let reference = reference();
    let proposal: Value = parse(reference.proposal.get()).unwrap();
    let result = json!({"change_id":reference.change_id,"status":"committed","revision":1,"configuration_status":"committed","activation_at_submission":activation(),"approval":{"approved":true,"principal":"fixture-human","expires_at":now()+200}});
    reading::submission(&result, &reference).unwrap();
    let mut status = json!({"change_id":reference.change_id,"status":"committed","proposal":proposal,"result":result,"storage":"available","binding_matches_current_source":true,"activation":activation()});
    reading::status(&status, &reference, &host()).unwrap();
    let mut changed = host();
    changed.source_digest = "d".repeat(64);
    assert!(reading::status(&status, &reference, &changed).is_err());
    status["binding_matches_current_source"] = json!(false);
    reading::status(&status, &reference, &changed).unwrap();
    for key in ["principal", "configuration", "host"] {
        let mut wrong = status.clone();
        wrong["proposal"][key] = Value::Null;
        assert!(reading::status(&wrong, &reference, &changed).is_err());
    }
    let mut wrong = status.clone();
    wrong["result"]["revision"] = json!(2);
    assert!(reading::status(&wrong, &reference, &changed).is_err());
    let unknown = json!({"change_id":reference.change_id,"status":"unknown","storage":"available","activation":activation()});
    reading::status(&unknown, &reference, &changed).unwrap();
}

#[tokio::test]
async fn forge_presenter_handoff_and_unsafe_recovery_do_not_disable_services() {
    let (paired_host, _) = crate::paired_host_tool::PairedHostFrontend::deferred();
    let (lazy, _) = crate::service_preparation::LazySetup::new(paired_host);
    let empty = RawValue::from_string("{}".into()).unwrap();
    assert!(lazy.forge().tools().is_empty());
    assert!(
        lazy.forge()
            .dispatch(ToolCallView {
                id: "untaken",
                name: FORGE_SELECT_HOST,
                args: &empty
            })
            .await
            .is_err()
    );
    let presenter = lazy.take_forge_presenter().unwrap();
    assert!(!lazy.forge().tools().is_empty());
    drop(presenter);
    assert!(
        lazy.forge()
            .dispatch(ToolCallView {
                id: "dropped",
                name: FORGE_SELECT_HOST,
                args: &empty
            })
            .await
            .is_err()
    );

    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let old = tempfile::Builder::new()
        .prefix("setup-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root.path())
        .unwrap();
    let current = tempfile::Builder::new()
        .prefix("setup-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root.path())
        .unwrap();
    std::fs::write(old.path().join("forge-change-bad.json"), b"{}").unwrap();
    let (paired_host, _) = crate::paired_host_tool::PairedHostFrontend::deferred();
    let (lazy, _) = crate::service_preparation::LazySetup::new(paired_host);
    assert!(lazy.set_forge_artifacts(current.path()).is_ok());
    let _presenter = lazy.take_forge_presenter().unwrap();
    assert!(!lazy.forge().tools().is_empty());
    let page = lazy
        .forge()
        .binding
        .configuration
        .lock()
        .unwrap()
        .discoveries
        .as_mut()
        .unwrap()
        .page();
    assert_eq!(page.skipped, 1);
    let options = lazy
        .session_options(crate::SessionOptions::default())
        .unwrap();
    assert!(
        options
            .build
            .external_tools
            .unwrap()
            .tools()
            .iter()
            .any(|t| t.name == crate::service_preparation::TOOLKIT_PREPARE_SERVICES)
    );
}

#[test]
fn forge_recovery_is_atomic_private_bounded_and_never_an_approval() {
    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let first = tempfile::Builder::new()
        .prefix("setup-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root.path())
        .unwrap();
    let second = tempfile::Builder::new()
        .prefix("setup-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root.path())
        .unwrap();
    let reference = reference();
    recovery::save(first.path(), &reference).unwrap();
    let found = recovery::discover(second.path()).unwrap().page();
    assert_eq!(found.references.len(), 1);
    assert_eq!(found.references[0].proposal.get(), reference.proposal.get());
    let path = std::fs::read_dir(first.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let raw = std::fs::read_to_string(&path).unwrap();
    for forbidden in ["proof", "cookie", "password", "approved"] {
        assert!(!raw.contains(forbidden));
    }
    let mut changed = reference.clone();
    changed.origin = "http://127.0.0.1:12346".into();
    assert!(recovery::save(first.path(), &changed).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(recovery::discover(second.path()).unwrap().page().skipped, 1);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let outside = root.path().join("outside.json");
    std::fs::rename(&path, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert_eq!(recovery::discover(second.path()).unwrap().page().skipped, 1);
}

#[test]
fn forge_recovery_pages_have_no_lifetime_setup_or_reference_limit() {
    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    for i in 0..1030 {
        let path = root.path().join(format!("setup-old-{i}"));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let current = tempfile::Builder::new()
        .prefix("setup-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root.path())
        .unwrap();
    for i in 0..140 {
        let mut reference = reference();
        reference.change_id = format!("reading-telegram:{i:032x}");
        let mut proposal: Value = parse(reference.proposal.get()).unwrap();
        proposal["change_id"] = json!(reference.change_id);
        reference.proposal = RawValue::from_string(proposal.to_string()).unwrap();
        let path = current.path().join(format!("forge-change-{i}.json"));
        std::fs::write(&path, serde_json::to_vec(&reference).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut scan = recovery::discover(current.path()).unwrap();
    let mut found = BTreeSet::new();
    let mut pages = 0;
    loop {
        let page = scan.page();
        pages += 1;
        assert!(page.references.len() <= 128);
        assert_eq!(page.skipped, 0);
        for reference in page.references {
            assert!(found.insert(reference.change_id));
        }
        if !page.more {
            break;
        }
        assert!(pages < 40);
    }
    assert_eq!(found.len(), 140);
    assert!(pages > 1);
}

#[tokio::test]
async fn forge_recovery_menu_joins_closed_tool_waiter_without_login() {
    let root = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let previous = tempfile::Builder::new()
        .prefix("setup-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root.path())
        .unwrap();
    let current = tempfile::Builder::new()
        .prefix("setup-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(root.path())
        .unwrap();
    recovery::save(previous.path(), &reference()).unwrap();
    let (front, mut presenter) = ForgeFrontend::new();
    front.artifacts(current.path()).unwrap();
    let caller = front.clone();
    let task = tokio::spawn(async move {
        let args = RawValue::from_string("{}".into()).unwrap();
        caller
            .dispatch(ToolCallView {
                id: "cancel-menu",
                name: FORGE_SELECT_HOST,
                args: &args,
            })
            .await
    });
    let prompt = presenter.next().await.unwrap();
    let pty = nix::pty::openpty(None, None).unwrap();
    let original = nix::sys::termios::tcgetattr(&pty.slave).unwrap();
    let observed = pty.slave.try_clone().unwrap();
    let mut input = TerminalInput::from_owned_fd(pty.slave);
    let mut output = Vec::new();
    {
        let presentation =
            present_forge_selection(prompt, &mut input, &mut output, std::future::pending());
        tokio::pin!(presentation);
        assert!(
            timeout(Duration::from_millis(25), &mut presentation)
                .await
                .is_err()
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(
            timeout(Duration::from_secs(2), presentation)
                .await
                .unwrap()
                .is_err()
        );
    }
    assert_eq!(nix::sys::termios::tcgetattr(&observed).unwrap(), original);
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("Previous Forge changes"));
    assert!(!output.contains("password"));
}

#[tokio::test]
async fn forge_recovery_menu_user_exit_preserves_locator_without_host_or_approval() {
    for stop in ["quit", "eof", "interrupt", "signal_error", "waiter_closed"] {
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let previous = tempfile::Builder::new()
            .prefix("setup-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(root.path())
            .unwrap();
        let current = tempfile::Builder::new()
            .prefix("setup-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(root.path())
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut saved = reference();
        saved.origin = format!("http://{}", listener.local_addr().unwrap());
        recovery::save(previous.path(), &saved).unwrap();
        let locator = std::fs::read_dir(previous.path()).unwrap().next().unwrap().unwrap().path();
        let original_locator = std::fs::read(&locator).unwrap();
        let (front, mut presenter) = ForgeFrontend::new();
        front.artifacts(current.path()).unwrap();
        let caller = front.clone();
        let task = tokio::spawn(async move {
            let args = RawValue::from_string("{}".into()).unwrap();
            caller.dispatch(ToolCallView {
                id: "exit-recovery-menu", name: FORGE_SELECT_HOST, args: &args,
            }).await
        });
        let prompt = presenter.next().await.unwrap();
        let pty = nix::pty::openpty(None, None).unwrap();
        let original_mode = nix::sys::termios::tcgetattr(&pty.slave).unwrap();
        let original_flags = nix::fcntl::fcntl(&pty.slave, nix::fcntl::FcntlArg::F_GETFL).unwrap();
        let observer = pty.slave.try_clone().unwrap();
        let mut input = TerminalInput::from_owned_fd(pty.slave);
        let mut output = Vec::new();
        let (signal, interrupted) = oneshot::channel();
        {
            let presentation = present_forge_selection(prompt, &mut input, &mut output, async {
                interrupted.await.unwrap()
            });
            tokio::pin!(presentation);
            assert!(timeout(Duration::from_millis(25), &mut presentation).await.is_err());
            match stop {
                "quit" => { nix::unistd::write(&pty.master, b"/quit\n").unwrap(); }
                "eof" => { nix::unistd::write(&pty.master, b"\x04").unwrap(); }
                "interrupt" => signal.send(Ok(())).unwrap(),
                "signal_error" => signal.send(Err(io::Error::other("fixture signal failure"))).unwrap(),
                "waiter_closed" => task.abort(),
                _ => unreachable!(),
            }
            let result = timeout(Duration::from_secs(2), presentation).await.unwrap();
            if stop == "signal_error" {
                assert!(matches!(result, Err(ConnectError::Native {
                    operation: "interrupt Forge recovery", ..
                })));
            } else if stop == "waiter_closed" {
                assert!(result.is_err(), "a lost native handoff must remain a failure");
            } else {
                assert!(!result.unwrap(), "intentional menu exit must request normal CLI stop");
            }
        }
        let dispatched = timeout(Duration::from_secs(2), task).await.unwrap();
        if stop == "waiter_closed" {
            assert!(dispatched.unwrap_err().is_cancelled());
        } else {
            assert!(dispatched.unwrap().is_err());
        }
        assert_eq!(nix::sys::termios::tcgetattr(&observer).unwrap(), original_mode);
        assert_eq!(nix::fcntl::fcntl(&observer, nix::fcntl::FcntlArg::F_GETFL).unwrap(), original_flags);
        assert!(matches!(*front.binding.state.lock().unwrap(), State::Retired));
        let state = front.binding.configuration.lock().unwrap();
        assert!(state.recovered.is_none() && state.references.is_empty());
        assert!(state.prepared.is_empty() && state.armed.is_none());
        assert!(state.spent_calls.is_empty() && state.submitted.is_empty() && !state.busy);
        drop(state);
        assert_eq!(std::fs::read(&locator).unwrap(), original_locator);
        assert_eq!(std::fs::read_dir(previous.path()).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(current.path()).unwrap().count(), 0);
        assert!(timeout(Duration::from_millis(25), listener.accept()).await.is_err(),
                "leaving recovery must not enter login, prepare or submit HTTP");
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Previous Forge changes"));
        assert!(!output.contains("Forge Console origin") && !output.contains("password"));
    }
}

fn operation(
    front: &ForgeFrontend,
    origin: &str,
) -> (ForgeOperation, oneshot::Receiver<Result<Value, ForgeError>>) {
    let mut client = ForgeClient::new(origin).unwrap();
    client.session = Some(HeaderValue::from_static("fixture=session"));
    let (send, receive) = oneshot::channel();
    front.binding.configuration.lock().unwrap().busy = true;
    front.binding.configuration.lock().unwrap().uncertain = Some(None);
    (
        ForgeOperation {
            binding: front.binding.clone(),
            client,
            host: host(),
            request: Some(OperationRequest::Prepare {
                body: json!({}),
                principal: json!("fixture-human"),
            }),
            entered: false,
            response: Some(send),
        },
        receive,
    )
}
#[tokio::test]
async fn forge_operation_pre_ready_cancellation_or_closed_waiter_never_enters_http() {
    let (front, _presenter) = ForgeFrontend::new();
    let (op, receive) = operation(&front, "http://127.0.0.1:1");
    assert!(
        !present_operation(op, &mut Vec::new(), async { Ok(()) })
            .await
            .unwrap()
    );
    assert!(receive.await.is_err());
    assert!(!front.binding.configuration.lock().unwrap().busy);
    assert!(
        front
            .binding
            .configuration
            .lock()
            .unwrap()
            .uncertain
            .is_none()
    );
    let (op, receive) = operation(&front, "http://127.0.0.1:1");
    drop(receive);
    assert!(
        present_operation(op, &mut Vec::new(), std::future::pending())
            .await
            .is_err()
    );
    assert!(
        front
            .binding
            .configuration
            .lock()
            .unwrap()
            .uncertain
            .is_none()
    );
}

#[tokio::test]
async fn forge_operation_after_entry_joins_original_request_on_cancel() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (observed, allowed, count) = (entered.clone(), release.clone(), calls.clone());
    let app=axum::Router::new().fallback(move || {let (observed,allowed,count)=(observed.clone(),allowed.clone(),count.clone());async move {
        count.fetch_add(1,Ordering::SeqCst);observed.notify_one();allowed.notified().await;
        (axum::http::StatusCode::BAD_REQUEST,[("content-type","application/json"),("cache-control","no-store")],"{\"detail\":\"Invalid frontend command. Use nonsecret configuration and the exact proposal.\"}")
    }});
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap()
    });
    let (front, _presenter) = ForgeFrontend::new();
    let (op, receive) = operation(&front, &origin);
    let (signal, interrupted) = oneshot::channel();
    let mut output = Vec::new();
    let presentation = present_operation(op, &mut output, async {
        interrupted.await.unwrap();
        Ok(())
    });
    tokio::pin!(presentation);
    tokio::select! {result=&mut presentation=>panic!("premature completion: {result:?}"), _=entered.notified()=>{}}
    signal.send(()).unwrap();
    assert!(
        timeout(Duration::from_millis(25), &mut presentation)
            .await
            .is_err()
    );
    assert!(front.binding.configuration.lock().unwrap().busy);
    release.notify_one();
    assert!(
        !timeout(Duration::from_secs(5), presentation)
            .await
            .unwrap()
            .unwrap()
    );
    assert!(matches!(
        receive.await.unwrap(),
        Err(ForgeError::CommandRefused)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!front.binding.configuration.lock().unwrap().busy);
    stop.send(()).unwrap();
    server.await.unwrap();
}
