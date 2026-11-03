//! Two actual installations and setup sessions inside one original invocation.
//! The public fixture covers native phase custody, not host account identity or
//! protected-parent acquisition. Requires an exact emitted acceptance host.
use super::*;

#[derive(Clone, Copy)]
enum End {
    Complete,
    CancelSecond,
    OutputLostSecond,
    RetireSecond,
    InputLostSecond,
    DenySecondController,
    DenySetup,
    CancelSetup,
    ChangedSetup,
}

async fn event_since(
    cli: &mut StdioCli,
    provider: &mut Server,
    offset: usize,
    kind: &str,
) -> Value {
    loop {
        if let Some(row) = cli.rows[offset..]
            .iter()
            .find(|row| row["params"]["kind"] == kind && row["params"]["state"] == "observed")
        {
            return row["params"].clone();
        }
        assert!(cli.rows.len() < 8192, "bounded second-service observations");
        provider.with_profile_provider(cli.next()).await;
    }
}
async fn next_available(cli: &mut StdioCli, provider: &mut Server, offset: usize) -> Value {
    loop {
        if let Some(row) = cli.rows[offset..].iter().find(|row| {
            row["params"]["kind"] == "state"
                && row["params"]["state"]["can_open_next_service"] == true
        }) {
            return row["params"]["state"].clone();
        }
        assert!(
            cli.rows.len() < 8192,
            "native transition never became available"
        );
        provider.with_profile_provider(cli.next()).await;
    }
}
async fn available(
    cli: &mut StdioCli,
    provider: &mut Server,
    offset: usize,
    session: &str,
    job: &str,
) {
    loop {
        if cli.rows[offset..].iter().any(|row| {
            row["params"]["kind"] == "service_collection_state"
                && row["params"]["state"] == "available"
                && row["params"]["session_id"] == session
                && row["params"]["job_id"] == job
        }) {
            return;
        }
        assert!(
            cli.rows.len() < 8192,
            "native second-service Ready was not observed"
        );
        provider.with_profile_provider(cli.next()).await;
    }
}
async fn setup_second(
    cli: &mut StdioCli,
    provider: &mut Server,
    scope: &str,
    session: &str,
    helpers: &mut Vec<public::ObservedProcess>,
    approve_controller: bool,
) -> (String, Value) {
    let offset = cli.rows.len();
    cli.request(
        "h:phase-work",
        "connect/send",
        json!({"scope":scope,"text":INSTALL}),
    );
    let install = provider.with_profile_provider(cli.prompt("approval")).await;
    assert_eq!(
        install["params"]["prompt"]["body"]["kind"],
        "service_software_install"
    );
    assert_eq!(install["params"]["prompt"]["body"]["selection"], selected());
    cli.answer(
        install["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":"approve"}),
    );
    let access = provider.with_profile_provider(cli.prompt("approval")).await;
    helpers.extend(public::observed_descendants(cli.child.as_ref().unwrap().id()).await);
    let body = &access["params"]["prompt"]["body"];
    assert_eq!(body["phase"], "public_service_access");
    assert_eq!(body["realm"], REALM);
    assert_eq!(body["configuration_action"], "save_configuration");
    assert_eq!(
        body["grant"]["operations"],
        json!([host_contract::OPERATION, host_contract::USER_OPERATION])
    );
    assert_eq!(body["grant"]["agent"], "rkat-agent");
    assert!(body["grant"]["expires_in_ms"].is_null());
    let job = body["job_id"].as_str().unwrap().to_owned();
    let server = body["server"].clone();
    cli.answer(
        access["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":"approve"}),
    );
    let work = provider
        .with_profile_provider(cli.response("h:phase-work"))
        .await;
    assert_eq!(work["result"]["completion"], "completed");
    // The existing realm is governed. No replacement realm or repeated
    // governance mutation is allowed to supply the second controller review.
    let controller = provider.with_profile_provider(cli.prompt("approval")).await;
    let body = &controller["params"]["prompt"]["body"];
    assert_eq!(body["session_id"], session);
    assert_eq!(body["job_id"], job);
    assert_eq!(
        body["permissions"],
        json!(["exact_controller_route", "native_tool_catalog"])
    );
    assert!(body.get("change").is_none());
    cli.answer(
        controller["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":if approve_controller {"approve"}else{"deny"}}),
    );
    let commission = event_since(cli, provider, offset, "service_commissioning").await;
    assert_eq!(commission["observation"]["job_id"], job);
    assert_eq!(commission["observation"]["session_id"], session);
    assert_eq!(
        commission["observation"]["state"],
        if approve_controller {
            "completed"
        } else {
            "incomplete"
        }
    );
    assert_eq!(commission["observation"]["governance"], "already_governed");
    assert_eq!(commission["observation"]["controller_disposal"], "drained");
    assert_eq!(commission["observation"]["custody_released"], true);
    if !approve_controller {
        assert_eq!(commission["observation"]["grants"], json!([]));
        assert_eq!(commission["observation"]["issuance"], "not_entered");
        assert!(
            !cli.rows[offset..]
                .iter()
                .any(|row| row["params"]["kind"] == "service_finalization"
                    && row["params"]["state"] == "observed")
        );
        return (
            job,
            json!({"server":server,"commissioning":commission,"controller_approved":false}),
        );
    }
    let finalization = event_since(cli, provider, offset, "service_finalization").await;
    assert_eq!(finalization["observation"]["facts"]["persistence"], "saved");
    assert_eq!(
        finalization["observation"]["facts"]["grant"]["state"],
        "selected"
    );
    available(cli, provider, offset, session, &job).await;
    (
        job,
        json!({"server":server,"commissioning":commission,"finalization":finalization}),
    )
}

async fn run_phases(end: End) {
    let label = match end {
        End::Complete => "two-services-complete",
        End::CancelSecond => "two-services-cancel",
        End::OutputLostSecond => "two-services-output-loss",
        End::RetireSecond => "two-services-logout",
        End::InputLostSecond => "two-services-input-eof",
        End::DenySecondController => "second-controller-denied",
        End::DenySetup => "second-setup-denied",
        End::CancelSetup => "second-setup-cancelled",
        End::ChangedSetup => "second-setup-reviewed-config-changed",
    };
    let output = PathBuf::from(
        std::env::var_os("CONNECT_SERVICE_COLLECTION_OUTPUT").expect("fresh evidence parent"),
    )
    .join(label);
    assert!(output.is_absolute() && !output.exists());
    fs::create_dir(&output).unwrap();
    // The test selects the preparation oracle from its actual lifecycle,
    // never from the returned changed flag or a reused model call-ID prefix.
    let lazy_add = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (mut provider, mut gates) = start_provider_with_preparation(BOUND, lazy_add.clone()).await;
    let mut cli = None;
    let mut held = None;
    let mut helpers = Vec::new();
    let mut observation = Value::Null;
    let checked=std::panic::AssertUnwindSafe(async {
        let prefix=PathBuf::from(std::env::var_os("CONNECT_REAL_LIBRARY_PREFIX").expect("supplied resources"));
        let binary=PathBuf::from(std::env::var_os("CONNECT_SERVICE_COLLECTION_HOST").expect("exact emitted host"));
        assert!(prefix.is_absolute()&&binary.is_absolute());assert!(fs::symlink_metadata(&binary).unwrap().is_file());
        assert_eq!(binary.parent(),Some(prefix.join("bin").as_path()));
        assert_eq!(file_digest(&binary),std::env::var("CONNECT_SERVICE_COLLECTION_HOST_SHA256").unwrap());
        assert_eq!(file_digest(&prefix.join("libexec/meerkat-connect/resources.json")),option_env!("MEERKAT_CONNECT_RESOURCES_SHA256").unwrap());
        cli=Some(StdioCli::start_selected_with_multi_collection_host(&provider.origin,binary,40,Duration::from_secs(300)));
        let cli=cli.as_mut().unwrap();let original_pid=cli.child.as_ref().unwrap().id();
        let (scope,first_session)=initialize(cli,&mut provider).await;
        let initial=provider.with_profile_provider(cli.response("h:init")).await;
        assert_eq!(initial["result"]["state"]["service_ids"],json!([host_contract::FIRST_SERVICE,host_contract::SECOND_SERVICE]));
        assert_eq!(initial["result"]["state"]["service_id"],host_contract::FIRST_SERVICE);
        assert_eq!(initial["result"]["state"]["can_open_next_service"],false);
        let before_early=provider.requests.lock().unwrap().len();
        cli.request("h:phase-too-early","connect/collection/open_next",json!({"scope":scope,"service_id":host_contract::SECOND_SERVICE}));
        assert_eq!(provider.with_profile_provider(cli.response("h:phase-too-early")).await["error"]["data"]["code"],"collection_unavailable");
        assert_eq!(provider.requests.lock().unwrap().len(),before_early);
        let (controller,first_job)=reach_controller_review(cli,&mut provider,&scope,&first_session,&mut helpers).await;
        let setup_body=provider.requests.lock().unwrap().last().unwrap().body.clone();
        let first_server=public::reviewed_descriptor(&setup_body,"","hackernews",host_contract::OPERATION);
        cli.answer(controller["id"].as_str().unwrap(),json!({"kind":"approval","decision":"approve"}));
        let first_commission=observed(cli,&mut provider,"service_commissioning","observed").await;
        assert_eq!(first_commission["observation"]["state"],"completed");
        available(cli,&mut provider,0,&first_session,&first_job).await;
        cli.request("h:phase-first","connect/collect",json!({"scope":scope,"job_id":first_job,"source_id":host_contract::SOURCE}));
        let mut first_body=Value::Null;
        for expected in [host_contract::OPERATION,host_contract::USER_OPERATION] {
            let (operation,release)=provider.with_profile_provider(timeout(BOUND,gates.recv())).await.unwrap().unwrap();held=Some(release);
            assert_eq!(operation,expected);helpers.extend(public::observed_descendants(cli.child.as_ref().unwrap().id()).await);
            if expected==host_contract::USER_OPERATION {first_body=provider.requests.lock().unwrap().last().unwrap().body.clone();}
            held.take().unwrap().send(()).expect("first-service model response remains held");
        }
        let first=provider.with_profile_provider(cli.response("h:phase-first")).await;
        verify_report(&first["result"]["report"],&scope,(host_contract::SOURCE,&first_session,&first_job),&first_server.name,&first_body,true);
        let transition=next_available(cli,&mut provider,0).await;
        assert_eq!(transition["service_id"],host_contract::FIRST_SERVICE);
        public::assert_observed_helpers_gone(&helpers).await;
        // Retain the actual installed descriptor, including both approved
        // operations. The tool-result helper projects only one operation.
        let first_config=fs::read_to_string(cli.state.join(REALM).join("config.toml")).unwrap();
        let first_saved_servers=meerkat_core::Config::from_persisted_toml(&first_config).unwrap().0.tools.mcp_servers;
        assert_eq!(first_saved_servers.len(),1);
        assert_eq!(first_saved_servers[0].name,first_server.name);
        let count=provider.requests.lock().unwrap().len();
        for (id,service) in [("h:phase-repeat-first",host_contract::FIRST_SERVICE),("h:phase-unknown","not-selected")] {
            cli.request(id,"connect/collection/open_next",json!({"scope":scope,"service_id":service}));
            assert_eq!(provider.with_profile_provider(cli.response(id)).await["error"]["data"]["code"],"invalid_binding");
        }
        assert_eq!(provider.requests.lock().unwrap().len(),count);
        let offset=cli.rows.len();
        cli.request("h:phase-next","connect/collection/open_next",json!({"scope":scope,"service_id":host_contract::SECOND_SERVICE}));
        let startup_review=provider.with_profile_provider(cli.prompt("approval")).await;
        let startup_body=&startup_review["params"]["prompt"]["body"];
        assert_eq!(startup_body["change"],"review_next_service_setup");
        assert_eq!(startup_body["realm"],REALM);
        assert_eq!(startup_body["supplied_setup"]["name"],"toolkit-setup");
        assert_ne!(startup_body["session_id"],first_session);
        assert!(!cli.rows[offset..].iter().any(|row|row["id"]=="h:phase-next"),"setup review precedes the open result");
        assert!(cli.rows[offset..].iter().any(|row|row["params"]["kind"]=="state"
            &&row["params"]["state"]["service_id"]==host_contract::SECOND_SERVICE));
        // An overlapping request cannot enter another review or helper.
        cli.request("h:phase-overlapping-next","connect/collection/open_next",json!({"scope":scope,"service_id":host_contract::SECOND_SERVICE}));
        assert_eq!(provider.with_profile_provider(cli.response("h:phase-overlapping-next")).await["error"]["data"]["code"],"collection_unavailable");
        if matches!(end,End::DenySetup | End::CancelSetup | End::ChangedSetup) {
            let calls=provider.requests.lock().unwrap().len();
            if matches!(end,End::ChangedSetup) {
                // An external writer makes the reviewed own document unreadable
                // before the answer. The failure must surface AFTER approval,
                // before any grant or authenticated setup can be published.
                fs::write(cli.state.join(REALM).join("config.toml"),format!("{first_config}\n[invalid fixture syntax\n")).unwrap();
            }
            if matches!(end,End::CancelSetup) {
                cli.request("h:phase-setup-cancel","connect/retire",json!({"target":{"scope":scope},"retirement_id":"pending-setup-cancel","reason":"logout"}));
            } else {
                cli.answer(startup_review["id"].as_str().unwrap(),json!({"kind":"approval","decision":
                    if matches!(end,End::ChangedSetup){"approve"}else{"deny"}}));
            }
            let refused=provider.with_profile_provider(cli.response("h:phase-next")).await;
            assert_eq!(refused["error"]["data"]["code"],if matches!(end,End::CancelSetup){"binding_retired"}else{"connector_unavailable"});
            // A failed open closes this invocation itself. Continue reading the
            // original pipe and join that owner instead of sending new work.
            assert!(!provider.with_profile_provider(cli.finish_output()).await.status.success());
            let (_,returned)=read_bounded_json(&cli.root.path().join("returned-collection-failure.json"),8*1024*1024);
            assert_eq!(returned["reports"],json!([first["result"]["report"]]));
            assert_eq!(returned["original_error_retained"],true);
            assert_eq!(returned["delivery"],"not_claimed");
            assert_eq!(provider.requests.lock().unwrap().len(),calls);
            if matches!(end,End::ChangedSetup) {
                fs::write(cli.state.join(REALM).join("config.toml"),&first_config).unwrap();
            }
            let own=fs::read_to_string(cli.state.join(REALM).join("config.toml")).unwrap();
            let saved=meerkat_core::Config::from_persisted_toml(&own).unwrap().0.tools.mcp_servers;
            assert_eq!(saved,first_saved_servers,"refused setup cannot save its prepared helper");
            observation=json!({"first_collection":first,"first_commissioning":first_commission,
                "startup_review":startup_review,"refused":refused,
                "returned_rust_failure":returned,"saved_installations":saved,"original_pid":original_pid});
            return;
        }
        cli.answer(startup_review["id"].as_str().unwrap(),json!({"kind":"approval","decision":"approve"}));
        let next=provider.with_profile_provider(cli.response("h:phase-next")).await;
        assert_eq!(next["result"]["session_id"],startup_body["session_id"]);
        assert_eq!(cli.rows[offset..].iter().filter(|row|row["params"]["prompt"]["body"]["change"]=="review_next_service_setup").count(),1);
        assert!(next.get("error").is_none(),"actual next-service setup failed: {next}");
        assert_eq!(next["result"]["scope"],scope);assert_eq!(next["result"]["service_id"],host_contract::SECOND_SERVICE);
        let second_session=next["result"]["session_id"].as_str().unwrap().to_owned();assert_ne!(second_session,first_session);
        assert_eq!(cli.child.as_ref().unwrap().id(),original_pid);
        assert_eq!(cli.rows.iter().filter(|row|row["id"]=="h:init").count(),1);
        assert_eq!(next["result"]["state"]["can_collect"],false);
        assert_eq!(next["result"]["state"]["can_open_next_service"],false);
        // A completed first request cannot retire the new setup.
        cli.request("h:phase-stale-cancel","connect/cancel",json!({"scope":scope,"send_request_id":"h:phase-first"}));
        assert_eq!(provider.with_profile_provider(cli.response("h:phase-stale-cancel")).await["result"]["status"],"not_current");
        let before_second_setup=provider.requests.lock().unwrap().len();
        // Startup review and the new session result are already observed.
        // Its first conversation must reuse that eagerly retained helper.
        lazy_add.store(false,std::sync::atomic::Ordering::Release);
        let (second_job,second_setup)=setup_second(cli,&mut provider,&scope,&second_session,&mut helpers,
            !matches!(end,End::DenySecondController)).await;
        assert_ne!(second_job,first_job);
        let second_server=second_setup["server"]["name"].as_str().unwrap();assert_ne!(second_server,first_server.name);
        assert!(!cli.rows[offset..].iter().any(|row|row["params"]["prompt"]["body"]["change"]=="govern_existing_realm"));
        if matches!(end,End::DenySecondController) {
            let count=provider.requests.lock().unwrap().len();
            cli.request("h:phase-denied-collect","connect/collect",json!({"scope":scope,"job_id":second_job,"source_id":host_contract::SECOND_SOURCE}));
            assert_eq!(provider.with_profile_provider(cli.response("h:phase-denied-collect")).await["error"]["data"]["code"],"collection_unavailable");
            cli.request("h:phase-denied-next","connect/collection/open_next",json!({"scope":scope,"service_id":host_contract::SECOND_SERVICE}));
            assert_eq!(provider.with_profile_provider(cli.response("h:phase-denied-next")).await["error"]["data"]["code"],"collection_unavailable");
            cli.request("h:phase-denied-close","connect/close",json!({"target":{"scope":scope}}));
            let closed=provider.with_profile_provider(cli.response("h:phase-denied-close")).await;
            assert_eq!(closed["error"]["data"]["code"],"shutdown_unconfirmed");
            assert!(!provider.with_profile_provider(cli.finish_output()).await.status.success());
            let (_,returned)=read_bounded_json(&cli.root.path().join("returned-collection-failure.json"),8*1024*1024);
            assert_eq!(returned["reports"],json!([first["result"]["report"]]));
            assert_eq!(returned["delivery"],"not_claimed");assert_eq!(returned["original_error_retained"],true);
            assert_eq!(provider.requests.lock().unwrap().len(),count);
            assert!(provider.requests.lock().unwrap()[before_second_setup..].iter().all(|request|instruction(request).is_none()));
            assert!(!cli.rows[offset..].iter().any(|row|row["params"]["kind"]=="service_collection_state"
                &&row["params"]["state"]=="available"));
            let own=fs::read_to_string(cli.state.join(REALM).join("config.toml")).unwrap();
            let saved=meerkat_core::Config::from_persisted_toml(&own).unwrap().0.tools.mcp_servers;
            assert_eq!(saved.len(),first_saved_servers.len()+1);
            assert_eq!(saved.iter().filter(|server|server.name!="toolkit-setup").cloned().collect::<Vec<_>>(),first_saved_servers);
            let reviewed_helper:meerkat_core::McpServerConfig=serde_json::from_value(startup_body["supplied_setup"].clone()).unwrap();
            assert_eq!(saved.iter().find(|server|server.name=="toolkit-setup"),Some(&reviewed_helper));
            public::assert_observed_helpers_gone(&helpers).await;
            observation=json!({"first_collection":first,"first_commissioning":first_commission,"next":next,
                "second_setup":second_setup,"original_pid":original_pid,"returned_rust_failure":returned,
                "close":closed,"saved_installations":{"before_second":first_saved_servers,"after_denial":saved}});
            return;
        }
        let count=provider.requests.lock().unwrap().len();
        for (id,job,source) in [("h:phase-old-job",first_job.as_str(),host_contract::SECOND_SOURCE),
            ("h:phase-old-source",second_job.as_str(),host_contract::SOURCE)] {
            cli.request(id,"connect/collect",json!({"scope":scope,"job_id":job,"source_id":source}));
            assert_eq!(provider.with_profile_provider(cli.response(id)).await["error"]["data"]["code"],"invalid_binding");
        }
        assert_eq!(provider.requests.lock().unwrap().len(),count);
        cli.request("h:phase-second","connect/collect",json!({"scope":scope,"job_id":second_job,"source_id":host_contract::SECOND_SOURCE}));
        let (operation,release)=provider.with_profile_provider(timeout(BOUND,gates.recv())).await.unwrap().unwrap();held=Some(release);
        assert_eq!(operation,host_contract::OPERATION);helpers.extend(public::observed_descendants(original_pid).await);
        held.take().unwrap().send(()).expect("second-service first model response remains held");
        let (operation,release)=provider.with_profile_provider(timeout(BOUND,gates.recv())).await.unwrap().unwrap();held=Some(release);
        assert_eq!(operation,host_contract::USER_OPERATION);helpers.extend(public::observed_descendants(original_pid).await);
        let second_body=provider.requests.lock().unwrap().last().unwrap().body.clone();
        observation=json!({"first_collection":first,"first_commissioning":first_commission,"next":next,"second_setup":second_setup,"original_pid":original_pid});
        match end {
            End::Complete=>{
                held.take().unwrap().send(()).expect("second-service final model response remains held");
                let second=provider.with_profile_provider(cli.response("h:phase-second")).await;
                verify_report(&second["result"]["report"],&scope,(host_contract::SECOND_SOURCE,&second_session,&second_job),second_server,&second_body,true);
                assert_ne!(first["result"]["report"]["requirements"][0]["calls"][0]["native"]["installed_connection_sha256"],
                    second["result"]["report"]["requirements"][0]["calls"][0]["native"]["installed_connection_sha256"]);
                assert_eq!(second["result"]["state"]["can_open_next_service"],false);
                observation["second_collection"]=second;
                prior::close(cli,&mut provider,&scope).await;
            },
            End::DenySecondController | End::DenySetup | End::CancelSetup | End::ChangedSetup=>unreachable!("denied review returned before collection admission"),
            End::CancelSecond|End::OutputLostSecond|End::RetireSecond|End::InputLostSecond=>{
                match end {
                    End::CancelSecond=>{
                        cli.request("h:phase-cancel","connect/cancel",json!({"scope":scope,"send_request_id":"h:phase-second"}));
                        assert_eq!(provider.with_profile_provider(cli.response("h:phase-cancel")).await["result"]["status"],"cancellation_requested");
                    },
                    End::RetireSecond=>{
                        cli.request("h:phase-logout","connect/retire",json!({"target":{"scope":scope},"retirement_id":"second-service-logout","reason":"logout"}));
                    },
                    End::InputLostSecond=>{
                        // Close only the original parent's write endpoint. The
                        // reader stays open to receive the actual joined report.
                        drop(cli.child.as_mut().unwrap().stdin.take().unwrap());
                        observation["original_input_closed"]=json!(true);
                    },
                    End::OutputLostSecond=>{
                        drop(cli.stdout.take().unwrap());cli.request("h:phase-broken-output","connect/history",json!({"scope":scope}));
                    },
                    _=>unreachable!("terminal failure case"),
                }
                if !matches!(end,End::OutputLostSecond) {
                    let second=provider.with_profile_provider(cli.response("h:phase-second")).await;
                    verify_report(&second["result"]["report"],&scope,(host_contract::SECOND_SOURCE,&second_session,&second_job),second_server,&second_body,false);
                    observation["second_collection"]=second;
                    if matches!(end,End::RetireSecond) {
                        let retired=provider.with_profile_provider(cli.response("h:phase-logout")).await;
                        assert_eq!(retired["error"]["data"]["code"],"shutdown_unconfirmed");
                        let report_index=cli.rows.iter().position(|row|row["id"]=="h:phase-second").unwrap();
                        let retired_index=cli.rows.iter().position(|row|row["id"]=="h:phase-logout").unwrap();
                        assert!(report_index<retired_index,"retirement cannot acknowledge before the retained collection joins");
                        observation["retirement"]=retired;
                    }
                    if !matches!(end,End::InputLostSecond) {
                        let count=provider.requests.lock().unwrap().len();
                        cli.request("h:phase-stopped-next","connect/collection/open_next",json!({"scope":scope,"service_id":host_contract::SECOND_SERVICE}));
                        assert_eq!(provider.with_profile_provider(cli.response("h:phase-stopped-next")).await["error"]["data"]["code"],"collection_unavailable");
                        assert_eq!(provider.requests.lock().unwrap().len(),count);
                        cli.request("h:phase-close","connect/close",json!({"target":{"scope":scope}}));
                        assert_eq!(provider.with_profile_provider(cli.response("h:phase-close")).await["error"]["data"]["code"],"shutdown_unconfirmed");
                    }
                }
                assert!(!provider.with_profile_provider(cli.finish_output()).await.status.success());
                let (_,returned)=read_bounded_json(&cli.root.path().join("returned-collection-failure.json"),8*1024*1024);
                assert_eq!(returned["reports"].as_array().unwrap().len(),2);
                assert_eq!(returned["reports"][0],first["result"]["report"]);
                verify_report(&returned["reports"][1],&scope,(host_contract::SECOND_SOURCE,&second_session,&second_job),second_server,&second_body,false);
                assert_eq!(returned["delivery"],"not_claimed");assert_eq!(returned["original_error_retained"],true);
                observation["returned_rust_failure"]=returned;
            },
        }
        public::assert_observed_helpers_gone(&helpers).await;
        assert_eq!(cli.rows.iter().filter(|row|row["id"]=="h:init").count(),1);
    }).catch_unwind().await;
    // A failed assertion still joins the exact original child. Forced cleanup
    // remains a failing observation, never successful lifecycle evidence.
    let child_cleanup = std::panic::AssertUnwindSafe(async {
        match cli.as_mut() {
            Some(cli) => cleanup_child(cli).await,
            None => true,
        }
    })
    .catch_unwind()
    .await;
    let provider_cleanup = cleanup_provider(&mut provider).await;
    drop(held);
    let child_cleanup = matches!(child_cleanup, Ok(true));
    let helpers_gone = std::panic::AssertUnwindSafe(public::assert_observed_helpers_gone(&helpers))
        .catch_unwind()
        .await
        .is_ok();
    let evidence = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let retained = cli.as_mut().map(|cli| {
            let root = std::mem::replace(&mut cli.root, tempfile::tempdir().unwrap()).keep();
            write_json(&output.join("retained-root.json"), &json!({"root":root}));
            write_json(&output.join("private-frames.json"), &json!(cli.rows));
            json!({"root":root,"host":cli.binary,"host_sha256":file_digest(&cli.binary)})
        });
        write_json(
            &output.join("model-requests.json"),
            &json!(
                provider
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|r| json!({"method":r.method,"path":r.path,"body":r.body}))
                    .collect::<Vec<_>>()
            ),
        );
        write_json(
            &output.join("OBSERVATIONS.json"),
            &json!({"status":if checked.is_ok()&&child_cleanup&&provider_cleanup&&helpers_gone{"passed"}else{"failed"},
            "retained":retained,"observations":observation,"child_cleanup_orderly":child_cleanup,"fixture_provider_joined":provider_cleanup,
            "observed_helpers":process_rows(&helpers),"helpers_gone":helpers_gone,"source_kind":"two_distinct_public_hackernews_installations",
            "same_invocation_enforced_by_committed_result_program":true,"host_account_qualified":false,"protected_parent_qualified":false,"full_goal_ready":false}),
        );
    }));
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = evidence {
        std::panic::resume_unwind(panic);
    }
    assert!(
        child_cleanup && provider_cleanup && helpers_gone,
        "actual two-service owners did not join normally"
    );
}
#[tokio::test]
#[ignore = "exact emitted v7 host and real library; two public installations, one original invocation"]
async fn actual_v7_two_services_share_committed_invocation_and_close() {
    run_phases(End::Complete).await;
}
#[tokio::test]
#[ignore = "exact emitted v7 host and real library; cancel during the second installed service"]
async fn actual_v7_second_service_cancel_retains_both_reports() {
    run_phases(End::CancelSecond).await;
}
#[tokio::test]
#[ignore = "exact emitted v7 host and real library; lose original output during the second installed service"]
async fn actual_v7_second_service_output_loss_joins_original_owner() {
    run_phases(End::OutputLostSecond).await;
}

#[tokio::test]
#[ignore = "exact emitted v7 host and real library; logout during the second installed service"]
async fn actual_v7_second_service_logout_joins_before_retirement_reply() {
    run_phases(End::RetireSecond).await;
}
#[tokio::test]
#[ignore = "exact emitted v7 host and real library; close original input during the second installed service"]
async fn actual_v7_second_service_input_eof_retains_both_reports() {
    run_phases(End::InputLostSecond).await;
}
#[tokio::test]
#[ignore = "exact emitted v7 host and real library; deny actual second-service controller review"]
async fn actual_v7_denied_second_controller_preserves_first_installation() {
    run_phases(End::DenySecondController).await;
}

#[tokio::test]
#[ignore = "exact emitted v7 host and real library; deny protected startup before the authenticated second runtime"]
async fn actual_v7_denied_next_setup_retains_first_report_without_saving_helper() {
    run_phases(End::DenySetup).await;
}

#[tokio::test]
#[ignore = "exact emitted v7 host and real library; cancel during the protected next-setup review"]
async fn actual_v7_cancel_pending_next_setup_joins_original_owner() {
    run_phases(End::CancelSetup).await;
}
#[tokio::test]
#[ignore = "exact emitted v7 host and real library; changed own configuration fails after approval"]
async fn actual_v7_changed_next_setup_review_preserves_original_failure() {
    run_phases(End::ChangedSetup).await;
}
