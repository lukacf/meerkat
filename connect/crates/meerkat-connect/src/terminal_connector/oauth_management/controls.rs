//! Authored native component controls. No live provider, installed Console or
//! full end-to-end qualification is implied by these fixtures.
use super::*;
use crate::stdio::tests::{
    connector_fixture_with_owners, connector_http, connector_http_reply, connector_metadata,
    connector_operations, connector_unprimed_fixture,
};
use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore, ProviderAuthPersistence};
use meerkat_core::mcp_config::McpConnectorBinding;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Default)]
struct Output {
    bytes: Arc<Mutex<Vec<u8>>>,
    fail_url: bool,
}
impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.bytes.lock().unwrap().clone()).unwrap()
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_url && String::from_utf8_lossy(bytes).contains("Open this private") {
            return Err(io::Error::other("controlled output failure"));
        }
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
async fn shown(output: &Output, needle: &str) {
    while !output.text().contains(needle) {
        tokio::task::yield_now().await;
    }
}
fn definition(storage: &ResolvedStorage, issuer: &str, name: &str) -> McpServerConfig {
    let mut config = McpServerConfig::stdio(name, "/bin/false", vec![], Default::default());
    config.access = Some(meerkat_core::McpServerAccess::allowing(["slack_auth_test"]));
    config.connector = Some(McpConnectorBinding {
        realm_id: storage.locator.realm.as_str().into(),
        slot_id: "test-account".into(),
        issuer: issuer.into(),
        client: "test-client".into(),
        resource: "https://service.example".into(),
        strategy_id: "oidc-userinfo-v1".into(),
        account: "verified-subject".into(),
        operations: [(
            "slack_auth_test".into(),
            McpConnectorOperation {
                required_scopes: ["openid".into()].into(),
                destination_origin: "https://service.example".into(),
                read_only: true,
            },
        )]
        .into(),
    });
    config
}
async fn fixture(root: &std::path::Path, issuer: &str) -> TerminalConnectors {
    let storage = crate::RealmOptions {
        context_root: root.into(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    }
    .resolve("work")
    .unwrap();
    let (_, bundle) = meerkat::storage_provider::open_realm_persistence_with_layout(
        storage.layout.clone(),
        storage.locator.realm.as_str(),
        None,
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .unwrap();
    drop(bundle);
    let principals = StockPrincipals::for_local_operator(&storage.locator.realm).unwrap();
    meerkat::governed_realm::stock::govern_stock_realm(
        &storage.locator.state_root,
        storage.locator.realm.as_str(),
        &principals,
    )
    .await
    .unwrap();
    let own = crate::realm::own_config_runtime(&storage).unwrap();
    meerkat_core::McpRealmPersistTarget::new(&own)
        .add(definition(&storage, issuer, "mail"), Some(0))
        .await
        .unwrap();
    let persistence = ProviderAuthPersistence::new(
        Arc::new(FileTokenStore::new(root.join("credentials"))),
        Arc::new(FileLockCoordinator::new(root.join("locks"))),
    );
    let (startup, auth, control) =
        connector_fixture_with_owners(root, issuer, storage.clone(), persistence, None).await;
    let profiles = Arc::new(meerkat::RealmConnectorClients::from_realm_source(
        crate::realm::config_source(&storage).unwrap(),
        storage.locator.realm.clone(),
    ));
    let (send, requests) = mpsc::channel(1);
    TerminalConnectors {
        storage,
        factory: startup.factory,
        auth,
        control,
        profiles,
        send,
        requests,
        reviewed: None,
        prior_finalizations: Arc::from([]),
    }
}
fn snapshot(storage: &ResolvedStorage) -> Vec<Option<Vec<u8>>> {
    let config = crate::realm::config_source(storage)
        .unwrap()
        .config_doc_path(&storage.locator.realm);
    let realm =
        meerkat_store::realm_paths_in(&storage.locator.state_root, storage.locator.realm.as_str());
    [
        config.clone(),
        config.with_file_name("config_state.json"),
        meerkat_store::grant_owner_store_path(&realm.root),
    ]
    .iter()
    .map(|path| std::fs::read(path).ok())
    .collect()
}
async fn callback(output: &Output) {
    callback_outcome(output, false).await;
}
async fn callback_outcome(output: &Output, refused: bool) {
    shown(output, "Waiting for native sign-in.").await;
    let text = output.text();
    let url = reqwest::Url::parse(
        text.lines()
            .find(|line| line.starts_with("http://"))
            .unwrap(),
    )
    .unwrap();
    let query = url.query_pairs().collect::<BTreeMap<_, _>>();
    let mut redirect = reqwest::Url::parse(query.get("redirect_uri").unwrap()).unwrap();
    redirect
        .query_pairs_mut()
        .append_pair("state", query.get("state").unwrap());
    if refused {
        redirect
            .query_pairs_mut()
            .append_pair("error", "access_denied");
    } else {
        redirect
            .query_pairs_mut()
            .append_pair("code", "fixture-code");
    }
    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", redirect.port().unwrap()))
        .await
        .unwrap();
    let path = format!("{}?{}", redirect.path(), redirect.query().unwrap());
    socket
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 "));
    if !refused {
        assert!(response.starts_with(b"HTTP/1.1 200"));
    }
}
async fn issuer(
    listener: tokio::net::TcpListener,
    origin: String,
    tokens: Arc<AtomicUsize>,
    stop: oneshot::Receiver<()>,
) {
    issuer_gated(listener, origin, tokens, stop, None).await;
}
async fn issuer_gated(
    listener: tokio::net::TcpListener,
    origin: String,
    tokens: Arc<AtomicUsize>,
    mut stop: oneshot::Receiver<()>,
    mut gate: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
) {
    loop {
        let (mut socket, _) = tokio::select! { biased; _ = &mut stop => break, value = listener.accept() => value.unwrap() };
        let (head, body) = connector_http(&mut socket).await;
        let value = if head.starts_with("POST /token ") {
            tokens.fetch_add(1, Ordering::SeqCst);
            assert!(
                String::from_utf8(body)
                    .unwrap()
                    .contains("code=fixture-code")
            );
            if let Some((entered, release)) = gate.take() {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            json!({"access_token":"private-native-token-canary","token_type":"Bearer","expires_in":3600,"scope":"openid"})
        } else if head.starts_with("GET /userinfo ") {
            json!({"sub":"verified-subject"})
        } else {
            assert!(head.starts_with("GET /.well-known/"));
            connector_metadata(&origin)
        };
        connector_http_reply(&mut socket, value).await;
    }
}

#[tokio::test]
async fn entered_completion_and_cancel_race_join_once_for_initial_and_existing_selection() {
    for kind in ["initial", "existing"] {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut owner = fixture(root.path(), &origin).await;
        let selected = ExistingConnector::select(&owner.storage, "mail")
            .await
            .unwrap();
        let mut target = selected.target(["openid".into()].into(), Some("test-profile".into()));
        let installation =
            connector_unprimed_fixture(owner.storage.locator.clone(), SessionId::new(), false);
        installation.prime(true).await;
        let selection = if kind == "initial" {
            target.account = meerkat_auth_core::connector_oauth::AccountSelection::Discover;
            LoginSelection::Installed(Arc::new(
                installation.selection(connector_operations()).unwrap(),
            ))
        } else {
            LoginSelection::Existing(Arc::new(selected))
        };
        let before = snapshot(&owner.storage);
        let tokens = Arc::new(AtomicUsize::new(0));
        let (finish, stopped) = oneshot::channel();
        let (entered, wait_entered) = oneshot::channel();
        let (release, wait_release) = oneshot::channel();
        let mut release = Some(release);
        let server = tokio::spawn(issuer_gated(
            listener,
            origin,
            tokens.clone(),
            stopped,
            Some((entered, wait_release)),
        ));
        let start = owner
            .control
            .start_selected(
                target.clone(),
                "https://console.example/callback".into(),
                selection,
            )
            .await;
        let started = owner.control.started().cloned();
        if start.is_err() || started.is_none() {
            let cleanup = owner.control.retire();
            let _ = finish.send(());
            let joined = server.await;
            start.unwrap();
            assert!(
                started.is_some(),
                "native start must retain the exact ceremony"
            );
            cleanup.unwrap();
            joined.unwrap();
            unreachable!();
        }
        let started = started.unwrap();
        let (_live, mut retired) = watch::channel(false);
        let (cancel, receive) = oneshot::channel();
        let mut cancel = Some(cancel);
        let (completion, primary) = {
            // The observation borrows this entered effect. Neither its timeout
            // nor its caught assertion can drop the retained completion.
            let completing = owner.control.complete(
                started.state,
                "fixture-code".into(),
                started.host_attempt_handle,
                &mut retired,
                receive,
            );
            tokio::pin!(completing);
            let mut completed = None;
            let primary = {
                let race = std::panic::AssertUnwindSafe(async {
                    wait_entered.await.unwrap();
                    cancel.take().unwrap().send(()).unwrap();
                    release.take().unwrap().send(()).unwrap();
                })
                .catch_unwind();
                tokio::pin!(race);
                tokio::time::timeout(Duration::from_secs(60), async {
                    tokio::select! {
                        result = completing.as_mut() => { completed = Some(result); race.await },
                        result = race.as_mut() => result,
                    }
                })
                .await
            };
            if let Some(cancel) = cancel.take() {
                let _ = cancel.send(());
            }
            if let Some(release) = release.take() {
                let _ = release.send(());
            }
            let completion = match completed {
                Some(result) => result,
                None => completing.await,
            };
            (completion, primary)
        };
        let (reply, _cancelled, failed) = completion;
        let settled = settle_terminal_completion(&mut owner.control, reply, failed);
        let _ = finish.send(());
        let issuer_joined = server.await;
        // Report the original observer failure only after the gate, exact
        // native effect, attempt settlement and issuer task have all settled.
        match primary {
            Ok(Err(panic)) => std::panic::resume_unwind(panic),
            Err(error) => panic!("completion cancellation observation timed out: {error}"),
            Ok(Ok(())) => {}
        }
        issuer_joined.unwrap();
        let joined = settled.unwrap();
        let status = owner
            .auth
            .facade
            .connector_status(&target.slot)
            .await
            .unwrap();
        if let Ok(completed) = joined {
            assert_eq!(status.phase, meerkat::ConnectorAuthPhase::Authorized);
            assert_eq!(
                status.verified_account.unwrap(),
                completed.native.verified_account
            );
            owner.control.retire().unwrap();
            assert!(owner.control.completed().is_some());
        } else {
            assert_eq!(
                status.phase,
                meerkat::ConnectorAuthPhase::AuthorizationRequired
            );
            assert!(owner.control.pending_slot().is_none());
        }
        assert_eq!(
            tokens.load(Ordering::SeqCst),
            1,
            "an entered exchange is joined, never replayed"
        );
        assert_eq!(snapshot(&owner.storage), before);
    }
}

#[tokio::test]
async fn terminal_cancel_during_exchange_keeps_its_winner_after_caller_loss() {
    for kind in ["initial", "existing"] {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut owner = fixture(root.path(), &origin).await;
        let selected = ExistingConnector::select(&owner.storage, "mail")
            .await
            .unwrap();
        let mut target = selected.target(["openid".into()].into(), None);
        let installation =
            connector_unprimed_fixture(owner.storage.locator.clone(), SessionId::new(), false);
        installation.prime(true).await;
        let selection = if kind == "initial" {
            target.account = meerkat_auth_core::connector_oauth::AccountSelection::Discover;
            LoginSelection::Installed(Arc::new(
                installation.selection(connector_operations()).unwrap(),
            ))
        } else {
            LoginSelection::Existing(Arc::new(selected))
        };
        let before = snapshot(&owner.storage);
        let tokens = Arc::new(AtomicUsize::new(0));
        let (finish, stopped) = oneshot::channel();
        let (entered, wait_entered) = oneshot::channel();
        let (release, wait_release) = oneshot::channel();
        let mut release = Some(release);
        let server = tokio::spawn(issuer_gated(
            listener,
            origin,
            tokens.clone(),
            stopped,
            Some((entered, wait_release)),
        ));
        let (send, receive) = mpsc::channel(4);
        let mut input = TerminalInput::scripted(receive);
        let (mut caller, response) = oneshot::channel::<Value>();
        let mut output = Output::default();
        let seen = output.clone();
        let (halt, halted) = oneshot::channel();
        let signal = async {
            let _ = halted.await;
            Ok(())
        };
        tokio::pin!(signal);
        let (result, primary) = {
            let running = login::run(
                &mut owner.control,
                target.clone(),
                selection,
                &mut caller,
                &mut input,
                &mut output,
                signal.as_mut(),
            );
            tokio::pin!(running);
            let mut completed = None;
            let primary = {
                let human = std::panic::AssertUnwindSafe(async {
                    callback(&seen).await;
                    wait_entered.await.unwrap();
                    send.send(Ok("/cancel".into())).await.unwrap();
                    // The sole input owner has selected the line before caller
                    // loss. A later stop cannot rewrite that retained winner.
                    while send.capacity() != 4 {
                        tokio::task::yield_now().await;
                    }
                    drop(response);
                    release.take().unwrap().send(()).unwrap();
                })
                .catch_unwind();
                tokio::pin!(human);
                tokio::time::timeout(Duration::from_secs(60), async {
                    tokio::select! {
                        result = running.as_mut() => { completed = Some(result); human.await },
                        result = human.as_mut() => result,
                    }
                })
                .await
            };
            // On observer panic or timeout, request stop and release the gate,
            // then await this same ceremony. A timeout never proves cleanup.
            let _ = halt.send(());
            if let Some(release) = release.take() {
                let _ = release.send(());
            }
            let result = match completed {
                Some(result) => result,
                None => running.await,
            };
            (result, primary)
        };
        let _ = finish.send(());
        let issuer_joined = server.await;
        match primary {
            Ok(Err(panic)) => std::panic::resume_unwind(panic),
            Err(error) => panic!("terminal cancellation observation timed out: {error}"),
            Ok(Ok(())) => {}
        }
        issuer_joined.unwrap();
        let result = result.unwrap();
        let facts = result.stop_reply().unwrap();
        let login::End::Stopped {
            reason: login::Stop::Cancelled,
            completion: Some(completion),
        } = result
        else {
            panic!("the selected terminal cancellation must survive later caller loss")
        };
        let current = owner
            .auth
            .facade
            .connector_status(&target.slot)
            .await
            .unwrap();
        match completion {
            Ok(completed) => {
                assert_eq!(facts["login_completed"], true);
                assert_eq!(current.phase, meerkat::ConnectorAuthPhase::Authorized);
                assert_eq!(
                    current.verified_account.unwrap(),
                    completed.native.verified_account
                );
                assert!(owner.control.completed().is_some());
            }
            Err(_) => {
                assert!(facts["login_completed"].is_null());
                assert!(owner.control.pending_slot().is_none());
            }
        }
        assert_eq!(tokens.load(Ordering::SeqCst), 1);
        assert_eq!(snapshot(&owner.storage), before);
        send.send(Ok("continue conversation".into())).await.unwrap();
        assert_eq!(
            input.recv().await.unwrap().unwrap(),
            "continue conversation"
        );
    }
}

#[test]
fn historical_profile_binding_requires_an_exact_current_match_and_returns_only_id() {
    let root = tempfile::tempdir().unwrap();
    let storage = crate::RealmOptions {
        context_root: root.path().into(),
        state_root: Some(root.path().join("state")),
        user_config_root: Some(root.path().join("home")),
    }
    .resolve("work")
    .unwrap();
    let config = definition(&storage, "https://issuer.invalid", "mail");
    let binding = config.connector.unwrap();
    let profile = meerkat::ConnectorClientAuthProfile {
        id: "mail-client".into(),
        issuer: binding.issuer.clone(),
        client: binding.client.clone(),
        client_secret: Some(meerkat::ConnectorClientSecret::Inline(
            "private-profile-canary".into(),
        )),
        authorize_params: Vec::new(),
        resource_indicator: true,
        protocol: None,
        registered_redirect_uris: Vec::new(),
    };
    let admitted = profile.binding();
    assert_eq!(
        matching_profile_id(&binding, &admitted, std::slice::from_ref(&profile)).unwrap(),
        "mail-client"
    );
    assert_ne!(admitted, "mail-client");
    assert!(matching_profile_id(&binding, &admitted, &[]).is_err());
    assert!(matching_profile_id(&binding, &admitted, &[profile.clone(), profile.clone()]).is_err());
    let mut changed = profile.clone();
    changed.resource_indicator = false;
    assert!(matching_profile_id(&binding, &admitted, &[changed]).is_err());
    assert!(
        matching_profile_id(&binding, "mail-client", &[profile]).is_err(),
        "an ID is not historical binding evidence"
    );
}

#[tokio::test]
async fn existing_definition_selection_refuses_foreign_ambiguous_or_changed_sources() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let root = tempfile::tempdir().unwrap();
        let owner = fixture(root.path(), "https://issuer.invalid").await;
        let selected = ExistingConnector::select(&owner.storage, "mail")
            .await
            .unwrap();
        let target = selected.target(["openid".into()].into(), None);
        selected.check_target(&target).await.unwrap();
        let mut substituted = target.clone();
        substituted.account =
            meerkat_auth_core::connector_oauth::AccountSelection::Known("someone-else".into());
        assert_eq!(
            selected.check_target(&substituted).await,
            Err("invalid_binding")
        );
        let (mut servers, generation) = crate::realm::read_own_mcp_config(&owner.storage)
            .await
            .unwrap();
        servers.push(servers[0].clone());
        assert!(matches!(
            ExistingConnector::from_observation(&owner.storage, &servers, generation, "mail"),
            Err("owned_connector_unavailable")
        ));
        servers.pop();
        servers[0].connector.as_mut().unwrap().realm_id = "elsewhere".into();
        assert!(matches!(
            ExistingConnector::from_observation(&owner.storage, &servers, generation, "mail"),
            Err("credential_owned_by_another_realm")
        ));
        let own = crate::realm::own_config_runtime(&owner.storage).unwrap();
        meerkat_core::McpRealmPersistTarget::new(&own)
            .add(
                definition(&owner.storage, "https://issuer.invalid", "calendar"),
                Some(generation),
            )
            .await
            .unwrap();
        assert_eq!(
            selected.check_target(&target).await,
            Err("reviewed_definition_changed")
        );
    })
    .await
    .expect("source refusal controls settle");
}

#[tokio::test]
async fn shared_terminal_ceremony_preserves_initial_and_existing_login_custody() {
    for kind in ["initial", "existing"] {
        for action in [
            "pre_cancel",
            "callback_cancel",
            "caller_gone",
            "complete",
            "output_error",
            "callback_refused",
            "callback_expired",
        ] {
            let root = tempfile::tempdir().unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let mut owner = fixture(root.path(), &origin).await;
            let selected = ExistingConnector::select(&owner.storage, "mail")
                .await
                .unwrap();
            let mut target = selected.target(["openid".into()].into(), None);
            let installation =
                connector_unprimed_fixture(owner.storage.locator.clone(), SessionId::new(), false);
            installation.prime(true).await;
            let selection = if kind == "initial" {
                target.account = meerkat_auth_core::connector_oauth::AccountSelection::Discover;
                LoginSelection::Installed(Arc::new(
                    installation.selection(connector_operations()).unwrap(),
                ))
            } else {
                LoginSelection::Existing(Arc::new(selected.clone()))
            };
            let before = snapshot(&owner.storage);
            assert!(before[2].is_some());
            let tokens = Arc::new(AtomicUsize::new(0));
            let (finish, stopped) = oneshot::channel();
            let server = tokio::spawn(issuer(listener, origin, tokens.clone(), stopped));
            let (send, receive) = mpsc::channel(4);
            let mut input = TerminalInput::scripted(receive);
            let (mut caller, response) = oneshot::channel::<Value>();
            let mut response = Some(response);
            let mut output = Output {
                fail_url: action == "output_error",
                ..Default::default()
            };
            let seen = output.clone();
            if action == "pre_cancel" {
                send.send(Ok("/cancel".into())).await.unwrap();
            }
            let (halt, halted) = oneshot::channel();
            let signal = async {
                let _ = halted.await;
                Ok(())
            };
            tokio::pin!(signal);
            let (result, primary) = {
                let running = async {
                    if action == "callback_expired" {
                        login::run_with_window(
                            &mut owner.control,
                            target.clone(),
                            selection,
                            &mut caller,
                            &mut input,
                            &mut output,
                            signal.as_mut(),
                            Duration::ZERO,
                        )
                        .await
                    } else {
                        login::run(
                            &mut owner.control,
                            target.clone(),
                            selection,
                            &mut caller,
                            &mut input,
                            &mut output,
                            signal.as_mut(),
                        )
                        .await
                    }
                };
                tokio::pin!(running);
                let mut completed = None;
                let primary = {
                    let human = std::panic::AssertUnwindSafe(async {
                        match action {
                            "callback_cancel" | "caller_gone" => {
                                shown(&seen, "Waiting for native sign-in.").await;
                                if action == "caller_gone" {
                                    drop(response.take());
                                } else {
                                    send.send(Ok("/cancel".into())).await.unwrap();
                                }
                            }
                            "complete" => callback(&seen).await,
                            "callback_refused" => callback_outcome(&seen, true).await,
                            _ => {}
                        }
                    })
                    .catch_unwind();
                    tokio::pin!(human);
                    tokio::time::timeout(Duration::from_secs(60), async {
                        tokio::select! {
                            result = running.as_mut() => { completed = Some(result); human.await },
                            observed = human.as_mut() => {
                                observed?;
                                // Ordinary expiry and output-error finish on
                                // their own; observer failure requests cleanup.
                                completed = Some(running.as_mut().await);
                                Ok(())
                            },
                        }
                    })
                    .await
                };
                if !matches!(&primary, Ok(Ok(()))) {
                    let _ = halt.send(());
                }
                let result = match completed {
                    Some(result) => result,
                    None => running.await,
                };
                (result, primary)
            };
            let _ = finish.send(());
            let issuer_joined = server.await;
            match primary {
                Ok(Err(panic)) => std::panic::resume_unwind(panic),
                Err(error) => panic!("shared ceremony observation timed out: {error}"),
                Ok(Ok(())) => {}
            }
            issuer_joined.unwrap();
            match action {
                "complete" => {
                    let login::End::Completed(completed) = result.unwrap() else {
                        panic!("completion expected")
                    };
                    assert_eq!(
                        completed.native.verified_account.subject,
                        "verified-subject"
                    );
                    assert_eq!(completed.prepared_installation().is_ok(), kind == "initial");
                    owner.control.retire().unwrap();
                    assert!(owner.control.completed().is_some());
                    assert_eq!(tokens.load(Ordering::SeqCst), 1);
                    let defaults = native_defaults(&owner, &selected).await.unwrap();
                    assert_eq!(defaults.scopes, ["openid".into()].into());
                    assert_eq!(defaults.profile_id, None);
                    let notice = defaults_notice(&Ok(defaults)).to_string();
                    assert!(!notice.contains("private-native-token-canary"));
                }
                "output_error" => {
                    assert!(result.is_err());
                    assert!(owner.control.pending_slot().is_none());
                }
                "callback_refused" | "callback_expired" => {
                    assert!(matches!(
                        result.unwrap(),
                        login::End::Refused("callback_unavailable")
                    ));
                    assert!(owner.control.pending_slot().is_none());
                    assert!(owner.control.completed().is_none());
                    assert_eq!(tokens.load(Ordering::SeqCst), 0);
                    assert_eq!(
                        owner
                            .auth
                            .facade
                            .connector_status(&target.slot)
                            .await
                            .unwrap()
                            .phase,
                        meerkat::ConnectorAuthPhase::AuthorizationRequired
                    );
                    send.send(Ok("continue after sign-in refusal".into()))
                        .await
                        .unwrap();
                    assert_eq!(
                        input.recv().await.unwrap().unwrap(),
                        "continue after sign-in refusal"
                    );
                }
                _ => {
                    let result = result.unwrap();
                    assert_eq!(result.stop_reply().unwrap()["login_completed"], false);
                    assert!(owner.control.pending_slot().is_none());
                    assert_eq!(tokens.load(Ordering::SeqCst), 0);
                    send.send(Ok("next request".into())).await.unwrap();
                    assert_eq!(input.recv().await.unwrap().unwrap(), "next request");
                }
            }
            assert_eq!(
                snapshot(&owner.storage),
                before,
                "login cannot install or issue/revoke grants"
            );
            assert!(!seen.text().contains("private-native-token-canary"));
        }
    }
}

#[tokio::test]
async fn reviewed_disconnect_is_slot_wide_and_keeps_configuration_and_grants() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut owner = fixture(root.path(), &origin).await;
        let (_, generation) = crate::realm::read_own_mcp_config(&owner.storage)
            .await
            .unwrap();
        let own = crate::realm::own_config_runtime(&owner.storage).unwrap();
        meerkat_core::McpRealmPersistTarget::new(&own)
            .add(
                definition(&owner.storage, &origin, "calendar"),
                Some(generation),
            )
            .await
            .unwrap();
        let selected = ExistingConnector::select(&owner.storage, "mail")
            .await
            .unwrap();
        let target = selected.target(["openid".into()].into(), None);
        let tokens = Arc::new(AtomicUsize::new(0));
        let (finish, stopped) = oneshot::channel();
        let server = tokio::spawn(issuer(listener, origin, tokens, stopped));
        let (send, receive) = mpsc::channel(4);
        let mut input = TerminalInput::scripted(receive);
        let (mut caller, _response) = oneshot::channel();
        let mut output = Output::default();
        let seen = output.clone();
        let signal = std::future::pending();
        tokio::pin!(signal);
        let (result, ()) = tokio::join!(
            login::run(
                &mut owner.control,
                target.clone(),
                LoginSelection::Existing(Arc::new(selected.clone())),
                &mut caller,
                &mut input,
                &mut output,
                signal.as_mut()
            ),
            callback(&seen)
        );
        assert!(matches!(result.unwrap(), login::End::Completed(_)));
        let (approvals, mut presenter) = LocalApprovals::create_paired_host(root.path()).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let before = snapshot(&owner.storage);
        let (response, receive) = oneshot::channel();
        let seen = output.clone();
        let human = async {
            shown(&seen, "Type 'approve ").await;
            let text = seen.text();
            let id = text
                .split("Type 'approve ")
                .last()
                .unwrap()
                .split('\'')
                .next()
                .unwrap();
            assert!(text.contains("slot-wide"));
            assert!(text.contains("calendar"));
            send.send(Ok(format!("approve {id}"))).await.unwrap();
        };
        let (result, ()) = tokio::join!(
            manage(
                &mut owner,
                &session,
                Change::Disconnect {
                    service: "mail".into()
                },
                response,
                &approvals,
                &mut presenter,
                &mut input,
                &mut output,
                signal.as_mut()
            ),
            human
        );
        assert!(result.unwrap());
        let reply = receive.await.unwrap();
        assert_eq!(reply["state"], "logout_completed");
        assert_eq!(reply["account_status"]["phase"], "authorization_required");
        let calendar = ExistingConnector::select(&owner.storage, "calendar")
            .await
            .unwrap();
        assert_eq!(calendar.slot(), selected.slot());
        assert_eq!(
            owner
                .auth
                .facade
                .connector_status(calendar.slot())
                .await
                .unwrap()
                .phase,
            meerkat::ConnectorAuthPhase::AuthorizationRequired
        );
        assert_eq!(snapshot(&owner.storage), before);
        finish.send(()).unwrap();
        server.await.unwrap();
    })
    .await
    .expect("reviewed slot-wide logout settles");
}

#[tokio::test]
async fn reconnect_recovers_native_inputs_and_refuses_definition_change_after_review() {
    tokio::time::timeout(Duration::from_secs(60), async {
        for changed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let mut owner = fixture(root.path(), &origin).await;
            let selected = ExistingConnector::select(&owner.storage, "mail")
                .await
                .unwrap();
            let target = selected.target(["openid".into()].into(), None);
            assert!(matches!(
                native_defaults(&owner, &selected).await,
                Err("reconnect_metadata_missing")
            ));
            let tokens = Arc::new(AtomicUsize::new(0));
            let (finish, stopped) = oneshot::channel();
            let server = tokio::spawn(issuer(listener, origin.clone(), tokens.clone(), stopped));
            let (send, receive) = mpsc::channel(4);
            let mut input = TerminalInput::scripted(receive);
            let (mut caller, _response) = oneshot::channel();
            let mut output = Output::default();
            let seen = output.clone();
            let signal = std::future::pending();
            tokio::pin!(signal);
            let (result, ()) = tokio::join!(
                login::run(
                    &mut owner.control,
                    target,
                    LoginSelection::Existing(Arc::new(selected)),
                    &mut caller,
                    &mut input,
                    &mut output,
                    signal.as_mut()
                ),
                callback(&seen)
            );
            assert!(matches!(result.unwrap(), login::End::Completed(_)));
            let before = snapshot(&owner.storage);
            output.bytes.lock().unwrap().clear();
            let seen = output.clone();
            let storage = owner.storage.clone();
            let (approvals, mut presenter) =
                LocalApprovals::create_paired_host(root.path()).unwrap();
            let session = SessionId::new();
            approvals.bind_session(&session).unwrap();
            let (response, receive) = oneshot::channel();
            let human = async {
                shown(&seen, "Type 'approve ").await;
                let text = seen.text();
                assert!(text.contains("native_credential_metadata_and_exact_current_profile"));
                assert!(text.contains("openid"));
                assert!(text.contains("verified-subject"));
                assert!(!text.contains("private-native-token-canary"));
                if changed {
                    let (_, generation) =
                        crate::realm::read_own_mcp_config(&storage).await.unwrap();
                    let runtime = crate::realm::own_config_runtime(&storage).unwrap();
                    meerkat_core::McpRealmPersistTarget::new(&runtime)
                        .add(
                            definition(&storage, &origin, "new-service"),
                            Some(generation),
                        )
                        .await
                        .unwrap();
                }
                let id = text
                    .split("Type 'approve ")
                    .last()
                    .unwrap()
                    .split('\'')
                    .next()
                    .unwrap();
                send.send(Ok(format!("approve {id}"))).await.unwrap();
                if !changed {
                    callback(&seen).await;
                }
            };
            let (result, ()) = tokio::join!(
                manage(
                    &mut owner,
                    &session,
                    Change::Reconnect {
                        service: "mail".into(),
                        scopes: None,
                        client_auth_profile: None
                    },
                    response,
                    &approvals,
                    &mut presenter,
                    &mut input,
                    &mut output,
                    signal.as_mut()
                ),
                human
            );
            assert!(result.unwrap());
            let reply = receive.await.unwrap();
            if changed {
                assert_eq!(reply["reason"], "reviewed_definition_changed");
                assert_eq!(tokens.load(Ordering::SeqCst), 1);
                assert_eq!(snapshot(&owner.storage)[2], before[2]);
            } else {
                assert_eq!(reply["state"], "login_completed");
                assert_eq!(tokens.load(Ordering::SeqCst), 2);
                assert_eq!(snapshot(&owner.storage), before);
            }
            finish.send(()).unwrap();
            server.await.unwrap();
        }
    })
    .await
    .expect("metadata defaults and review rechecks settle");
}
