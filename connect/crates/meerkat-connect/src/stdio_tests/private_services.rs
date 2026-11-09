//! Actual private Driver prompt/cancellation controls; no external provider I/O.
use super::*;

fn selected(version: u8) -> Initialize {
    serde_json::from_value(json!({"protocol_version":version,"adapter":"services",
        "application":{"application_id":"console","instance_id":"one","origin":"https://console.example"},
        "viewer":{"principal_id":"viewer","login_scope_id":"original-login","expires_at_ms":now()+60_000},
        "realm":{"realm_id":"private-services","state_root":"/fixture/state","context_root":"/fixture/context","user_config_root":"/fixture/home"},
        "services":{"resource_manifest_sha256":"0".repeat(64),"preparation":"permitted"}})).unwrap()
}
fn driver(version: u8) -> (Driver, mpsc::Receiver<OutputFrame>) {
    let (tx, rx) = mpsc::channel(OUTPUT_FRAMES);
    let (mut driver, _, _) = new_driver(tx, Arc::new(Semaphore::new(MAX_FRAME * 2)));
    driver.binding = Some(selected(version));
    driver.initialize_id = Some("h:init".into());
    driver.scope = Some("selected-scope".into());
    driver.active = Some(ActiveSend {
        id: "h:send".into(),
        cancel: watch::channel(false).0,
        receipt: true,
        cancel_result: None,
        cancel_requests: vec![],
        provisional_closed: false,
    });
    (driver, rx)
}
fn prompt() -> Value {
    json!({"kind":"service_secret","job_id":"job-a","service":"github-enterprise","slot":"GH_TOKEN","label":"PAT"})
}
fn answer() -> Value {
    json!({"jsonrpc":"2.0","id":"p:1","result":{"answer":{
    "kind":"service_secret","job_id":"job-a","service":"github-enterprise","slot":"GH_TOKEN","value":"PRIVATE_SENTINEL"}}})
}
fn rows(rx: &mut mpsc::Receiver<OutputFrame>) -> Vec<Value> {
    let mut rows = vec![];
    while let Ok(frame) = rx.try_recv() {
        rows.push(serde_json::from_slice(&frame.bytes).unwrap());
    }
    rows
}

#[test]
fn v5_opt_in_keeps_old_prompt_vocabulary_and_rejects_transient_profile_configuration() {
    let (_, presenter) = LazySetup::private_services();
    for version in [3, 5] {
        let selected = selected(version);
        assert!(selected.valid());
        let offered =
            capabilities(version, Adapter::Services, false, &[], Some(&presenter)).unwrap();
        assert_eq!(
            offered["prompt_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "service_secret"),
            version == 5
        );
    }
    let mut v5 = json!({"protocol_version":5,"adapter":"services",
        "application":{"application_id":"console","instance_id":"one","origin":"https://console.example"},
        "viewer":{"principal_id":"viewer","login_scope_id":"original-login","expires_at_ms":now()+60_000},
        "realm":{"realm_id":"private-services","state_root":"/fixture/state","context_root":"/fixture/context","user_config_root":"/fixture/home"},
        "services":{"resource_manifest_sha256":"0".repeat(64),"preparation":"permitted"},"connector_auth":{"profiles":[]}});
    assert!(serde_json::from_value::<Initialize>(v5.clone()).is_err());
    v5["protocol_version"] = json!(3);
    assert!(serde_json::from_value::<Initialize>(v5).is_ok());
    let (mut old, mut output) = driver(3);
    let (send, mut receive) = oneshot::channel();
    old.prompt(Exchange {
        commissioning: false,
        prompt: prompt(),
        response: send,
        owner_close: None,
    });
    assert!(old.retired && receive.try_recv().is_err());
    assert!(
        !rows(&mut output)
            .iter()
            .any(|r| r["method"] == "connect/prompt")
    );
}

#[tokio::test]
async fn secret_answers_bind_exact_prompt_job_service_slot_and_never_echo_value() {
    let (mut driver, mut output) = driver(5);
    let (send, mut receive) = oneshot::channel();
    driver.prompt(Exchange {
        commissioning: false,
        prompt: prompt(),
        response: send,
        owner_close: None,
    });
    for pointer in [
        "/id",
        "/result/answer/job_id",
        "/result/answer/service",
        "/result/answer/slot",
    ] {
        let mut wrong = answer();
        *wrong.pointer_mut(pointer).unwrap() = json!("other");
        driver.frame(&serde_json::to_vec(&wrong).unwrap());
        assert!(driver.pending.is_some());
        assert!(receive.try_recv().is_err());
    }
    let mut malformed = answer();
    malformed["result"]["answer"]["unexpected"] = json!("PRIVATE_SENTINEL");
    driver.frame(&serde_json::to_vec(&malformed).unwrap());
    assert!(driver.pending.is_some());
    driver.frame(&serde_json::to_vec(&answer()).unwrap());
    assert!(
        matches!(receive.await.unwrap().unwrap(),Answer::ServiceSecret{value,..} if value=="PRIVATE_SENTINEL")
    );
    assert!(driver.pending.is_none());
    let retained = serde_json::to_string(&rows(&mut output)).unwrap();
    assert!(!retained.contains("PRIVATE_SENTINEL"));
    driver.frame(&serde_json::to_vec(&answer()).unwrap());
    assert!(
        !serde_json::to_string(&rows(&mut output))
            .unwrap()
            .contains("PRIVATE_SENTINEL")
    );
}

#[tokio::test]
async fn v5_cancellation_and_expiry_cannot_reopen_original_administration() {
    for expiry in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let session = super::control_session(root.path()).await;
        let (mut driver, mut output) = driver(5);
        driver.session = Some(session.clone());
        let admission = crate::terminal_connector::private_service::Admission::new(
            driver.binding.as_ref().unwrap().viewer.expires_at_ms,
            driver.plain_cancel.subscribe(),
        );
        if expiry {
            driver.binding.as_mut().unwrap().viewer.expires_at_ms = 0;
        }
        driver.frame(br#"{"jsonrpc":"2.0","id":"h:cancel","method":"connect/cancel","params":{"scope":"selected-scope","send_request_id":"h:send"}}"#);
        assert!(driver.retired);
        assert!(meerkat_core::AdministrationEntry::enter(&admission).is_err());
        driver.active = None;
        driver.frame(br#"{"jsonrpc":"2.0","id":"h:next","method":"connect/send","params":{"scope":"selected-scope","text":"new request"}}"#);
        assert!(driver.active.is_none());
        assert!(*driver.plain_cancel.borrow());
        assert!(!rows(&mut output).is_empty());
        driver.session.take();
        Arc::try_unwrap(session)
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn v5_actual_closed_setup_remains_owned_through_presenter_join_and_expiry() {
    let root = tempfile::tempdir().unwrap();
    let session = Arc::try_unwrap(super::control_session(root.path()).await)
        .ok()
        .unwrap();
    let closed = match session.try_close_for_finalization().await {
        crate::session::FinalizationClose::Closed(closed) => closed,
        _ => panic!("idle native setup must provide actual Closed"),
    };
    let (mut driver, mut output) = driver(5);
    driver.active = None;
    let admission = crate::terminal_connector::private_service::Admission::new(
        driver.binding.as_ref().unwrap().viewer.expires_at_ms,
        driver.plain_cancel.subscribe(),
    );
    driver.plain_pending = true;
    driver.plain_close_started = true;
    driver.plain_closed = Some(closed);
    driver.workers = 1;
    driver.fence("finalizing");
    assert!(meerkat_core::AdministrationEntry::enter(&admission).is_ok());
    assert!(driver.watches_expiry());
    driver.fence("expired");
    assert!(meerkat_core::AdministrationEntry::enter(&admission).is_err());
    assert!(driver.settle_plain() && driver.tasks.is_empty());
    assert!(
        !rows(&mut output)
            .iter()
            .any(|row| row["result"]["entered_work"] == "settled")
    );
    driver.workers = 0;
    driver
        .plain_closed
        .take()
        .unwrap()
        .finish_host_cleanup()
        .await
        .map_err(|(_, error)| error)
        .unwrap();
    assert!(driver.plain_outcomes.is_empty());
}

/// The package adapter below is the existing synthetic checked-install fixture.
/// Native realm profiles, login/token/account observation and LocalApprovals are
/// real owners; HTTP is a bounded loopback fixture, not external OAuth acceptance.
#[tokio::test]
async fn v5_account_presenter_reuses_native_realm_profile_login_and_exact_access_handoff() {
    use meerkat_auth_core::auth_store::{
        FileLockCoordinator, FileTokenStore, ProviderAuthPersistence,
    };
    use meerkat_core::AgentToolDispatcher;
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server_origin = origin.clone();
    let (stop_server, mut stopped) = watch::channel(false);
    let server_future = async move {
        let mut calls = 0;
        for expected in [
            "GET /.well-known/oauth-authorization-server ",
            "GET /.well-known/oauth-authorization-server ",
            "POST /token ",
            "GET /.well-known/openid-configuration ",
            "GET /userinfo ",
        ] {
            let request = async {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (header, _) = super::connector_http(&mut socket).await;
                assert!(header.starts_with(expected));
                let body = if expected == "POST /token " {
                    json!({"access_token":"PRIVATE_NATIVE_TOKEN","token_type":"Bearer","expires_in":3600,"scope":"openid"})
                } else if expected == "GET /userinfo " {
                    json!({"sub":"verified-private-account"})
                } else {
                    super::connector_metadata(&server_origin)
                };
                super::connector_http_reply(&mut socket, body).await;
            };
            tokio::select! { biased;
                _=stopped.changed()=>return calls,
                result=tokio::time::timeout(Duration::from_secs(10),request)=>result.unwrap(),
            }
            calls += 1;
        }
        calls
    };
    let storage = RealmOptions {
        context_root: root.path().to_owned(),
        state_root: Some(root.path().join("state")),
        user_config_root: Some(root.path().join("home")),
    }
    .resolve("private-account")
    .unwrap();
    let persistence = ProviderAuthPersistence::new(
        Arc::new(FileTokenStore::new(root.path().join("vault"))),
        Arc::new(FileLockCoordinator::new(
            root.path().join("credential-locks"),
        )),
    );
    let mut startup = SessionStartup {
        provider_store_root: None,
        storage: storage.clone(),
        config: meerkat::Config::default(),
        factory: meerkat::AgentFactory::new(root.path().join("factory"))
            .with_provider_auth_persistence(persistence.clone()),
        persistence: None,
        connector_auth: None,
        standalone: None,
    };
    let mut owner = None;
    let mut control = None;
    let server = tokio::spawn(server_future);
    let result=std::panic::AssertUnwindSafe(async {
        meerkat::put_connector_client(crate::realm::config_source(&storage).unwrap(),
            &persistence,&storage.locator.realm,"test-profile",meerkat_core::ConnectorClientConfig {
                issuer:origin.clone(),client:"test-client".into(),client_auth:meerkat_core::ConnectorClientAuthMethod::None,
                authorize_params:vec![],resource_indicator:true,protocol:None,
                registered_redirect_uris:vec!["https://console.example/callback".into()],
            },meerkat::ConnectorClientSecretUpdate::Keep).await.unwrap();
        owner=Some(crate::terminal_connector::TerminalConnectors::open(&mut startup).await.unwrap());
        let owner=owner.as_mut().unwrap();
        let session=meerkat_core::SessionId::new();
        let installed=super::connector_unprimed_fixture(storage.locator.clone(),session.clone(),false);
        let folder=root.path().join("installed");std::fs::create_dir_all(folder.join("ready")).unwrap();
        std::fs::create_dir_all(folder.join("configuration")).unwrap();
        let configuration=br#"{"profile":"slack","config":{"token_env":"FIXTURE_TOKEN"}}"#;
        let receipt=b"inert checked package fixture";
        let configuration_hash=format!("{:x}",Sha256::digest(configuration));
        let document=json!({"mcpServers":{"toolkit-services-test":{"command":folder.join("runtime/bin/python"),
            "args":["-I","-m","meerkat_toolkit_services.adapters.services_mcp","serve","--config",folder.join("configuration/selected.json")]}}});
        std::fs::write(folder.join("ready/mcp.json"),serde_json::to_vec(&document).unwrap()).unwrap();
        std::fs::write(folder.join("ready/installed.json"),receipt).unwrap();
        std::fs::write(folder.join("configuration/selected.json"),configuration).unwrap();
        {
            let mut current=installed.package.result.lock().unwrap();
            current["configuration_sha256"]=json!(configuration_hash);current["receipt_sha256"]=json!(format!("{:x}",Sha256::digest(receipt)));
            current["mcp_path"]=json!(folder.join("ready/mcp.json"));current["receipt_path"]=json!(folder.join("ready/installed.json"));
            current["mcp_configuration"]=document;
            installed.package.check.lock().unwrap()["configuration_sha256"]=json!(configuration_hash);
        }
        let mut proposal=json!({"family":"services","profile":"slack","operations":["auth_test"],
            "values":{"token_env":"FIXTURE_TOKEN","timeout_seconds":2,"max_response_bytes":65536}});
        let help=json!({"family":"services","profile":"slack"});
        installed.call("setup_help",help.clone()).await.unwrap();installed.call("setup_check",proposal.clone()).await.unwrap();
        proposal["validation_id"]=json!("a".repeat(32));proposal["configuration_sha256"]=json!(configuration_hash);
        let args=serde_json::value::to_raw_value(&proposal).unwrap();
        let call=meerkat_core::ToolCallView{id:"install",name:"setup_install",args:&args};
        let held=installed.owner.retain_action(call,&session).unwrap();installed.owner.arm_action(held).unwrap();
        let context=meerkat_core::ToolDispatchContext::default().with_runtime_identity(session.clone(),None);
        installed.owner.dispatch_with_context(call,&context).await.unwrap();
        installed.call("setup_help",help).await.unwrap();installed.call("setup_reconcile",json!({"job_id":"3".repeat(32)})).await.unwrap();
        let params:ConnectorStart=serde_json::from_value(super::connector_start("scope",&origin)).unwrap();
        let target=params.connector.into_native(storage.locator.realm.clone()).unwrap();
        control=Some(owner.private_control());
        let control=control.as_mut().unwrap();
        control.start(target,params.redirect_uri,installed.selection(super::connector_operations()).unwrap()).await.unwrap();
        let started=control.started().unwrap();let state=started.state.clone();let handle=started.host_attempt_handle.clone();
        let (retire,mut retired)=watch::channel(false);let (_cancel,cancel)=oneshot::channel();
        let (login,cancellation,cleanup_failed)=control.complete(state,"fixture-code".into(),handle,&mut retired,cancel).await;
        assert!(!cleanup_failed && cancellation.is_none());assert!(matches!(login,Ok(crate::connector_auth::Reply::Completed(_))));
        let (approvals,mut presenter)=LocalApprovals::create_paired_host(root.path()).unwrap();approvals.bind_session(&session).unwrap();
        let (bridge,_messages,retire_bridge)=PromptBridge::channel();
        let admission=crate::terminal_connector::private_service::Admission::new(now()+60_000,retired.clone());
        let dispatcher=(owner.private_services_dispatcher_factory())(installed.owner.clone());
        let mut proposal=super::connector_start("scope",&origin);
        proposal.as_object_mut().unwrap().remove("scope");proposal.as_object_mut().unwrap().remove("redirect_uri");
        proposal["agent"]=json!("rkat-agent");proposal["expires_in_ms"]=json!(60000);
        let raw=serde_json::value::to_raw_value(&proposal).unwrap();
        let call=meerkat_core::ToolCallView{id:"account",name:crate::terminal_connector::TOOL,args:&raw};
        let dispatched=dispatcher.dispatch_with_context(call,&context);tokio::pin!(dispatched);
        let request=tokio::select! { biased;
            _=dispatched.as_mut()=>panic!("exact request must await protected review"),
            request=owner.next()=>request.unwrap(),
        };
        let human=async {
            let result=std::panic::AssertUnwindSafe(async {
                let mut prompt=tokio::time::timeout(Duration::from_secs(5),presenter.next()).await.unwrap().unwrap();
                let body=prompt.record().proposed_action.body.as_ref().unwrap();
                assert_eq!(body["account"]["subject"],"verified-private-account");
                assert_eq!(body["client_profile"]["id"],"test-profile");
                assert_eq!(body["operations"],super::connector_operations());
                assert_eq!(body["agent"],"rkat-agent");assert_eq!(body["expires_in_ms"],60000);
                assert!(!body.to_string().contains("PRIVATE_NATIVE_TOKEN"));
                prompt.decide(meerkat_core::ApprovalDecision::Approve).unwrap();
            }).catch_unwind().await;
            if result.is_err(){retire.send_replace(true);retire_bridge.send_replace(true);}
            result
        };
        let presented=std::panic::AssertUnwindSafe(owner.present_private_services(request,&approvals,&bridge,admission)).catch_unwind();
        let (presented,human)=tokio::join!(presented,human);
        if let Err(panic)=human{std::panic::resume_unwind(panic);}
        let handoff=presented.unwrap().unwrap().unwrap();
        assert!(dispatched.as_mut().now_or_never().is_none());
        assert!(crate::realm::read_own_mcp_config(&storage).await.unwrap().0.is_empty());
        handoff.complete();assert!(dispatched.await.is_ok());
        assert!(owner.has_reviewed());
        control.retire().unwrap();approvals.retire();installed.owner.retire();
        // No administrative finalization is run in this account transport
        // control. The retained reviewed action is discarded after owner close.
        drop(retire_bridge);drop(retire);
    }).catch_unwind().await;
    stop_server.send_replace(true);
    let served = server.await;
    let login_retired = control.as_mut().map(|control| control.retire());
    let retired = owner.as_mut().map(|owner| owner.retire());
    drop(control);
    drop(owner);
    drop(startup);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    assert!(login_retired.is_some_and(|result| result.is_ok()));
    assert!(retired.is_some_and(|result| result.is_ok()));
    assert_eq!(served.unwrap(), 5);
}

/// The completion response stays unread while the host cancels the actual last
/// send. Older or foreign correlation cannot retire a different active input.
#[tokio::test]
async fn v5_buffered_completed_send_cancel_retires_only_latest_owned_binding() {
    for finalizing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let session = super::control_session(root.path()).await;
        let (mut driver, mut output) = driver(5);
        driver.active = None;
        driver.session = Some(session.clone());
        let admission = crate::terminal_connector::private_service::Admission::new(
            driver.binding.as_ref().unwrap().viewer.expires_at_ms,
            driver.plain_cancel.subscribe(),
        );
        let result=std::panic::AssertUnwindSafe(async {
            for send_id in ["h:first","h:second"] {
                driver.frame(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":send_id,
                    "method":"connect/send","params":{"scope":"selected-scope","text":"Return one short answer."}})).unwrap());
                assert_eq!(driver.active.as_ref().unwrap().id,send_id);
                let work=tokio::time::timeout(Duration::from_secs(20),driver.tasks.next()).await.unwrap().unwrap();
                assert!(matches!(&work,Work::Sent(id,observed,_) if id==send_id && observed.settled
                    && matches!(observed.outcome,Ok("completed"|"completed_without_result"))));
                driver.work(work);assert!(driver.active.is_none() && !driver.retired);
            }
            for (id,scope,send_id) in [("h:foreign","foreign","h:second"),
                ("h:arbitrary","selected-scope","unknown"),("h:old","selected-scope","h:first")] {
                driver.frame(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,
                    "method":"connect/cancel","params":{"scope":scope,"send_request_id":send_id}})).unwrap());
                assert!(!driver.retired && !*driver.plain_cancel.borrow());
            }
            driver.frame(br#"{"jsonrpc":"2.0","id":"h:third","method":"connect/send","params":{"scope":"selected-scope","text":"Return one short answer."}}"#);
            driver.frame(br#"{"jsonrpc":"2.0","id":"h:prior-active","method":"connect/cancel","params":{"scope":"selected-scope","send_request_id":"h:second"}}"#);
            assert_eq!(driver.active.as_ref().unwrap().id,"h:third");
            assert!(!driver.retired && !*driver.plain_cancel.borrow());
            let work=tokio::time::timeout(Duration::from_secs(20),driver.tasks.next()).await.unwrap().unwrap();
            assert!(matches!(&work,Work::Sent(id,observed,_) if id=="h:third" && observed.settled
                && matches!(observed.outcome,Ok("completed"|"completed_without_result"))));
            driver.work(work);
            if finalizing { driver.fence("finalizing"); }
            assert!(meerkat_core::AdministrationEntry::enter(&admission).is_ok());
            // No output has been consumed since any native send completed.
            driver.frame(br#"{"jsonrpc":"2.0","id":"h:stop","method":"connect/cancel","params":{"scope":"selected-scope","send_request_id":"h:third"}}"#);
            assert!(driver.retired && *driver.plain_cancel.borrow());
            assert!(meerkat_core::AdministrationEntry::enter(&admission).is_err());
            driver.frame(br#"{"jsonrpc":"2.0","id":"h:after","method":"connect/send","params":{"scope":"selected-scope","text":"must not reopen"}}"#);
            assert!(driver.active.is_none() && driver.tasks.is_empty());
            let rows=rows(&mut output);
            assert!(rows.iter().any(|r|r["id"]=="h:third" && matches!(r["result"]["completion"].as_str(),Some("completed"|"completed_without_result"))));
            assert!(rows.iter().any(|r|r["id"]=="h:stop" && r["result"]["status"]=="not_current"));
            assert!(rows.iter().any(|r|r["id"]=="h:after" && r["error"]["data"]["code"]=="binding_retired"));
        }).catch_unwind().await;
        // Keep every actual send and native owner through failure cleanup.
        driver.fence("test_cleanup");
        while let Some(work) = driver.tasks.next().await {
            driver.work(work);
        }
        driver.session.take();
        drop(driver);
        let closed = Arc::try_unwrap(session).ok().unwrap().close().await;
        if let Err(error) = result {
            let _ = closed;
            std::panic::resume_unwind(error);
        }
        closed.unwrap();
    }
}
