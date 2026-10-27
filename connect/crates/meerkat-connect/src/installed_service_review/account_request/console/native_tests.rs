//! Uses the existing connector fixture's real facade, Control, native session
//! and blocked token exchange. It is not a verified Console viewer or B test.
use super::*;
use crate::installed_service_review::{
    InstalledServiceReview, account_request::console::{complete_joined, fixture_join_on_close},
};

#[tokio::test]
async fn console_account_close_joins_the_entered_native_completion() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let origin_server = origin.clone();
        let (entered, wait_entered) = oneshot::channel();
        let (release, wait_release) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut entered = Some(entered);
            let mut wait_release = Some(wait_release);
            for expected in [
                "GET /.well-known/oauth-authorization-server ",
                "GET /.well-known/oauth-authorization-server ",
                "POST /token ",
                "GET /.well-known/openid-configuration ",
                "GET /userinfo ",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (header, body) = connector_http(&mut socket).await;
                assert!(header.starts_with(expected));
                let response = if expected == "POST /token " {
                    let body = String::from_utf8(body).unwrap();
                    assert!(body.contains("code=private-code-canary"));
                    assert!(body.contains("client_secret=private-profile-canary"));
                    entered.take().unwrap().send(()).unwrap();
                    wait_release.take().unwrap().await.unwrap();
                    json!({"access_token":"private-token-canary","token_type":"Bearer",
                        "expires_in":3600,"scope":"openid"})
                } else if expected == "GET /userinfo " {
                    assert!(
                        header
                            .to_ascii_lowercase()
                            .contains("authorization: bearer private-token-canary")
                    );
                    json!({"sub":"verified-subject"})
                } else {
                    connector_metadata(&origin_server)
                };
                connector_http_reply(&mut socket, response).await;
            }
        });
        let (startup, auth, _unused_control) = connector_fixture(root.path(), &origin).await;
        let realm = startup.storage.locator.clone();
        let session = connector_native_session(startup).await;
        let installed =
            connector_installed_fixture(realm.clone(), session.id().clone(), false, true).await;
        let (approvals, _presenter) = LocalApprovals::create_browser(
            root.path(),
            ApprovalPrincipalId::new("fixture-console-presenter").unwrap(),
            std::time::SystemTime::now() + Duration::from_secs(120),
        )
        .unwrap();
        approvals.bind_session(session.id()).unwrap();
        let mut review = InstalledServiceReview::fixture_checked(
            realm,
            session.id().clone(),
            installed.owner.clone(),
            auth.clone(),
            Arc::new(approvals),
        )
        .unwrap();
        let params: ConnectorStart =
            serde_json::from_value(connector_start("fixture-scope", &origin)).unwrap();
        let target = params.connector.into_native(auth.realm.clone()).unwrap();
        let slot = target.slot.clone();
        let operations = serde_json::from_value(connector_operations()).unwrap();
        let started = review
            .start_account(&"3".repeat(32), target, operations, params.redirect_uri)
            .await
            .unwrap();
        let (result, stopped) = {
            let (close, wait_close) = oneshot::channel();
            let completing = complete_joined(
                &mut review,
                started.state,
                "private-code-canary".into(),
                started.host_attempt_handle,
                async {
                    let _ = wait_close.await;
                },
            );
            tokio::pin!(completing);
            tokio::select! { biased;
                entered = wait_entered => entered.unwrap(),
                _ = completing.as_mut() => panic!("native token exchange must remain entered"),
            }
            close.send(()).unwrap();
            assert!(
                completing.as_mut().now_or_never().is_none(),
                "closing the Console must join the actual held token exchange"
            );
            release.send(()).unwrap();
            completing.await.unwrap()
        };
        assert!(stopped);
        let result = result.expect("entered completion returns its joined result");
        assert!(
            result.result.is_err(),
            "cancelled native completion must not become approval"
        );
        assert!(matches!(result.cancellation, Some(Ok(()))));
        assert!(!result.cleanup_unconfirmed);
        server.await.unwrap();
        let status = auth.facade.connector_status(&slot).await.unwrap();
        assert!(matches!(
            status.phase,
            meerkat::ConnectorAuthPhase::AuthorizationRequired
        ));
        drop(review);
        Arc::try_unwrap(session)
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
    })
    .await
    .expect("Console account close did not join native work");
}

#[test]
fn console_ready_close_does_not_start_the_native_account_ceremony() {
    // Native-owner measurement found a 2,094,344-byte debug setup stack.
    // Keep this fixture's scheduler current-thread and
    // construct its entire future on the explicitly sized, joined test thread.
    let worker = std::thread::Builder::new()
        .name("console-ready-close-fixture".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("ready-close Tokio runtime")
                .block_on(async {
                tokio::time::timeout(Duration::from_secs(10), async {
                    let root = tempfile::tempdir().unwrap();
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let origin = format!("http://{}", listener.local_addr().unwrap());
                    let (startup, auth, _unused_control) = connector_fixture(root.path(), &origin).await;
                    let realm = startup.storage.locator.clone();
                    let session = connector_native_session(startup).await;
                    let installed =
                        connector_installed_fixture(realm.clone(), session.id().clone(), false, true).await;
                    let (approvals, _presenter) = LocalApprovals::create_browser(
                        root.path(),
                        ApprovalPrincipalId::new("fixture-console-presenter").unwrap(),
                        std::time::SystemTime::now() + Duration::from_secs(120),
                    ).unwrap();
                    approvals.bind_session(session.id()).unwrap();
                    let mut review = InstalledServiceReview::fixture_checked(
                        realm, session.id().clone(), installed.owner.clone(), auth.clone(), Arc::new(approvals),
                    ).unwrap();
                    let params: ConnectorStart =
                        serde_json::from_value(connector_start("fixture-scope", &origin)).unwrap();
                    let target = params.connector.into_native(auth.realm.clone()).unwrap();
                    let (bridge, _prompts, _retire) = PromptBridge::channel();
                    let closed = std::future::ready(());
                    tokio::pin!(closed);
                    let (result, stopped) = fixture_join_on_close(
                        review.start_account(
                            &"3".repeat(32), target, serde_json::from_value(connector_operations()).unwrap(),
                            params.redirect_uri,
                        ),
                        &bridge, closed.as_mut(),
                    ).await;
                    assert!(stopped);
                    assert!(result.is_none(), "ready close must not poll native start");
                    assert!(!review.fixture_account_started());
                    assert!(listener.accept().now_or_never().is_none(),
                        "ready close must not begin native authorization-server discovery");
                    review.retire().unwrap();
                    drop(review);
                    Arc::try_unwrap(session).ok().unwrap().close().await.unwrap();
                }).await.expect("ready close started or failed to release native account work");
                });
        })
        .expect("ready-close fixture thread");
    if let Err(panic) = worker.join() {
        std::panic::resume_unwind(panic);
    }
}


#[derive(Clone, Copy)]
enum SignInCancelPoint { BeforeCompletion, EnteredCompletion, AfterCompletion }

#[tokio::test]
async fn provider_sign_in_cancel_before_completion_entry_retires_only_the_started_attempt() {
    provider_sign_in_cancel_fixture(SignInCancelPoint::BeforeCompletion).await;
}
#[tokio::test]
async fn provider_sign_in_cancel_joins_an_entered_completion_before_prompt_closure() {
    provider_sign_in_cancel_fixture(SignInCancelPoint::EnteredCompletion).await;
}
#[tokio::test]
async fn provider_sign_in_cancel_preserves_the_joined_completion_and_account() {
    provider_sign_in_cancel_fixture(SignInCancelPoint::AfterCompletion).await;
}

// These source controls use the existing fixture's facade, Control and native
// session. Callback transport consumption is covered in console/tests.rs. They
// do not claim a verified Console login, full original-task continuation or live
// provider authentication. No entered owner is scoped inside an observer timeout.
async fn provider_sign_in_cancel_fixture(point: SignInCancelPoint) {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let origin_server = origin.clone();
    let (entered, mut wait_entered) = oneshot::channel();
    let (release, wait_release) = oneshot::channel();
    let (stop_issuer, mut issuer_closed) = oneshot::channel();
    let calls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let server_calls = calls.clone();
    let server = tokio::spawn(async move {
        let mut entered = Some(entered);
        let mut wait_release = Some(wait_release);
        loop {
            let incoming = tokio::select! { biased;
                _ = &mut issuer_closed => break,
                incoming = listener.accept() => incoming,
            };
            let (mut socket, _) = incoming.unwrap();
            let (header, _body) = connector_http(&mut socket).await;
            server_calls.lock().unwrap().push(header.lines().next().unwrap_or_default().to_owned());
            let response = if header.starts_with("POST /token ") {
                if let Some(entered) = entered.take() { let _ = entered.send(()); }
                if let Some(release) = wait_release.take() { let _ = release.await; }
                json!({"access_token":"private-token-canary","token_type":"Bearer",
                    "expires_in":3600,"scope":"openid"})
            } else if header.starts_with("GET /userinfo ") {
                json!({"sub":"verified-subject"})
            } else { connector_metadata(&origin_server) };
            connector_http_reply(&mut socket, response).await;
        }
    });
    let (startup, auth, _unused_control) = connector_fixture(root.path(), &origin).await;
    let realm = startup.storage.locator.clone();
    let session = connector_native_session(startup).await;
    let installed = connector_installed_fixture(realm.clone(), session.id().clone(), false, true).await;
    let (approvals, _presenter) = LocalApprovals::create_browser(root.path(),
        ApprovalPrincipalId::new("fixture-console-presenter").unwrap(),
        std::time::SystemTime::now() + Duration::from_secs(120)).unwrap();
    approvals.bind_session(session.id()).unwrap();
    let mut review = InstalledServiceReview::fixture_checked(realm, session.id().clone(),
        installed.owner.clone(), auth.clone(), Arc::new(approvals)).unwrap();
    let params: ConnectorStart = serde_json::from_value(connector_start("fixture-scope", &origin)).unwrap();
    let target = params.connector.into_native(auth.realm.clone()).unwrap();
    let slot = target.slot.clone();
    let started = review.start_account(&"3".repeat(32), target,
        serde_json::from_value(connector_operations()).unwrap(), params.redirect_uri).await.unwrap();
    let (bridge, mut prompts, _retire) = PromptBridge::channel();
    let mut notice = bridge.provider_notice(json!({"kind":"provider_browser",
        "authorization_url":started.authorize_url})).await.unwrap();
    let mut exchange = Some(prompts.recv().await.unwrap());
    let owner_close = exchange.as_mut().unwrap().take_owner_close().unwrap();
    let mut callback = None;
    let mut cancellation = None;
    if matches!(point, SignInCancelPoint::BeforeCompletion) {
        cancellation = Some(crate::installed_service_review::account_request::console::cancel_retained_sign_in_authorized(
            31, &mut exchange, &mut callback, br#"{"review_id":31}"#, || Ok(())));
    }
    let mut entered_observed = None;
    let mut stayed_pending = None;
    let mut release = Some(release);
    let joined = {
        let completing = complete_joined(&mut review, started.state, "private-code-canary".into(),
            started.host_attempt_handle, notice.cancelled());
        tokio::pin!(completing);
        let mut early = None;
        if matches!(point, SignInCancelPoint::EnteredCompletion) {
            // Only the observer borrows the completion. A timeout cannot drop
            // it, and all diagnostics are checked after release and full join.
            entered_observed = Some(tokio::time::timeout(Duration::from_secs(10), async {
                tokio::select! { biased;
                    entered = &mut wait_entered => entered.is_ok(),
                    result = completing.as_mut() => { early = Some(result); false },
                }
            }).await);
            cancellation = Some(crate::installed_service_review::account_request::console::cancel_retained_sign_in_authorized(
                31, &mut exchange, &mut callback, br#"{"review_id":31}"#, || Ok(())));
            if early.is_none() {
                let observed = completing.as_mut().now_or_never();
                stayed_pending = Some(observed.is_none());
                early = observed;
            }
        }
        // Release the exact issuer gate regardless of observation outcome.
        let _ = release.take().unwrap().send(());
        match early { Some(result) => result, None => completing.await }
    };
    if matches!(point, SignInCancelPoint::AfterCompletion) {
        cancellation = Some(crate::installed_service_review::account_request::console::cancel_retained_sign_in_authorized(
            31, &mut exchange, &mut callback, br#"{"review_id":31}"#, || Ok(())));
    }
    let login_completed = joined.as_ref().ok().and_then(|(completion,_)| completion.as_ref())
        .is_some_and(|completion| completion.result.is_ok());
    // Fixture teardown reaches the same Control retirement: Started is
    // cancelled and Completed is kept. This also retires fixture approvals;
    // it does not establish continued eligibility of a real Console host.
    let retired = review.retire();
    let native_status = auth.facade.connector_status(&slot).await;
    let finish = notice.finish_with_cancellation(if login_completed {
        SignInDisposition::Completed
    } else { SignInDisposition::Refused });
    tokio::pin!(finish);
    let early_finish = finish.as_mut().now_or_never();
    let closing = owner_close.await;
    let disposition = closing.as_ref().ok().map(|(value,_)| matches!(value, SignInDisposition::Completed));
    if let Ok((_,ack)) = closing { let _ = ack.send(()); }
    let finished = match early_finish { Some(result) => result, None => finish.await };
    let _ = stop_issuer.send(());
    let issuer_joined = server.await;
    let requests = calls.lock().unwrap().clone();
    drop(review);
    let session_closed = Arc::try_unwrap(session).ok().unwrap().close().await;
    // All entered native work, the exact prompt, issuer and session are joined.
    assert!(cancellation.unwrap().is_ok());
    assert!(retired.is_ok());
    assert!(issuer_joined.is_ok());
    assert!(session_closed.is_ok());
    assert!(finished.unwrap());
    let (completion, stopped) = joined.unwrap();
    match point {
        SignInCancelPoint::BeforeCompletion => {
            assert!(stopped && completion.is_none());
            assert!(!requests.iter().any(|request| request.starts_with("POST /token ")));
            assert_eq!(native_status.unwrap().phase, meerkat::ConnectorAuthPhase::AuthorizationRequired);
            assert_eq!(disposition, Some(false));
        }
        SignInCancelPoint::EnteredCompletion => {
            assert!(matches!(entered_observed, Some(Ok(true))));
            assert_eq!(stayed_pending, Some(true));
            assert!(stopped);
            let completed = completion.unwrap();
            assert!(completed.result.is_err() && !completed.cleanup_unconfirmed);
            assert!(matches!(completed.cancellation, Some(Ok(()))));
            assert_eq!(native_status.unwrap().phase, meerkat::ConnectorAuthPhase::AuthorizationRequired);
            assert_eq!(disposition, Some(false));
        }
        SignInCancelPoint::AfterCompletion => {
            assert!(!stopped);
            let completed = completion.unwrap();
            assert!(completed.result.is_ok() && !completed.cleanup_unconfirmed);
            assert_eq!(native_status.unwrap().phase, meerkat::ConnectorAuthPhase::Authorized);
            assert_eq!(disposition, Some(true));
        }
    }
}
