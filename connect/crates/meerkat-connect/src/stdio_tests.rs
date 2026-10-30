//! Private driver controls. Actual executable/native composition is exercised
//! separately; these tests never claim a browser or installed host run.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::AsyncWriteExt;

fn driver() -> (Driver, mpsc::Receiver<OutputFrame>) {
    let (tx, rx) = mpsc::channel(OUTPUT_FRAMES);
    let (driver, _, _) = new_driver(tx, Arc::new(Semaphore::new(MAX_FRAME * 2)));
    (driver, rx)
}
fn drain(rx: &mut mpsc::Receiver<OutputFrame>) -> Vec<Value> {
    let mut rows = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        rows.push(serde_json::from_slice(&frame.bytes).unwrap());
    }
    rows
}
fn active(driver: &mut Driver) {
    driver.initialize_id = Some("h:init".into());
    driver.scope = Some("test-scope".into());
    let (cancel, _) = watch::channel(false);
    driver.active = Some(ActiveSend {
        provisional_closed: false,
        id: "h:send".into(),
        cancel,
        receipt: true,
        cancel_result: None,
        cancel_requests: Vec::new(),
    });
}
fn retire(driver: &mut Driver) {
    driver.frame(br#"{"jsonrpc":"2.0","id":"h:retire","method":"connect/retire","params":{"target":{"scope":"test-scope"},"retirement_id":"logout","reason":"logout"}}"#);
}
fn binding(root: &std::path::Path) -> Value {
    json!({"protocol_version":1,"application":{"application_id":"reading","instance_id":"one","origin":"http://127.0.0.1:12345"},"viewer":{"principal_id":"actual-viewer","login_scope_id":"login-one","expires_at_ms":now()+60_000},"realm":{"realm_id":"stdio-test","context_root":root,"state_root":root.join("state"),"user_config_root":root.join("user")}})
}

fn service_binding(root: &std::path::Path) -> Value {
    let mut value = binding(root);
    value["protocol_version"] = json!(3);
    value["adapter"] = json!("forge_services");
    value["services"] = json!({"resource_manifest_sha256":
        option_env!("MEERKAT_CONNECT_RESOURCES_SHA256").unwrap_or(&"0".repeat(64)),
        "preparation":"permitted"});
    value
}

fn generic_service_binding(root: &std::path::Path) -> Value {
    let mut value = service_binding(root);
    value["adapter"] = json!("services");
    value["application"] = json!({"application_id":"host-console",
        "instance_id":"service-instance", "origin":"https://console.example"});
    value
}

#[test]
fn services_v3_preserves_trusted_host_binding_and_legacy_origin_rules() {
    let root = tempfile::tempdir().unwrap();
    let value = generic_service_binding(root.path());
    let decoded: Initialize = serde_json::from_value(value.clone()).unwrap();
    assert!(decoded.valid());
    assert!(decoded.adapter.is_services());
    assert_eq!(decoded.viewer.principal_id, "actual-viewer");
    for origin in [
        "https://console.example",
        "http://homeserver:8080",
        "http://127.0.0.1:12345",
    ] {
        let mut allowed = value.clone();
        allowed["application"]["origin"] = json!(origin);
        assert!(
            serde_json::from_value::<Initialize>(allowed)
                .unwrap()
                .valid()
        );
    }
    for origin in [
        "https://console.example:443",
        "https://console.example/",
        "https://CONSOLE.example",
        "https://console.example/path",
        "https://console.example?x=1",
        "https://console.example#fragment",
        "https://user:password@console.example",
        "https://console.example/../",
        "https://console.example\\",
        "ws://console.example",
    ] {
        let mut wrong = value.clone();
        wrong["application"]["origin"] = json!(origin);
        assert!(!serde_json::from_value::<Initialize>(wrong).unwrap().valid());
    }
    for (pointer, invalid) in [
        ("/application/application_id", json!("")),
        ("/application/instance_id", json!("")),
        ("/viewer/principal_id", json!("")),
        ("/viewer/login_scope_id", json!("")),
        ("/viewer/expires_at_ms", json!(0)),
        ("/realm/state_root", json!("relative")),
    ] {
        let mut wrong = value.clone();
        *wrong.pointer_mut(pointer).unwrap() = invalid;
        assert!(!serde_json::from_value::<Initialize>(wrong).unwrap().valid());
    }
    for field in ["paired_host", "credential", "resource_path"] {
        let mut wrong = value.clone();
        wrong[field] = json!({});
        assert!(serde_json::from_value::<Initialize>(wrong).is_err());
    }
    let mut legacy = value.clone();
    legacy["adapter"] = json!("forge_services");
    assert!(
        !serde_json::from_value::<Initialize>(legacy)
            .unwrap()
            .valid()
    );
    for version in [1, 2] {
        let mut wrong = value.clone();
        wrong["protocol_version"] = json!(version);
        wrong.as_object_mut().unwrap().remove("services");
        let decoded = serde_json::from_value::<Initialize>(wrong);
        assert!(decoded.is_err() || !decoded.unwrap().valid());
    }
}

#[test]
fn service_v3_is_a_separate_closed_bootstrap_and_answer_union() {
    let root = tempfile::tempdir().unwrap();
    let value = service_binding(root.path());
    assert!(
        serde_json::from_value::<Initialize>(value.clone())
            .unwrap()
            .valid()
    );
    for (key, extra) in [
        ("paired_host", json!({})),
        ("credential", json!("private")),
        ("resource_path", json!("/tmp/other")),
    ] {
        let mut wrong = value.clone();
        wrong[key] = extra;
        assert!(serde_json::from_value::<Initialize>(wrong).is_err());
    }
    for version in [1, 2] {
        let mut wrong = value.clone();
        wrong["protocol_version"] = json!(version);
        assert!(serde_json::from_value::<Initialize>(wrong).is_err());
    }
    for origin in [
        "https://127.0.0.1:12345",
        "http://localhost:12345",
        "http://127.0.0.1:12345/",
    ] {
        let mut wrong = value.clone();
        wrong["application"]["origin"] = json!(origin);
        assert!(!serde_json::from_value::<Initialize>(wrong).unwrap().valid());
    }
    let action = json!({"kind":"provider_action","actions":["back"]});
    assert!(!valid_answer(
        &action,
        &Answer::ProviderAction {
            action: ProviderAction::EnterKey
        }
    ));
    assert!(!valid_answer(
        &json!({"kind":"approval"}),
        &Answer::ProviderApiKey {
            key: "private".into()
        }
    ));
    assert!(
        serde_json::from_value::<AnswerEnvelope>(json!({"answer":{"answer":{"kind":"cancel"}}}))
            .is_err()
    );
}

#[tokio::test]
async fn service_v3_unqualified_lifecycle_refuses_before_startup_or_artifacts() {
    for (generic, mismatched) in [(false, false), (false, true), (true, false), (true, true)] {
        let root = tempfile::tempdir().unwrap();
        let mut value = if generic {
            generic_service_binding(root.path())
        } else {
            service_binding(root.path())
        };
        if mismatched {
            let digest = value["services"]["resource_manifest_sha256"]
                .as_str()
                .unwrap();
            value["services"]["resource_manifest_sha256"] = json!(format!(
                "{}{}",
                if digest.starts_with('0') { '1' } else { '0' },
                &digest[1..]
            ));
        }
        let (tx, mut rx) = mpsc::channel(OUTPUT_FRAMES);
        let (mut d, _, _) = new_driver(tx, Arc::new(Semaphore::new(MAX_FRAME * 2)));
        d.frame(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"h:init","method":"connect/initialize","params":value})).unwrap());
        assert!(d.opening);
        let initial = drain(&mut rx);
        assert_eq!(initial.len(), 1);
        assert_eq!(
            initial[0]["params"],
            json!({"target":{"initialize_request_id":"h:init"},"kind":"state","state":{
            "available":false,"can_send":false,"can_stop":false,"outcome_uncertain":false,
            "active_send":null,"last_completion":null,"private_notice_pending":false}})
        );
        let work = d.tasks.next().await.unwrap();
        d.work(work);
        assert!(d.session.is_none());
        assert!(d.scope.is_none());
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
        let rows = drain(&mut rx);
        let reply = rows.iter().find(|row| row["id"] == "h:init").unwrap();
        assert_eq!(
            reply["error"]["data"]["code"],
            if !mismatched && option_env!("MEERKAT_CONNECT_RESOURCES_SHA256").is_some() {
                "unavailable"
            } else {
                "invalid_binding"
            }
        );
        assert!(rows.iter().all(|row| row["method"] != "connect/prompt"));
    }
}

#[tokio::test]
async fn service_v3_private_key_answers_preserve_one_prompt_and_exact_bytes() {
    for generic in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(OUTPUT_FRAMES);
        let (mut d, _, mut prompts) = new_driver(tx, Arc::new(Semaphore::new(MAX_FRAME * 2)));
        let binding = if generic {
            generic_service_binding(root.path())
        } else {
            service_binding(root.path())
        };
        d.binding = Some(serde_json::from_value(binding).unwrap());
        d.initialize_id = Some("h:init".into());
        d.opening = true;
        let bridge = d.bridge.clone();
        let request = bridge.ask(json!({"kind":"provider_api_key","target":{"realm":"owner","binding":"selected"},"max_bytes":8192}));
        tokio::pin!(request);
        assert!(request.as_mut().now_or_never().is_none());
        d.prompt(prompts.recv().await.unwrap());
        let original = drain(&mut rx).pop().unwrap();
        let id = original["id"].as_str().unwrap();
        assert_eq!(
            original["params"]["target"],
            json!({"initialize_request_id":"h:init"})
        );
        assert!(original["params"].get("scope").is_none());
        for answer in [
            json!({"kind":"provider_action","action":"back"}),
            json!({"kind":"provider_api_key","key":"\n"}),
        ] {
            d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":answer}}));
            assert_eq!(d.pending.as_ref().unwrap().id, id);
        }
        d.answer(json!({"jsonrpc":"2.0","id":"p:wrong","result":{"answer":{"kind":"cancel"}}}));
        assert_eq!(d.pending.as_ref().unwrap().id, id);
        let key = "  synthetic-private-key-☃ /quit  ";
        d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"provider_api_key","key":key}}}));
        assert!(
            matches!(request.await.unwrap(), Answer::ProviderApiKey{key:actual} if actual==key)
        );
        assert!(!d.retired);
        d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"cancel"}}}));
        let rows = drain(&mut rx);
        assert!(rows.iter().all(|row| !row.to_string().contains(key)));
        assert_eq!(
            rows.last().unwrap()["error"]["data"]["code"],
            "stale_prompt"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row["params"]["kind"] == "prompt_closed")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn service_v3_owner_notice_closure_is_acknowledged_before_init_result() {
    for (disposition, expected) in [
        (SignInDisposition::Refused, "refused"),
        (SignInDisposition::Completed, "owner_completed"),
        (SignInDisposition::Expired, "expired"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(OUTPUT_FRAMES);
        let (mut d, _, mut prompts) = new_driver(tx, Arc::new(Semaphore::new(MAX_FRAME * 2)));
        d.binding = Some(serde_json::from_value(service_binding(root.path())).unwrap());
        d.initialize_id = Some("h:init".into());
        d.opening = true;
        let notice = d.bridge.provider_notice(json!({"kind":"provider_browser","target":{"realm":"owner","binding":"selected"},"authorization_url":"https://example.invalid/private-canary"})).await.unwrap();
        d.prompt(prompts.recv().await.unwrap());
        let original = drain(&mut rx).pop().unwrap();
        let id = original["id"].as_str().unwrap();
        assert_eq!(d.state()["private_notice_pending"], false);
        assert_eq!(d.state()["active_send"], Value::Null);
        d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"approval","decision":"approve"}}}));
        assert_eq!(d.pending.as_ref().unwrap().id, id);
        let close = notice.finish(disposition);
        tokio::pin!(close);
        assert!(close.as_mut().now_or_never().is_none());
        let PromptClosure::Owner(observed) = pending_prompt_closed(&mut d.pending).await else {
            panic!("The retained native owner must close this prompt");
        };
        d.owner_closed(observed);
        close.await.unwrap();
        assert!(d.pending.is_none());
        d.work(Work::Opened(Box::new(Err("native_auth_required"))));
        let rows = drain(&mut rx);
        let closed = rows
            .iter()
            .position(|row| row["params"]["kind"] == "prompt_closed")
            .unwrap();
        assert_eq!(rows[closed]["params"]["disposition"], expected);
        assert!(closed < rows.iter().position(|row| row["id"] == "h:init").unwrap());
        assert!(
            rows.iter()
                .all(|row| !row.to_string().contains("private-canary"))
        );
    }
}

#[tokio::test]
async fn pending_prompt_closure_keeps_channels_after_cancel_and_owner_priority() {
    for owner_ready in [false, true] {
        let (response, receiver) = oneshot::channel();
        let (owner, owner_close) = oneshot::channel();
        let mut pending = Some(PendingPrompt {
            id: "p:retained".into(),
            prompt: json!({"kind":"provider_device"}),
            response: Some(response),
            owner_close: Some(owner_close),
        });
        // An unrelated outer-select branch drops this pending wait.
        assert!(pending_prompt_closed(&mut pending).now_or_never().is_none());
        assert_eq!(pending.as_ref().unwrap().id, "p:retained");
        assert!(!owner.is_closed());
        assert!(
            !pending
                .as_ref()
                .unwrap()
                .response
                .as_ref()
                .unwrap()
                .is_closed()
        );
        drop(receiver);
        let (ack, joined) = oneshot::channel();
        if owner_ready {
            assert!(owner.send((SignInDisposition::Completed, ack)).is_ok());
            let PromptClosure::Owner(Ok((SignInDisposition::Completed, ack))) =
                pending_prompt_closed(&mut pending).await
            else {
                panic!("Ready owner closure must precede response closure");
            };
            assert!(ack.send(()).is_ok());
            joined.await.unwrap();
        } else {
            assert!(matches!(
                pending_prompt_closed(&mut pending).await,
                PromptClosure::Response
            ));
            assert!(!owner.is_closed());
        }
        assert_eq!(pending.as_ref().unwrap().id, "p:retained");
    }
    let mut display = Some(PendingPrompt {
        id: "p:display".into(),
        prompt: json!({"kind":"paired_host_notice"}),
        response: None,
        owner_close: None,
    });
    assert!(pending_prompt_closed(&mut display).now_or_never().is_none());
    assert_eq!(display.as_ref().unwrap().id, "p:display");
}

#[tokio::test]
async fn service_v3_retirement_keeps_original_init_until_entered_work_joins() {
    let root = tempfile::tempdir().unwrap();
    let (tx, mut rx) = mpsc::channel(OUTPUT_FRAMES);
    let (mut d, _, mut prompts) = new_driver(tx, Arc::new(Semaphore::new(MAX_FRAME * 2)));
    d.binding = Some(serde_json::from_value(service_binding(root.path())).unwrap());
    d.initialize_id = Some("h:init".into());
    d.opening = true;
    let mut notice = d.bridge.provider_notice(json!({"kind":"provider_device","target":{},"verification_uri":"https://example.invalid","user_code":"private-code"})).await.unwrap();
    d.prompt(prompts.recv().await.unwrap());
    drain(&mut rx);
    d.frame(br#"{"jsonrpc":"2.0","id":"h:retire","method":"connect/retire","params":{"target":{"initialize_request_id":"h:init"},"retirement_id":"logout","reason":"logout"}}"#);
    notice.cancelled().await;
    d.settle();
    let rows = drain(&mut rx);
    assert!(
        rows.iter()
            .any(|row| row["params"]["kind"] == "prompt_closed")
    );
    assert!(!rows.iter().any(|row| row["id"] == "h:retire"));
    d.work(Work::Opened(Box::new(Err("binding_retired"))));
    d.settle();
    let rows = drain(&mut rx);
    assert_eq!(
        rows.iter().find(|row| row["id"] == "h:init").unwrap()["error"]["data"]["code"],
        "binding_retired"
    );
    assert_eq!(
        rows.iter().find(|row| row["id"] == "h:retire").unwrap()["result"]["entered_work"],
        "settled"
    );
    assert!(d.scope.is_none());
}

#[tokio::test]
async fn service_v3_viewer_expiry_closes_before_original_init_settles() {
    for explicit_retire in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(OUTPUT_FRAMES);
        let (mut d, _, mut prompts) = new_driver(tx, Arc::new(Semaphore::new(MAX_FRAME * 2)));
        d.binding = Some(serde_json::from_value(service_binding(root.path())).unwrap());
        d.initialize_id = Some("h:init".into());
        d.opening = true;
        let mut notice = d.bridge.provider_notice(json!({"kind":"provider_device","target":{},"verification_uri":"https://example.invalid","user_code":"private-expiry-code"})).await.unwrap();
        d.prompt(prompts.recv().await.unwrap());
        let original = drain(&mut rx).pop().unwrap();
        let id = original["id"].as_str().unwrap();
        if !explicit_retire {
            d.binding.as_mut().unwrap().viewer.expires_at_ms = now().saturating_sub(1);
            d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"cancel"}}}));
        }
        d.frame(br#"{"jsonrpc":"2.0","id":"h:retire","method":"connect/retire","params":{"target":{"initialize_request_id":"h:init"},"retirement_id":"expired-login","reason":"login_expired"}}"#);
        notice.cancelled().await;
        d.settle();
        let rows = drain(&mut rx);
        let closed: Vec<_> = rows
            .iter()
            .filter(|row| row["params"]["kind"] == "prompt_closed")
            .collect();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0]["params"]["prompt_id"], id);
        assert_eq!(closed[0]["params"]["disposition"], "expired");
        assert_eq!(
            closed[0]["params"]["target"],
            json!({"initialize_request_id":"h:init"})
        );
        assert!(
            !rows
                .iter()
                .any(|row| row["id"] == "h:init" || row["id"] == "h:retire")
        );
        assert!(
            rows.iter()
                .all(|row| !row.to_string().contains("private-expiry-code"))
        );
        assert!(d.scope.is_none());
        d.work(Work::Opened(Box::new(Err("binding_retired"))));
        d.settle();
        let rows = drain(&mut rx);
        assert_eq!(
            rows.iter().find(|row| row["id"] == "h:init").unwrap()["error"]["data"]["code"],
            "binding_retired"
        );
        assert_eq!(
            rows.iter().find(|row| row["id"] == "h:retire").unwrap()["result"]["entered_work"],
            "settled"
        );
        assert!(d.scope.is_none());
    }
    // Earlier wire versions retain their existing label for the same fence.
    for version in [1, 2] {
        let root = tempfile::tempdir().unwrap();
        let (mut d, mut rx) = driver();
        d.binding = Some(serde_json::from_value(binding(root.path())).unwrap());
        d.binding.as_mut().unwrap().protocol_version = version;
        active(&mut d);
        let (response, _receiver) = oneshot::channel();
        d.pending = Some(PendingPrompt {
            id: "p:legacy".into(),
            prompt: json!({"kind":"approval"}),
            response: Some(response),
            owner_close: None,
        });
        d.fence("expired");
        assert_eq!(
            drain(&mut rx)
                .iter()
                .find(|row| row["params"]["kind"] == "prompt_closed")
                .unwrap()["params"]["disposition"],
            "retired"
        );
    }
}

#[tokio::test]
async fn service_composition_keeps_frontends_absent_and_unchecked_install_refused() {
    let (default, _default_worker) = LazySetup::private_services();
    let router =
        meerkat::McpRouter::new_with_stdio_profile(meerkat::McpStdioLaunchProfile::trusted_host());
    let original_slot = router.external_surface_handle_slot().unwrap();
    // Native unbound handles observe construction identity, not authority.
    let replacement_router =
        meerkat::McpRouter::new_with_stdio_profile(meerkat::McpStdioLaunchProfile::trusted_host());
    let replacement_slot = replacement_router.external_surface_handle_slot().unwrap();
    let replacement = replacement_slot.read().unwrap().clone();
    assert!(!Arc::ptr_eq(&original_slot.read().unwrap(), &replacement));
    let (lazy, worker) = LazySetup::private_services_with_router(router);
    assert_eq!(lazy.declared_tools(), default.declared_tools());
    lazy.dispatcher()
        .bind_external_tool_surface_handle(replacement.clone());
    assert!(Arc::ptr_eq(&original_slot.read().unwrap(), &replacement));
    let tools = lazy.declared_tools();
    let actual = lazy
        .dispatcher()
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<BTreeSet<_>>();
    let expected = json!({
        "prompt_kinds":["approval", "provider_target", "provider_action", "provider_api_key", "provider_browser", "provider_device"],
        "forge_configuration":false, "service_preparation":true, "paired_host":false,
        "tools":actual,
    });
    for adapter in [Adapter::ForgeServices, Adapter::Services] {
        let declared = super::capabilities(3, adapter, false, &tools, Some(&worker)).unwrap();
        assert_eq!(declared, expected);
        assert!(super::capabilities(3, adapter, false, &tools, None).is_err());
        assert!(super::capabilities(3, adapter, true, &tools, Some(&worker)).is_err());
        for version in [1, 2] {
            assert!(super::capabilities(version, adapter, false, &tools, Some(&worker)).is_err());
        }
    }
    for version in [1, 2] {
        assert!(
            super::capabilities(version, Adapter::ForgeReading, false, &tools, Some(&worker))
                .is_err()
        );
    }
    assert!(super::capabilities(2, Adapter::PairedHost, true, &tools, Some(&worker)).is_err());
    assert!(
        tools
            .iter()
            .any(|name| name == crate::service_preparation::TOOLKIT_PREPARE_SERVICES)
    );
    assert!(
        tools
            .iter()
            .all(|name| !name.starts_with("paired_host_") && !name.starts_with("forge_"))
    );
    let args = serde_json::value::to_raw_value(&json!({})).unwrap();
    let dispatcher = lazy.dispatcher();
    for name in [
        "setup_install",
        "paired_host_frontend_submit",
        "paired_host_extension_secret_create",
        "forge_select_host",
    ] {
        assert!(!tools.iter().any(|tool| tool == name));
        assert!(
            dispatcher
                .dispatch(meerkat_core::types::ToolCallView {
                    id: "excluded",
                    name,
                    args: &args
                })
                .await
                .is_err()
        );
    }
    assert!(!lazy.owner().requires_shutdown());
    assert!(
        lazy.allowed_tools()
            .iter()
            .any(|name| name == "setup_install")
    );
    assert!(lazy.installs().is_some());
    let (paired_host, _hosts) = crate::paired_host_tool::PairedHostFrontend::deferred();
    let (terminal, _presenter) = LazySetup::new(paired_host);
    assert!(terminal.installs().is_none());
    // These are the constructors used by main.rs and the service console.
    // Inert acquisition is available in both; it never grants installation.
    for composition in [&terminal, &lazy] {
        for (caller, can_acquire_python, can_acquire_node, can_acquire_prebuilt) in [
            (None, true, true, true),
            (
                Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly),
                false,
                false,
                false,
            ),
            (
                Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                    ["setup_acquire_python"].into_iter().collect(),
                )),
                false,
                true,
                true,
            ),
            (
                Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                    ["setup_acquire_node"].into_iter().collect(),
                )),
                true,
                false,
                true,
            ),
            (
                Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                    ["setup_acquire_node_prebuilt"].into_iter().collect(),
                )),
                true,
                true,
                false,
            ),
        ] {
            let mut options = crate::SessionOptions::default();
            options.build.tool_access_policy = caller;
            let options = composition.session_options(options).unwrap();
            let policy = meerkat_core::ToolExecutionPolicy::resolve(
                options.build.tool_access_policy.unwrap(),
            )
            .unwrap();
            assert_eq!(
                policy.permits_call(
                    "setup_acquire_python",
                    meerkat_core::ToolMutationClass::Mutating
                ),
                can_acquire_python
            );
            assert_eq!(
                policy.permits_call(
                    "setup_acquire_node",
                    meerkat_core::ToolMutationClass::Mutating
                ),
                can_acquire_node
            );
            assert_eq!(
                policy.permits_call(
                    "setup_acquire_node_prebuilt",
                    meerkat_core::ToolMutationClass::Mutating
                ),
                can_acquire_prebuilt
            );
            assert!(policy.permits_call(
                "setup_search_node",
                meerkat_core::ToolMutationClass::ReadOnly
            ));
            assert!(policy.permits_call(
                "setup_inspect_acquired",
                meerkat_core::ToolMutationClass::ReadOnly
            ));
        }
        // Visibility is not native readiness: before the retained handoff is
        // accepted no acquisition tool can enter, and no installation prompt appears.
        let root = tempfile::tempdir().unwrap();
        let (approvals, mut prompts) =
            crate::approval::LocalApprovals::create_paired_host(root.path()).unwrap();
        let mut options = composition
            .session_options(crate::SessionOptions::default())
            .unwrap();
        approvals
            .attach_lazy_setup(&mut options, composition)
            .unwrap();
        let session = meerkat_core::SessionId::new();
        approvals.bind_session(&session).unwrap();
        let context =
            meerkat_core::ToolDispatchContext::default().with_runtime_identity(session, None);
        let admission = options.build.tool_dispatch_admission.unwrap();
        for name in crate::setup::ACQUISITION_TOOLS {
            let args = serde_json::value::to_raw_value(&json!({})).unwrap();
            assert!(
                admission
                    .await_dispatch_admission(
                        meerkat_core::ToolCallView {
                            id: "not-ready",
                            name,
                            args: &args
                        },
                        Some(&context),
                        meerkat_core::LiveBridgeEffectKind::ExternalIo,
                    )
                    .await
                    .is_err()
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(10), prompts.next())
                .await
                .is_err()
        );
    }
    for name in crate::setup::ACQUISITION_TOOLS {
        assert!(
            !LazySetup::reading()
                .allowed_tools()
                .iter()
                .any(|tool| tool == name)
        );
    }
    for name in [
        "setup_install",
        "setup_selected_install",
        "setup_selected_check",
    ] {
        assert!(!terminal.allowed_tools().iter().any(|tool| tool == name));
    }
}

#[test]
fn service_worker_settlement_is_required_before_retirement_ack() {
    for failed in [false, true] {
        let (mut d, mut rx) = driver();
        d.initialize_id = Some("h:init".into());
        d.scope = Some("test-scope".into());
        d.workers = 1;
        retire(&mut d);
        d.settle();
        assert!(!drain(&mut rx).iter().any(|row| row["id"] == "h:retire"));
        d.work(Work::Service(if failed { Err(ended()) } else { Ok(()) }));
        d.settle();
        let rows = drain(&mut rx);
        let reply = rows.iter().find(|row| row["id"] == "h:retire").unwrap();
        if failed {
            assert_eq!(reply["error"]["data"]["code"], "shutdown_unconfirmed");
            assert!(
                rows.iter()
                    .all(|row| row["result"]["entered_work"] != "settled")
            );
            assert!(d.failed);
        } else {
            assert_eq!(reply["result"]["entered_work"], "settled");
            assert!(!d.failed);
        }
    }
}

#[tokio::test]
async fn closed_service_request_fences_completed_input_before_new_send() {
    let (paired_host, _hosts) = crate::paired_host_tool::PairedHostFrontend::deferred();
    let (lazy, _presenter) = LazySetup::new(paired_host);
    let dispatcher = lazy.dispatcher();
    let args = serde_json::value::to_raw_value(&json!({})).unwrap();
    let mut request = Box::pin(dispatcher.dispatch(meerkat_core::types::ToolCallView {
        id: "prepare",
        name: crate::service_preparation::TOOLKIT_PREPARE_SERVICES,
        args: &args,
    }));
    assert!(request.as_mut().now_or_never().is_none());
    drop(request);
    assert!(lazy.owner().requires_shutdown());
    let (mut d, mut rx) = driver();
    active(&mut d);
    d.lazy = Some(Arc::new(lazy));
    d.workers = 1;
    d.work(Work::Sent(
        "h:send".into(),
        TurnCompletion {
            outcome: Ok("completed"),
            uncertain: false,
            settled: true,
        },
        None,
    ));
    let rows = drain(&mut rx);
    let result = rows.iter().find(|row| row["id"] == "h:send").unwrap();
    assert_eq!(result["result"]["completion"], "completed");
    assert_eq!(result["result"]["state"]["can_send"], false);
    assert_eq!(d.last.as_ref().unwrap()["completion"], "completed");
    assert!(d.retired && d.failed && d.closing);
    d.frame(br#"{"jsonrpc":"2.0","id":"h:next","method":"connect/send","params":{"scope":"test-scope","text":"must not enter"}}"#);
    assert!(d.tasks.is_empty());
    assert!(
        drain(&mut rx)
            .iter()
            .any(|row| row["id"] == "h:next" && row.get("error").is_some())
    );
}

fn paired_host_binding(root: &std::path::Path) -> Value {
    let mut value = binding(root);
    value["protocol_version"] = json!(2);
    value["adapter"] = json!("paired_host");
    value["application"]["origin"] = json!("https://paired-host.example");
    value["viewer"]["principal_id"] = json!("paired-device:owner-fixture");
    value["model"] = json!("gpt-6.1-sol");
    value["auth_binding"] = json!({"realm":"stdio-test","binding":"selected"});
    value["paired_host"] = json!({"targets":[{"target_id":"selected","origin":"https://paired-host.example"}],
        "credential":{"header":"x-paired-host-presenter-credential","value":"private-test-only",
            "expires_at_ms":value["viewer"]["expires_at_ms"]}});
    value
}

#[tokio::test]
async fn paired_host_native_bootstrap_is_closed_and_does_not_retain_the_credential_in_binding() {
    let root = tempfile::tempdir().unwrap();
    let original = paired_host_binding(root.path());
    assert!(
        serde_json::from_value::<Initialize>(original.clone())
            .unwrap()
            .valid()
    );
    for (pointer, replacement) in [
        ("/adapter", json!("forge_reading")),
        ("/protocol_version", json!(1)),
        ("/application/origin", json!("https://other.example")),
        (
            "/application/origin",
            json!("https://paired-host.example:443"),
        ),
        ("/viewer/principal_id", json!("operator")),
        ("/viewer/principal_id", json!("paired-device:")),
        ("/model", Value::Null),
        ("/auth_binding", Value::Null),
        ("/paired_host/credential", Value::Null),
        ("/paired_host/credential/header", json!("authorization")),
        (
            "/paired_host/credential/value",
            json!("secret\r\nInjected: value"),
        ),
        ("/paired_host/credential/expires_at_ms", json!(now() + 1)),
        ("/paired_host/targets", json!([])),
        (
            "/paired_host/targets",
            json!([{"target_id":"selected","origin":"https://paired-host.example"},
            {"target_id":"extra","origin":"https://other.example"}]),
        ),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            !serde_json::from_value::<Initialize>(value).is_ok_and(|v| v.valid()),
            "{pointer}"
        );
    }
    for key in ["model", "auth_binding"] {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(key);
        assert!(!serde_json::from_value::<Initialize>(value).unwrap().valid());
    }
    let mut unknown = original.clone();
    unknown["paired_host"]["credential"]["cookie"] = json!("refused");
    assert!(serde_json::from_value::<Initialize>(unknown).is_err());
    let (mut driver, mut rx) = driver();
    driver.frame(
        &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"h:init",
        "method":"connect/initialize","params":original}))
        .unwrap(),
    );
    assert!(driver.opening);
    assert!(
        driver
            .binding
            .as_ref()
            .unwrap()
            .paired_host
            .as_ref()
            .unwrap()
            .credential
            .is_none()
    );
    driver.frame(br#"{"jsonrpc":"2.0","id":"h:retire","method":"connect/retire","params":{"target":{"initialize_request_id":"h:init"},"retirement_id":"logout","reason":"logout"}}"#);
    let work = driver.tasks.next().await.unwrap();
    driver.work(work);
    driver.settle();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert!(
        !serde_json::to_string(&drain(&mut rx))
            .unwrap()
            .contains("private-test-only")
    );
}

#[tokio::test]
async fn paired_host_preparation_declares_exact_installed_tools_and_native_ceiling() {
    use meerkat_core::{ToolExecutionPolicy, ToolMutationClass};
    let root = tempfile::tempdir().unwrap();
    let binding: Initialize = serde_json::from_value(paired_host_binding(root.path())).unwrap();
    let client = crate::paired_host::PairedHostClient::for_presenter(
        "https://paired-host.example",
        "x-paired-host-presenter-credential",
        "private-test-only",
    )
    .unwrap();
    let paired_host = crate::paired_host_tool::PairedHostFrontend::private_preparation(client);
    let _review = paired_host.take_secret_entry_presenter().unwrap();
    let lazy = LazySetup::paired_host_preparation(paired_host);
    let installed = lazy
        .dispatcher()
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();
    let expected = crate::paired_host_tool::PAIRED_HOST_PREPARATION_TOOLS
        .iter()
        .chain(crate::recipe_discovery::RECIPE_TOOLS)
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(installed.len(), 8);
    assert_eq!(installed.iter().cloned().collect::<BTreeSet<_>>(), expected);
    assert_eq!(lazy.declared_tools(), installed);
    let capabilities = super::capabilities(
        binding.protocol_version,
        binding.adapter,
        binding.paired_host.is_some(),
        &lazy.declared_tools(),
        None,
    )
    .unwrap();
    assert_eq!(capabilities["tools"], json!(installed));
    assert_eq!(
        capabilities["prompt_kinds"],
        json!(["approval", "paired_host_notice"])
    );
    assert_eq!(capabilities["forge_configuration"], false);
    assert_eq!(capabilities["service_preparation"], false);
    let mut options = lazy.session_options(SessionOptions::default()).unwrap();
    let artifacts = root.path().join("approvals");
    std::fs::create_dir(&artifacts).unwrap();
    let (approvals, _presenter) = LocalApprovals::create_browser(
        &artifacts,
        ApprovalPrincipalId::new("paired-device:owner-fixture").unwrap(),
        std::time::SystemTime::now() + Duration::from_secs(60),
    )
    .unwrap();
    approvals.attach_lazy_setup(&mut options, &lazy).unwrap();
    let policy = ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap();
    for name in &installed {
        assert!(policy.permits_call(name, ToolMutationClass::ReadOnly));
    }
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    for name in [
        "paired_host_select_host",
        "paired_host_frontend_submit",
        "paired_host_extension_secret_create",
        "paired_host_extension_secret_status",
        "paired_host_extension_secret_cancel",
        "forge_select_host",
        "forge_frontend_prepare",
        "read_public_document",
        "toolkit_prepare_services",
        "frontend_research",
        "read_file",
        "shell",
        "spawn_agent",
        "tool_catalog_search",
        "setup_check",
    ] {
        assert!(!policy.permits(name), "{name}");
        assert!(
            lazy.dispatcher()
                .dispatch(meerkat_core::ToolCallView {
                    id: "excluded",
                    name,
                    args: &args,
                })
                .await
                .is_err(),
            "{name}"
        );
    }
    let legacy: Initialize = serde_json::from_value(super::tests::binding(root.path())).unwrap();
    assert_eq!(
        super::capabilities(
            legacy.protocol_version,
            legacy.adapter,
            legacy.paired_host.is_some(),
            &installed,
            None
        )
        .unwrap(),
        json!({
            "prompt_kinds":["forge_recovery","forge_origin","forge_password","approval"],
            "forge_configuration":true,"service_preparation":false,"paired_host":false,
        })
    );
    assert_eq!(
        super::capabilities(2, Adapter::ForgeReading, true, &installed, None).unwrap(),
        json!({
            "prompt_kinds":["forge_recovery","forge_origin","forge_password","approval",
                "paired_host_target","paired_host_pairing_method","paired_host_pairing_code","paired_host_notice"],
            "forge_configuration":true,"service_preparation":false,"paired_host":true,"tools":installed,
        })
    );
}

#[tokio::test]
async fn paired_host_v2_selects_explicit_targets_without_widening_v1() {
    let root = tempfile::tempdir().unwrap();
    let mut params = binding(root.path());
    params["protocol_version"] = json!(2);
    params["adapter"] = json!("forge_reading");
    params["paired_host"] =
        json!({"targets":[{"target_id":"household","origin":"http://127.0.0.1:23456"}]});
    let (mut driver, mut rx) = driver();
    assert!(driver.state().get("private_notice_pending").is_none());
    driver.frame(
        &serde_json::to_vec(
            &json!({"jsonrpc":"2.0","id":"h:init","method":"connect/initialize","params":params}),
        )
        .unwrap(),
    );
    assert!(
        driver.opening,
        "explicit v2 target bootstrap must reach retained startup"
    );
    assert_eq!(driver.state()["private_notice_pending"], false);
    driver.frame(br#"{"jsonrpc":"2.0","id":"h:retire","method":"connect/retire","params":{"target":{"initialize_request_id":"h:init"},"retirement_id":"logout","reason":"logout"}}"#);
    let work = driver.tasks.next().await.unwrap();
    driver.work(work);
    driver.settle();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert!(
        drain(&mut rx)
            .iter()
            .any(|r| r["id"] == "h:retire" && r["result"]["entered_work"] == "settled")
    );

    let mut legacy = params.clone();
    legacy["protocol_version"] = json!(1);
    assert!(
        serde_json::from_value::<Initialize>(legacy).is_err(),
        "v1 keeps its closed fields"
    );
    for targets in [
        json!([]),
        json!([{"target_id":"x","origin":"http://127.0.0.1:23456"},{"target_id":"x","origin":"http://127.0.0.1:23457"}]),
        json!([{"target_id":"x","origin":"http://127.0.0.1:23456"},{"target_id":"y","origin":"http://127.0.0.1:23456"}]),
        json!([{"target_id":"x","origin":"http://127.0.0.1:23456/path"}]),
        json!([{"target_id":"x","origin":"http://user:password@127.0.0.1:23456"}]),
    ] {
        params["paired_host"]["targets"] = targets;
        assert!(!serde_json::from_value::<Initialize>(params.clone()).is_ok_and(|v| v.valid()));
    }
}

// These controls exercise retained presentation custody, not a paired host.
// The separate frozen-owner qualification covers actual HTTP and secret entry.
async fn control_session(root: &std::path::Path) -> Arc<ConnectSession> {
    let storage = RealmOptions {
        context_root: root.into(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("user")),
    }
    .resolve("stdio-notices")
    .unwrap();
    Arc::new(
        ConnectSession::open_with_config(
            storage,
            meerkat::Config::default(),
            SessionOptions {
                model: meerkat::CreateSessionModelResolutionRequest {
                    model: Some("gpt-5.4".into()),
                    ..Default::default()
                },
                build: meerkat_core::SessionBuildOptions {
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        Arc::new(meerkat_client::TestClient::for_provider(
                            meerkat_core::Provider::OpenAI,
                        )),
                    )),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap(),
    )
}

#[tokio::test]
async fn joined_notices_fence_next_send_and_keep_original_private_custody() {
    let root = tempfile::tempdir().unwrap();
    let session = control_session(root.path()).await;
    let (mut d, mut rx) = driver();
    let mut bootstrap = binding(root.path());
    bootstrap["protocol_version"] = json!(2);
    bootstrap["adapter"] = json!("forge_reading");
    d.binding = Some(serde_json::from_value(bootstrap).unwrap());
    active(&mut d);
    d.session = Some(session.clone());
    let (entries, reviews, presenter) = crate::secret_presenter::channel();
    d.secret_presenter = Some(presenter);
    let digest = crate::paired_host::FrontendChangeDigest::new(&format!(
        "paired-host-change-v1:sha256:{}",
        "a".repeat(64)
    ))
    .unwrap();
    for (sender, path, kind) in [
        (&entries, "/private-entry-slot", "entry-slot"),
        (&reviews, "/private-review-slot", "review-slot"),
    ] {
        sender
            .try_send(crate::secret_presenter::SecretEntryNotice::prepared_review(
                "http://127.0.0.1:23456",
                path,
                &digest,
                kind,
            ))
            .unwrap();
    }
    d.settle();
    assert!(
        d.pending.is_none(),
        "no notice while the native input is active"
    );
    let (_, completion) = session
        .submit("Complete the notice custody control".into())
        .await
        .unwrap();
    let completion = completion.try_wait_with_terminal_outcome().await;
    d.work(Work::Sent(
        "h:send".into(),
        TurnCompletion::observation(completion),
        None,
    ));
    assert!(d.active.is_none());
    assert!(!d.private_notices.is_empty());
    assert_eq!(d.state()["can_send"], false);
    assert_eq!(d.state()["private_notice_pending"], true);
    let original_completion = d.last.clone();
    // This request is processed before the first notice has even been emitted.
    d.frame(br#"{"jsonrpc":"2.0","id":"h:early","method":"connect/send","params":{"scope":"test-scope","text":"must not enter"}}"#);
    assert!(d.tasks.is_empty());
    let initial = drain(&mut rx);
    assert!(
        initial
            .iter()
            .any(|r| r["id"] == "h:early" && r["error"]["data"]["code"] == "busy")
    );
    assert!(
        initial
            .iter()
            .filter(|r| r.get("result").is_some())
            .all(|r| r["result"]["state"]["can_send"] != true)
    );
    assert!(
        !serde_json::to_string(&initial)
            .unwrap()
            .contains("private-entry-slot")
    );
    for (index, expected_path) in ["/private-entry-slot", "/private-review-slot"]
        .iter()
        .enumerate()
    {
        d.settle();
        let prompt = d.pending.as_ref().unwrap();
        assert!(prompt.response.is_none());
        let id = prompt.id.clone();
        let body = prompt.prompt.clone();
        assert_eq!(body["send_request_id"], "h:send");
        assert!(
            body["notice"]["url"]
                .as_str()
                .unwrap()
                .ends_with(expected_path)
        );
        assert_eq!(d.state()["active_send"], Value::Null);
        assert_eq!(d.state()["can_send"], false);
        assert_eq!(d.state()["private_notice_pending"], true);
        // A remount snapshot reads canonical history without replacing the live
        // p: request retained by the private host projection.
        d.frame(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":format!("h:history{index}"),"method":"connect/history","params":{"scope":"test-scope"}})).unwrap());
        let work = d.tasks.next().await.unwrap();
        d.work(work);
        assert_eq!(d.pending.as_ref().unwrap().id, id);
        assert_eq!(d.pending.as_ref().unwrap().prompt, body);
        for answer in [
            json!({"kind":"approval","decision":"approve"}),
            json!({"kind":"paired_host_notice","dismiss":false}),
        ] {
            d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":answer}}));
            assert_eq!(d.pending.as_ref().unwrap().id, id);
        }
        d.frame(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":format!("h:busy{index}"),"method":"connect/send","params":{"scope":"test-scope","text":"must not enter"}})).unwrap());
        assert!(d.tasks.is_empty());
        if index == 0 {
            d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"paired_host_notice","dismiss":true}}}));
        } else {
            d.answer(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"display closed"}}),
            );
        }
        assert!(d.pending.is_none());
        d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"paired_host_notice","dismiss":true}}}));
        let rows = drain(&mut rx);
        assert!(
            rows.iter().any(
                |r| r["id"] == format!("h:busy{index}") && r["error"]["data"]["code"] == "busy"
            )
        );
        assert!(
            rows.iter()
                .any(|r| r["id"] == id && r["error"]["data"]["code"] == "stale_prompt")
        );
        assert!(
            !rows
                .iter()
                .filter(|r| r["method"] != "connect/prompt")
                .any(|r| r.to_string().contains(expected_path))
        );
        assert_eq!(d.last, original_completion);
    }
    assert_eq!(d.state()["can_send"], true);
    assert_eq!(d.state()["private_notice_pending"], false);
    // Cancel on a display-only notice does not cancel an unrelated receipt,
    // even if a future integration were to violate the admission guard.
    d.private_notices.push_back((
        "h:send".into(),
        crate::secret_presenter::SecretEntryNotice::prepared_review(
            "http://127.0.0.1:23456",
            "/private-cancel",
            &digest,
            "review-slot",
        ),
    ));
    d.settle();
    let id = d.pending.as_ref().unwrap().id.clone();
    active(&mut d);
    d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"cancel"}}}));
    assert!(!*d.active.as_ref().unwrap().cancel.borrow());
    d.active.take();
    d.private_notices.push_back((
        "h:send".into(),
        crate::secret_presenter::SecretEntryNotice::prepared_review(
            "http://127.0.0.1:23456",
            "/private-retired",
            &digest,
            "review-slot",
        ),
    ));
    d.settle();
    let id = d.pending.as_ref().unwrap().id.clone();
    d.fence("retired");
    assert!(d.pending.is_none() && d.private_notices.is_empty());
    assert_eq!(d.state()["private_notice_pending"], false);
    d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"paired_host_notice","dismiss":true}}}));
    assert_eq!(d.last, original_completion);
    assert!(
        drain(&mut rx)
            .iter()
            .any(|r| r["id"] == id && r["error"]["data"]["code"] == "binding_retired")
    );
    d.session.take();
    Arc::try_unwrap(session)
        .ok()
        .expect("all retained session work joined")
        .close()
        .await
        .unwrap();
}

#[tokio::test]
async fn pairing_operation_retains_first_poll_and_cannot_publish_after_retirement() {
    use crate::host_selection::{HostSelection, PAIRED_HOST_SELECT_HOST};
    use meerkat_core::types::ToolCallView;
    for preclosed in [false, true] {
        let (host, mut presenter) = HostSelection::deferred();
        let selected = host.clone();
        let mut task = Some(tokio::spawn(async move {
            let args = serde_json::value::to_raw_value(&json!({})).unwrap();
            selected
                .request(ToolCallView {
                    id: "select",
                    name: PAIRED_HOST_SELECT_HOST,
                    args: &args,
                })
                .await
        }));
        let mut prompt = presenter.next().await.unwrap();
        let (mut d, _) = driver();
        if preclosed {
            let task = task.take().unwrap();
            task.abort();
            let _ = task.await;
        } else {
            d.fence("retired");
        }
        let count = AtomicUsize::new(0);
        let result = crate::paired_host_stdio::entered(&mut prompt, &d.bridge, async {
            count.fetch_add(1, Ordering::SeqCst);
            Ok::<_, crate::paired_host::PairedHostError>(())
        })
        .await;
        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(host.captured().is_none());
        drop(prompt);
        if let Some(task) = task {
            assert!(task.await.unwrap().is_err());
        }
    }
    let (host, mut presenter) = HostSelection::deferred();
    let selected = host.clone();
    let task = tokio::spawn(async move {
        let args = serde_json::value::to_raw_value(&json!({})).unwrap();
        selected
            .request(ToolCallView {
                id: "select",
                name: PAIRED_HOST_SELECT_HOST,
                args: &args,
            })
            .await
    });
    let mut prompt = presenter.next().await.unwrap();
    let (d, _) = driver();
    let count = AtomicUsize::new(0);
    let (release, wait) = oneshot::channel();
    {
        let operation = crate::paired_host_stdio::entered(&mut prompt, &d.bridge, async {
            count.fetch_add(1, Ordering::SeqCst);
            wait.await.unwrap();
            Ok::<_, crate::paired_host::PairedHostError>(())
        });
        tokio::pin!(operation);
        assert!(operation.as_mut().now_or_never().is_none());
        d.stop.send_replace(true);
        host.retire().unwrap();
        assert!(operation.as_mut().now_or_never().is_none());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        assert!(operation.await.is_ok());
    }
    assert!(
        prompt
            .handoff(crate::paired_host::PairedHostClient::new("http://127.0.0.1:23456").unwrap())
            .await
            .is_err()
    );
    assert!(task.await.unwrap().is_err());
    assert!(host.captured().is_none());
}

async fn pairing_http_request(socket: &mut tokio::net::TcpStream) -> (String, Value) {
    use tokio::io::AsyncReadExt;
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 8192);
            bytes.push(socket.read_u8().await.unwrap());
        }
        let head = String::from_utf8(bytes).unwrap();
        let length: usize = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_owned)
            })
            .unwrap()
            .parse()
            .unwrap();
        assert!(length < 8192);
        let mut body = vec![0; length];
        socket.read_exact(&mut body).await.unwrap();
        (head, serde_json::from_slice(&body).unwrap())
    })
    .await
    .unwrap()
}

async fn pairing_prompt(
    driver: &mut Driver,
    prompts: &mut mpsc::Receiver<Exchange>,
) -> (String, Value) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                work = driver.tasks.next() => driver.work(work.unwrap()),
                exchange = prompts.recv() => {
                    driver.prompt(exchange.unwrap());
                    let pending = driver.pending.as_ref().unwrap();
                    return (pending.id.clone(), pending.prompt.clone());
                }
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn private_pairing_corrects_locally_and_joins_actual_http_before_retirement() {
    use crate::host_selection::{HostSelection, PAIRED_HOST_SELECT_HOST};
    use meerkat_core::types::ToolCallView;
    use tokio::net::TcpListener;
    // Controlled HTTP acknowledgements exercise transport custody. Actual
    // Paired host enrollment/secret ownership remains a separate qualification.
    for (method, held) in [
        ("request", None),
        ("terminal", None),
        ("request", Some("request")),
        ("request", Some("confirm")),
        ("terminal", Some("terminal")),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let expected_origin = origin.clone();
        let (entered_send, entered_receive) = oneshot::channel();
        let (release_send, release_receive) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut entered = Some(entered_send);
            let mut release = Some(release_receive);
            let expected = if method == "terminal" {
                vec!["terminal"]
            } else if held == Some("request") {
                vec!["request"]
            } else {
                vec!["request", "confirm"]
            };
            let mut posts = Vec::new();
            for operation in expected {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let (head, body) = pairing_http_request(&mut socket).await;
                let lower = head.to_ascii_lowercase();
                let path = format!("/console/pair/{operation}");
                assert!(head.starts_with(&format!("POST {path} HTTP/1.1\r\n")));
                assert!(lower.contains(&format!("\r\norigin: {expected_origin}\r\n")));
                if operation == "request" {
                    assert!(body == json!({"label":"Meerkat Connect Console"}));
                    assert!(!lower.contains("\r\ncookie:"));
                } else {
                    let code = if operation == "terminal" {
                        "ABCDEFGHJKMN"
                    } else {
                        "672491"
                    };
                    assert!(body == json!({"code":code}));
                    assert_eq!(
                        lower.contains("paired_host_pair=fixture-pair"),
                        operation == "confirm"
                    );
                }
                posts.push(path);
                if held == Some(operation) {
                    entered.take().unwrap().send(()).unwrap();
                    release.take().unwrap().await.unwrap();
                }
                let cookie = if operation == "request" {
                    "paired_host_pair=fixture-pair; Path=/console/pair; HttpOnly; SameSite=Strict"
                } else {
                    "paired_host_device=fixture-device; Path=/; HttpOnly; SameSite=Lax"
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nSet-Cookie: {cookie}\r\nConnection: close\r\n\r\n{{\"ok\":true}}").as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(30), listener.accept())
                    .await
                    .is_err(),
                "no additional pairing request"
            );
            posts
        });
        let (host, presenter) = HostSelection::deferred();
        let selected = host.clone();
        let request = tokio::spawn(async move {
            let args = serde_json::value::to_raw_value(&json!({})).unwrap();
            selected
                .request(ToolCallView {
                    id: "select",
                    name: PAIRED_HOST_SELECT_HOST,
                    args: &args,
                })
                .await
        });
        let (out, mut output) = mpsc::channel(OUTPUT_FRAMES);
        let (mut driver, _, mut prompts) = new_driver(out, Arc::new(Semaphore::new(MAX_FRAME * 2)));
        active(&mut driver);
        driver.workers = 1;
        driver.tasks.push(
            paired_host(
                presenter,
                driver.bridge.clone(),
                vec![crate::paired_host_stdio::Target {
                    target_id: "household".into(),
                    origin: origin.clone(),
                }],
            )
            .boxed(),
        );
        let (target_id, target) = pairing_prompt(&mut driver, &mut prompts).await;
        assert_eq!(target["kind"], "paired_host_target");
        assert_eq!(target["custody"], "process_only");
        driver.answer(json!({"jsonrpc":"2.0","id":target_id,"result":{"answer":{"kind":"paired_host_target","target_id":"not-offered"}}}));
        assert_eq!(driver.pending.as_ref().unwrap().id, target_id);
        driver.answer(json!({"jsonrpc":"2.0","id":target_id,"result":{"answer":{"kind":"paired_host_target","target_id":"household"}}}));
        let (method_id, _) = pairing_prompt(&mut driver, &mut prompts).await;
        driver.answer(json!({"jsonrpc":"2.0","id":method_id,"result":{"answer":{"kind":"paired_host_pairing_method","method":method}}}));
        if held != Some("request") {
            let (code_id, code_prompt) = pairing_prompt(&mut driver, &mut prompts).await;
            assert_eq!(code_prompt["kind"], "paired_host_pairing_code");
            driver.answer(json!({"jsonrpc":"2.0","id":code_id,"result":{"answer":{"kind":"paired_host_pairing_code","code":"invalid"}}}));
            assert_eq!(driver.pending.as_ref().unwrap().id, code_id);
            assert_eq!(driver.pending.as_ref().unwrap().prompt, code_prompt);
            let code = if method == "terminal" {
                "ABCD-EFGH-JKMN"
            } else {
                "672491"
            };
            driver.answer(json!({"jsonrpc":"2.0","id":code_id,"result":{"answer":{"kind":"paired_host_pairing_code","code":code}}}));
            driver.answer(json!({"jsonrpc":"2.0","id":code_id,"result":{"answer":{"kind":"paired_host_pairing_code","code":code}}}));
            assert!(
                drain(&mut output)
                    .iter()
                    .any(|row| row["id"] == code_id
                        && row["error"]["data"]["code"] == "stale_prompt")
            );
        }
        let outcome = if held.is_some() {
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::select! {
                    _ = entered_receive => (),
                    work = driver.tasks.next() => panic!("presenter ended before held HTTP: {}", matches!(work, Some(Work::Presenter(_)))),
                }
            }).await.unwrap();
            // The real driver calls this same selection fence from its retained
            // LazySetup. This fixture supplies the owner directly.
            host.retire().unwrap();
            retire(&mut driver);
            assert!(
                tokio::time::timeout(Duration::from_millis(30), driver.tasks.next())
                    .await
                    .is_err()
            );
            driver.settle();
            assert!(!drain(&mut output).iter().any(|row| row["id"] == "h:retire"));
            release_send.send(()).unwrap();
            let work = tokio::time::timeout(Duration::from_secs(5), driver.tasks.next())
                .await
                .unwrap()
                .unwrap();
            driver.work(work);
            tokio::time::timeout(Duration::from_secs(5), request)
                .await
                .unwrap()
                .unwrap()
        } else {
            let mut request = request;
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    tokio::select! {
                        result = &mut request => break result.unwrap(),
                        work = driver.tasks.next() => driver.work(work.unwrap()),
                    }
                }
            })
            .await
            .unwrap()
        };
        assert_eq!(outcome.is_ok(), held.is_none());
        assert_eq!(host.captured().is_some(), held.is_none());
        if !driver.retired {
            host.retire().unwrap();
            retire(&mut driver);
            let work = tokio::time::timeout(Duration::from_secs(5), driver.tasks.next())
                .await
                .unwrap()
                .unwrap();
            driver.work(work);
        }
        // The native send is intentionally not simulated as completed by this
        // transport-only case. Its joined policy is covered by other controls.
        driver.active.take();
        driver.settle();
        assert!(
            drain(&mut output)
                .iter()
                .any(|row| row["id"] == "h:retire" && row["result"]["entered_work"] == "settled")
        );
        let posts = server.await.unwrap();
        assert_eq!(
            posts.len(),
            if method == "request" && held != Some("request") {
                2
            } else {
                1
            }
        );
    }
}

#[tokio::test]
async fn pipelined_retirement_does_not_enter_queued_native_startup() {
    let root = tempfile::tempdir().unwrap();
    let (mut driver, mut rx) = driver();
    driver.frame(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"h:init","method":"connect/initialize","params":binding(root.path())})).unwrap());
    driver.frame(br#"{"jsonrpc":"2.0","id":"h:retire","method":"connect/retire","params":{"target":{"initialize_request_id":"h:init"},"retirement_id":"logout","reason":"logout"}}"#);
    let work = driver.tasks.next().await.unwrap();
    driver.work(work);
    driver.settle();
    assert_eq!(
        std::fs::read_dir(root.path()).unwrap().count(),
        0,
        "no realm or setup artifact entry"
    );
    let rows = drain(&mut rx);
    assert!(
        rows.iter()
            .any(|r| r["id"] == "h:init" && r["error"]["data"]["code"] == "binding_retired")
    );
    assert!(
        rows.iter()
            .any(|r| r["id"] == "h:retire" && r["result"]["entered_work"] == "settled")
    );
}

#[tokio::test]
async fn startup_already_entered_is_retained_through_retirement() {
    let (stop, retired) = watch::channel(false);
    let (release, wait) = oneshot::channel();
    let entered = AtomicUsize::new(0);
    let future = before_entry(retired, async {
        entered.fetch_add(1, Ordering::SeqCst);
        wait.await.unwrap();
        Ok(())
    });
    tokio::pin!(future);
    assert!(future.as_mut().now_or_never().is_none());
    stop.send_replace(true);
    assert!(future.as_mut().now_or_never().is_none());
    assert_eq!(entered.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    assert!(future.await.is_ok());
}

#[test]
fn exact_terminal_witness_controls_next_input_and_retirement_not_last_completion() {
    use meerkat_core::{TurnErrorMetadata, lifecycle::InputId};
    let error = TurnErrorMetadata::runtime_apply_failure("fixture");
    let failed = CompletionOutcome::AbandonedWithError {
        reason: "fixture".into(),
        error: error.clone(),
    };
    for terminal in [
        None,
        Some(InputTerminalOutcome::Superseded {
            superseded_by: InputId::new(),
        }),
        Some(InputTerminalOutcome::Coalesced {
            aggregate_id: InputId::new(),
        }),
    ] {
        let observed = TurnCompletion::observed(&failed, terminal.as_ref());
        assert!(!observed.settled);
        assert!(!observed.uncertain);
        let (mut d, mut rx) = driver();
        active(&mut d);
        d.work(Work::Sent("h:send".into(), observed, Some(Ok(true))));
        retire(&mut d);
        d.settle();
        assert_eq!(d.state()["can_send"], false);
        assert_eq!(
            d.state()["last_completion"]["completion"],
            "abandoned_with_error"
        );
        let rows = drain(&mut rx);
        assert!(
            rows.iter()
                .any(|r| r["id"] == "h:retire"
                    && r["error"]["data"]["code"] == "shutdown_unconfirmed")
        );
        assert!(
            !rows
                .iter()
                .any(|r| r["result"]["entered_work"] == "settled")
        );
    }
    assert!(TurnCompletion::observed(&failed, Some(&InputTerminalOutcome::Consumed)).settled);
    for outcome in [
        CompletionOutcome::CallbackPending {
            tool_use_id: "one".into(),
            tool_name: "tool".into(),
            args: json!({}),
            dedupe_key: None,
        },
        CompletionOutcome::CallbackBatchPending {
            pending_tool_calls: vec![],
        },
        CompletionOutcome::CompletedWithFinalizationFailure {
            error: error.clone(),
        },
        CompletionOutcome::RuntimeTerminated {
            reason: "fixture".into(),
            error,
        },
    ] {
        assert!(!TurnCompletion::observed(&outcome, Some(&InputTerminalOutcome::Consumed)).settled);
    }
    for (native, expected) in [
        (Ok(true), "cancellation_requested"),
        (Ok(false), "not_current"),
        (Err(()), "stop_uncertain"),
    ] {
        let (mut d, mut rx) = driver();
        active(&mut d);
        d.active
            .as_mut()
            .unwrap()
            .cancel_requests
            .push("h:cancel".into());
        d.work(Work::Sent(
            "h:send".into(),
            TurnCompletion::observed(&CompletionOutcome::CompletedWithoutResult, None),
            Some(native),
        ));
        assert_eq!(d.last.unwrap()["completion"], "completed_without_result");
        assert!(!d.failed);
        assert!(!d.uncertain);
        let rows = drain(&mut rx);
        let reply = rows.iter().find(|row| row["id"] == "h:cancel").unwrap();
        if native.is_err() {
            assert_eq!(reply["error"]["data"]["code"], expected);
        } else {
            assert_eq!(
                reply["result"],
                json!({"scope":"test-scope","send_request_id":"h:send","status":expected})
            );
        }
    }
}

#[tokio::test]
async fn failed_required_native_completion_witness_never_acknowledges_settled() {
    let root = tempfile::tempdir().unwrap();
    let storage = RealmOptions {
        context_root: root.path().into(),
        state_root: Some(root.path().join("state")),
        user_config_root: Some(root.path().join("user")),
    }
    .resolve("stdio-witness")
    .unwrap();
    let session = ConnectSession::open_with_config(
        storage,
        meerkat::Config::default(),
        SessionOptions {
            model: meerkat::CreateSessionModelResolutionRequest {
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
            build: meerkat_core::SessionBuildOptions {
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(meerkat_client::TestClient::for_provider(
                        meerkat_core::Provider::OpenAI,
                    )),
                )),
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    let (_, handle) = session
        .submit("Native completion witness control".into())
        .await
        .unwrap();
    let result = handle
        .with_resultful_completion_cleanup(|result| async move {
            assert!(result.is_ok());
            Err(CompletionWaitError::AuthorityUnavailable(
                "fixture required witness failed".into(),
            ))
        })
        .try_wait_with_terminal_outcome()
        .await;
    assert!(matches!(
        result,
        Err(CompletionWaitError::AuthorityUnavailable(_))
    ));
    let (mut d, mut rx) = driver();
    active(&mut d);
    d.work(Work::Sent(
        "h:send".into(),
        TurnCompletion::observation(result),
        None,
    ));
    retire(&mut d);
    d.settle();
    assert!(d.uncertain);
    assert!(d.failed);
    assert_eq!(d.last, None);
    let rows = drain(&mut rx);
    assert!(
        !rows
            .iter()
            .any(|r| r["result"]["entered_work"] == "settled")
    );
    assert!(
        rows.iter()
            .any(|r| r["id"] == "h:retire" && r["error"]["data"]["code"] == "shutdown_unconfirmed")
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn partial_frame_survives_control_wakeups() {
    let (mut peer, input) = tokio::io::duplex(64);
    let mut frames = transport::Frames::new(input);
    peer.write_all(b"{\"secret\":\"split").await.unwrap();
    assert!(frames.next().now_or_never().is_none());
    peer.write_all(b" value\"}\n{\"next\":true}\n")
        .await
        .unwrap();
    assert_eq!(
        frames.next().await.unwrap().unwrap(),
        br#"{"secret":"split value"}"#
    );
    assert_eq!(frames.next().await.unwrap().unwrap(), br#"{"next":true}"#);
    drop(peer);
    assert!(frames.next().await.unwrap().is_none());
}

#[tokio::test]
async fn provisional_burst_reserves_control_capacity_without_retiring_live_binding() {
    let (mut d, mut rx) = driver();
    active(&mut d);
    for i in 0..4096 {
        d.notice(Notice::Conversation(
            "h:send".into(),
            json!({"kind":"text_delta","text":"delta"}),
        ));
        if i % 100 == 0 {
            let _ = drain(&mut rx);
        }
    }
    assert!(!d.retired);
    assert!(!d.failed);
    assert!(!d.output_failed);
    d.work(Work::Sent(
        "h:send".into(),
        TurnCompletion::observed(&CompletionOutcome::CompletedWithoutResult, None),
        None,
    ));
    assert!(drain(&mut rx).iter().any(|r|r["id"]=="h:send"&&r["result"]["completion"]=="completed_without_result"));
}

#[tokio::test]
async fn unread_stdout_does_not_block_retirement_or_eof_custody() {
    let (mut peer, input) = tokio::io::duplex(65536);
    let (output, _unread) = tokio::io::duplex(1);
    // Saturate the finite control queue. The first blocked write cannot keep
    // the reader from observing the remaining frames and EOF.
    let mut bytes = Vec::new();
    for i in 0..200 {
        bytes.extend(format!("{{\"jsonrpc\":\"2.0\",\"id\":\"h:{i}\",\"method\":\"connect/send\",\"params\":{{\"scope\":\"wrong\",\"text\":\"x\"}}}}\n").as_bytes());
    }
    assert!(bytes.len() < 65536);
    peer.write_all(&bytes).await.unwrap();
    let work = tokio::spawn(run_io(input, output));
    drop(peer);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), work)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

/// Called from the actual native-policy fixture, after its admission is spent
/// and its consequence check is held. The driver must wait for that exact
/// dispatch, not equate the local spent marker with physical settlement.
pub(crate) async fn retire_joined_dispatch(
    approvals: Arc<LocalApprovals>,
    dispatch: impl std::future::Future<Output = ()> + std::marker::Send + 'static,
    retired: oneshot::Sender<()>,
) {
    let (mut d, mut rx) = driver();
    d.initialize_id = Some("h:init".into());
    d.scope = Some("test-scope".into());
    d.approvals = Some(approvals);
    d.workers = 1;
    d.tasks.push(
        async move {
            dispatch.await;
            Work::Presenter(false)
        }
        .boxed(),
    );
    retire(&mut d);
    d.settle();
    assert!(
        !drain(&mut rx)
            .iter()
            .any(|r| r["result"]["entered_work"] == "settled")
    );
    retired.send(()).unwrap();
    let work = d.tasks.next().await.unwrap();
    d.work(work);
    d.settle();
    assert!(!d.approvals.as_ref().unwrap().has_uncertain_dispatch());
    assert!(
        drain(&mut rx)
            .iter()
            .any(|r| r["id"] == "h:retire" && r["result"]["entered_work"] == "settled")
    );
}

#[test]
fn oversized_history_refuses_before_serialization_growth_exceeds_limit() {
    let mut output = transport::BoundedFrame {
        bytes: Vec::new(),
        limit: 128,
    };
    assert!(
        serde_json::to_writer(
            &mut output,
            &json!({"messages":["x".repeat(100),"y".repeat(100)]})
        )
        .is_err()
    );
    assert!(output.bytes.len() <= 128);
    assert!(output.bytes.capacity() <= 128);
}

// Synthetic package responses enter the production CheckedInstalls dispatcher.
// This proves host selection/correlation, not actual installation or grants.
struct ConnectorHelpPause {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
    response: Option<Value>,
}
struct ConnectorPackage {
    remote_pauses: std::sync::Mutex<std::collections::VecDeque<ConnectorHelpPause>>,
    help_pauses: std::sync::Mutex<std::collections::VecDeque<ConnectorHelpPause>>,
    result: std::sync::Mutex<Value>,
    check: std::sync::Mutex<Value>,
    help: std::sync::Mutex<Value>,
    epoch: std::sync::atomic::AtomicU64,
    revision: std::sync::atomic::AtomicU64,
    lose_read: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for ConnectorPackage {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Vec::new().into()
    }
    fn execution_binding_epoch(&self, _: &str) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
    async fn dispatch(
        &self,
        _: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        panic!("selected setup context must be preserved")
    }
    async fn dispatch_with_context(
        &self,
        call: meerkat_core::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        assert!(context.origin_session_id().is_some());
        let response = match call.name {
            "setup_check" | "setup_selected_check" => self.check.lock().unwrap().clone(),
            "setup_install" | "setup_selected_install" => {
                let mut result = self.result.lock().unwrap().clone();
                result["scope"] = json!("observed_software_installation_only");
                result.as_object_mut().unwrap().remove("native_connector");
                let family = if call.name == "setup_selected_install" {
                    json!("selected-python")
                } else {
                    self.check.lock().unwrap()["family"].clone()
                };
                json!({"job":{"id":"3".repeat(32),"kind":"install","family":family,"status":"succeeded","result":result,
                    "recovery_request":{"id":"3".repeat(32),"sha256":"d".repeat(64)}},
                    "reconciliation":{"available":true,"observation":null}})
            }
            "setup_help" => {
                let held = self.help_pauses.lock().unwrap().pop_front();
                if let Some(held) = held {
                    held.entered.send(()).unwrap();
                    held.release.await.unwrap();
                    held.response.ok_or_else(|| {
                        meerkat_core::ToolError::execution_failed("lost help reply")
                    })?
                } else {
                    self.help.lock().unwrap().clone()
                }
            }
            "setup_export_recipe" => json!({"job":remote_fixture_job()}),
            "setup_job" | "setup_reconcile"
                if self.result.lock().unwrap()["scope"] == "portable_mcp_configuration_only" =>
            {
                let held = self.remote_pauses.lock().unwrap().pop_front();
                if let Some(held) = held {
                    held.entered.send(()).unwrap();
                    held.release.await.unwrap();
                    held.response.ok_or_else(|| {
                        meerkat_core::ToolError::execution_failed("lost remote reply")
                    })?
                } else {
                    if self.lose_read.swap(false, Ordering::SeqCst) {
                        return Err(meerkat_core::ToolError::execution_failed(
                            "lost remote reply",
                        ));
                    }
                    let observation = remote_fixture_observation(
                        self.result.lock().unwrap().clone(),
                        self.revision.load(Ordering::SeqCst),
                    );
                    if call.name == "setup_job" {
                        json!({"job":remote_fixture_job(),
                        "reconciliation":{"available":true,"observation":observation}})
                    } else {
                        json!({"observation":observation})
                    }
                }
            }
            "setup_reconcile" => {
                if self.lose_read.swap(false, Ordering::SeqCst) {
                    return Err(meerkat_core::ToolError::execution_failed(
                        "lost owner reply",
                    ));
                }
                let mut result = self.result.lock().unwrap().clone();
                result["scope"] = json!("retained_payload_and_launch_binding_only");
                json!({"observation":{"job_id":"3".repeat(32),"revision":self.revision.load(Ordering::SeqCst),
                    "status":"matched","scope":"retained_payload_and_launch_binding_only","result":result}})
            }
            _ => panic!("unexpected package request"),
        };
        Ok(meerkat_core::ToolResult::new(call.id.into(), response.to_string(), false).into())
    }
}
pub(crate) struct ConnectorInstallFixture {
    lazy: Arc<LazySetup>,
    pub(crate) owner: Arc<crate::service_install::CheckedInstalls>,
    package: Arc<ConnectorPackage>,
    realm: meerkat_core::runtime_bootstrap::RealmLocator,
    session: meerkat_core::SessionId,
}
impl ConnectorInstallFixture {
    async fn call(&self, name: &str, args: Value) -> Result<(), meerkat_core::ToolError> {
        use meerkat_core::AgentToolDispatcher;
        let raw = serde_json::value::to_raw_value(&args).unwrap();
        let call = meerkat_core::ToolCallView {
            id: name,
            name,
            args: &raw,
        };
        let context = meerkat_core::ToolDispatchContext::default()
            .with_runtime_identity(self.session.clone(), None);
        self.owner
            .dispatch_with_context(call, &context)
            .await
            .map(|_| ())
    }
    pub(crate) fn selection(
        &self,
        operations: Value,
    ) -> Result<crate::connector_install::InstallationLogin, &'static str> {
        let selected = self
            .owner
            .connector_selection(&self.realm, &self.session, &"3".repeat(32))
            .map_err(|_| "installation_unconfirmed")?;
        crate::connector_install::InstallationLogin::prepare(
            selected,
            serde_json::from_value(operations).unwrap(),
        )
    }
}
pub(crate) fn connector_unprimed_fixture(
    realm: meerkat_core::runtime_bootstrap::RealmLocator,
    session: meerkat_core::SessionId,
    unsupported_entry: bool,
) -> ConnectorInstallFixture {
    let root = "/fixture/installations/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let package = Arc::new(ConnectorPackage {
        remote_pauses: Default::default(),
        help_pauses: Default::default(),
        result: std::sync::Mutex::new(
            json!({"family":"services","profile":"slack","configuration_sha256":"c".repeat(64),
            "contents_sha256":"2".repeat(64),"receipt_sha256":"6".repeat(64),
            "native_connector":{
                "schema_version":1,"module":"meerkat_toolkit_services.adapters.services_mcp",
                "mode":"serve-native","configuration_argument":"--config",
                "member":"meerkat_toolkit_services/adapters/services_mcp.py","member_sha256":"7".repeat(64),
                "wheel":{"filename":"meerkat_toolkit_services-0.1.0-py3-none-any.whl","sha256":"8".repeat(64)},
                "package_sources_sha256":"9".repeat(64)},
            "mcp_path":format!("{root}/ready/mcp.json"),"receipt_path":format!("{root}/ready/installed.json"),
            "mcp_configuration":{"mcpServers":{"toolkit-services-test":{"command":format!("{root}/runtime/bin/python"),
                "args":["-I","-m","meerkat_toolkit_services.adapters.services_mcp","serve","--config",format!("{root}/configuration/selected.json")]}}}}),
        ),
        check: std::sync::Mutex::new(
            json!({"scope":"configuration_only","validation_id":"a".repeat(32),
            "family":"services","profile":"slack","configuration_sha256":"c".repeat(64),"contents_sha256":"2".repeat(64)}),
        ),
        help: std::sync::Mutex::new(
            json!({"family":"services","profile":"slack","operations":[{"id":"auth_test","tool":"slack_auth_test","effect":"read"}]}),
        ),
        epoch: 7.into(),
        revision: 1.into(),
        lose_read: false.into(),
    });
    if unsupported_entry {
        package.result.lock().unwrap()["mcp_configuration"]["mcpServers"]["toolkit-services-test"]
            ["args"][2] = json!("unrelated_module");
        package.result.lock().unwrap()["mcp_configuration"]["mcpServers"]["toolkit-services-test"]
            ["env"] = json!({"SECRET":"private-config-canary"});
    }
    let owner = crate::service_install::CheckedInstalls::fixture(package.clone());
    owner.bind_realm(&realm).unwrap();
    let (lazy, _presenter) = LazySetup::fixture_services(owner.clone());
    ConnectorInstallFixture {
        lazy: Arc::new(lazy),
        owner,
        package,
        realm,
        session,
    }
}

impl ConnectorInstallFixture {
    pub(crate) async fn prime(&self, help_after_check: bool) {
        let request = json!({"family":"services","profile":"slack","operations":["auth_test"],
        "values":{"token_env":"FIXTURE_TOKEN","timeout_seconds":2,"max_response_bytes":65536}});
        self.prime_request(request, help_after_check).await.unwrap();
    }

    async fn prime_request(
        &self,
        request: Value,
        help_after_check: bool,
    ) -> Result<(), meerkat_core::ToolError> {
        use meerkat_core::AgentToolDispatcher;
        let fixture = self;
        let help = json!({"family":request["family"],"profile":request["profile"]});
        fixture.call("setup_help", help.clone()).await?;
        fixture.call("setup_check", request.clone()).await?;
        let mut install = request;
        install["validation_id"] = json!("a".repeat(32));
        install["configuration_sha256"] = json!("c".repeat(64));
        let args = serde_json::value::to_raw_value(&install).unwrap();
        let call = meerkat_core::ToolCallView {
            id: "install",
            name: "setup_install",
            args: &args,
        };
        let held = fixture.owner.retain_action(call, &fixture.session)?;
        fixture.owner.arm_action(held)?;
        let context = meerkat_core::ToolDispatchContext::default()
            .with_runtime_identity(fixture.session.clone(), None);
        fixture.owner.dispatch_with_context(call, &context).await?;
        if help_after_check {
            fixture.call("setup_help", help).await?;
        }
        fixture
            .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
            .await
    }
}

#[tokio::test]
async fn agno_native_entry_uses_the_current_salesforce_selection_before_login() {
    // Inert publisher/installer replies exercise the real checked-install join.
    // No account observation, credential, login, grant or provider call is made.
    for mode in [
        "valid",
        "absent",
        "other-profile",
        "missing-profiles",
        "extra-profile",
        "duplicate-profile",
        "wrong-mode",
        "wrong-module",
        "wrong-member",
        "wrong-wheel",
        "member-hash",
        "wheel-hash",
        "source-hash",
        "changed-profile",
    ] {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions {
            context_root: root.path().to_owned(),
            state_root: Some(root.path().join("state")),
            user_config_root: Some(root.path().join("home")),
        }
        .resolve("agno-entry")
        .unwrap();
        let fixture =
            connector_unprimed_fixture(storage.locator, meerkat_core::SessionId::new(), false);
        let profile = if mode == "other-profile" {
            "tavily"
        } else {
            "salesforce"
        };
        let raw_operation = if mode == "other-profile" {
            "web_search_using_tavily"
        } else {
            "get_record"
        };
        {
            let mut check = fixture.package.check.lock().unwrap();
            check["family"] = json!("agno");
            check["profile"] = json!(profile);
            let mut help = fixture.package.help.lock().unwrap();
            *help = json!({"family":"agno","profile":profile,
                "operations":[{"id":raw_operation,"tool":raw_operation,"effect":"read"}]});
            let mut result = fixture.package.result.lock().unwrap();
            result["family"] = json!("agno");
            result["profile"] = json!(profile);
            result["mcp_configuration"]["mcpServers"]["toolkit-services-test"]["args"][2] =
                json!("meerkat_toolkit_agno.agno_mcp");
            result["native_connector"] = json!({"schema_version":1,
                "module":"meerkat_toolkit_agno.agno_mcp","mode":"serve-native","configuration_argument":"--config",
                "profiles":["salesforce"],"member":"meerkat_toolkit_agno/agno_mcp.py","member_sha256":"7".repeat(64),
                "wheel":{"filename":"meerkat_toolkit_agno-0.1.0-py3-none-any.whl","sha256":"8".repeat(64)},
                "package_sources_sha256":"9".repeat(64)});
            match mode {
                "absent" => {
                    result.as_object_mut().unwrap().remove("native_connector");
                }
                "missing-profiles" => {
                    result["native_connector"]
                        .as_object_mut()
                        .unwrap()
                        .remove("profiles");
                }
                "extra-profile" => {
                    result["native_connector"]["profiles"] = json!(["salesforce", "tavily"])
                }
                "duplicate-profile" => {
                    result["native_connector"]["profiles"] = json!(["salesforce", "salesforce"])
                }
                "wrong-mode" => result["native_connector"]["mode"] = json!("serve"),
                "wrong-module" => result["native_connector"]["module"] = json!("another.module"),
                "wrong-member" => result["native_connector"]["member"] = json!("another/member.py"),
                "wrong-wheel" => {
                    result["native_connector"]["wheel"]["filename"] =
                        json!("meerkat_toolkit_services-0.1.0-py3-none-any.whl")
                }
                "member-hash" => result["native_connector"]["member_sha256"] = json!("invalid"),
                "wheel-hash" => result["native_connector"]["wheel"]["sha256"] = json!("invalid"),
                "source-hash" => {
                    result["native_connector"]["package_sources_sha256"] = json!("invalid")
                }
                _ => {}
            }
        }
        let values = if profile == "salesforce" {
            json!({"api_key_env":"DO_NOT_RESOLVE","timeout":3,"instance_url":"https://selected.my.salesforce.com"})
        } else {
            json!({"api_key_env":"DO_NOT_RESOLVE","timeout":3})
        };
        let result = fixture
            .prime_request(
                json!({"family":"agno","profile":profile,
            "operations":[raw_operation],"values":values}),
                true,
            )
            .await;
        if !matches!(mode, "valid" | "absent" | "changed-profile") {
            assert!(result.is_err(), "{mode}");
            assert!(
                fixture
                    .owner
                    .connector_selection(&fixture.realm, &fixture.session, &"3".repeat(32))
                    .is_err(),
                "{mode}"
            );
            continue;
        }
        result.unwrap();
        let selected = fixture
            .owner
            .connector_selection(&fixture.realm, &fixture.session, &"3".repeat(32))
            .ok()
            .expect("selected installed profile");
        let operations = json!({"get_record":{"required_scopes":["api"],"destination_origin":"https://selected.my.salesforce.com"}});
        if mode == "absent" {
            assert!(selected.native_connector.is_none());
            assert_eq!(
                fixture.selection(operations).err(),
                Some("unsupported_native_entry")
            );
            continue;
        }
        assert!(fixture.selection(operations).is_ok());
        let original = selected.configuration.clone();
        let projected = crate::connector_install::native_launch_configuration(
            &selected.installed,
            &original,
            selected.native_connector.as_ref().unwrap(),
        )
        .unwrap();
        let projected = serde_json::to_value(projected).unwrap();
        assert_eq!(projected["args"][2], "meerkat_toolkit_agno.agno_mcp");
        assert_eq!(projected["args"][3], "serve-native");
        assert_eq!(
            original["mcpServers"]["toolkit-services-test"]["args"][3],
            "serve"
        );
        assert!(
            fixture
                .selection(json!({"unselected_raw":{"required_scopes":["api"],
            "destination_origin":"https://selected.my.salesforce.com"}}))
                .is_err()
        );
        if mode == "changed-profile" {
            fixture.package.result.lock().unwrap()["profile"] = json!("tavily");
            assert!(
                fixture
                    .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
                    .await
                    .is_err()
            );
            assert!(selected.check_current().is_err());
        }
    }
}

async fn connector_installed_fixture(
    realm: meerkat_core::runtime_bootstrap::RealmLocator,
    session: meerkat_core::SessionId,
    unsupported_entry: bool,
    help_after_check: bool,
) -> ConnectorInstallFixture {
    let fixture = connector_unprimed_fixture(realm, session, unsupported_entry);
    fixture.prime(help_after_check).await;
    fixture
}
// The existing synthetic package owner supplies a Selected response. Actual
// package validation is covered in toolkit-package; this controls consumer custody.
pub(crate) async fn selected_public_fixture(
    root: &std::path::Path,
    declaration: Option<Value>,
) -> ConnectorInstallFixture {
    selected_public_fixture_form(root, declaration, false, meerkat_core::SessionId::new()).await
}

// Reuse the checked-package fixture in the actual admitted presenter's session.
pub(crate) async fn selected_public_fixture_for_session(
    root: &std::path::Path,
    declaration: Value,
    session: meerkat_core::SessionId,
) -> ConnectorInstallFixture {
    selected_public_fixture_form(root, Some(declaration), false, session).await
}

// Canonical software metadata reaches the actual CheckedInstalls reader. The
// installation process/receipt remain mechanical fixtures, not artifact or
// native schema observations.
pub(crate) async fn selected_software_fixture(root: &std::path::Path) -> ConnectorInstallFixture {
    selected_public_fixture_form(root, None, true, meerkat_core::SessionId::new()).await
}

async fn selected_public_fixture_form(
    root: &std::path::Path,
    mut declaration: Option<Value>,
    software: bool,
    session: meerkat_core::SessionId,
) -> ConnectorInstallFixture {
    selected_public_fixture_at(
        root,
        declaration.take(),
        software,
        session,
        "selected-owner",
        json!({"command":"/bin/true","args":["not-executed"]}),
    )
    .await
}

// The native continuation control needs the same checked source in its actual
// realm and an MCP peer that can initialize when persisted configuration opens.
// This remains a synthetic package owner, not a real package installation.
pub(crate) async fn selected_public_fixture_at(
    root: &std::path::Path,
    mut declaration: Option<Value>,
    software: bool,
    session: meerkat_core::SessionId,
    realm: &str,
    launch: Value,
) -> ConnectorInstallFixture {
    use meerkat_core::AgentToolDispatcher;
    use sha2::{Digest, Sha256};
    let storage = crate::RealmOptions {
        context_root: root.to_owned(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    }
    .resolve(realm)
    .unwrap();
    let fixture = connector_unprimed_fixture(storage.locator, session, false);
    let destination = root.join("installations").join("a".repeat(32));
    std::fs::create_dir_all(destination.join("inputs")).unwrap();
    std::fs::create_dir_all(destination.join("ready")).unwrap();
    let (package, activation, plan) = if software {
        let package =
            include_bytes!("../../toolkit-package/tests/fixtures/python-install/package.json")
                .to_vec();
        let typed: toolkit_package::IntegrationPackage = serde_json::from_slice(&package).unwrap();
        let value = json!({"schema_version":1,"document_kind":"local_software_declaration",
            "package":typed.identity,"package_content_digest":typed.content_digest().unwrap(),"strategy":"python-local",
            "components":[{"id":"server","version":"0.0.0-fixture","runtime":"python","source":{
                "kind":"artifact","location":"https://example.invalid/server-1.0-py3-none-any.whl","content_digest":"1".repeat(64),
                "dependencies":{"kind":"pinned_closure","artifacts":{"requirements":"2".repeat(64)}},
                "entrypoint":{"executable":"server.py","arguments":[]}}}],
            "exports":[{"role":"service_connector","id":"service","components":["server"],"authentication":{"kind":"none"},
                "operations":[{"id":"declared/read","schema_digest":"5".repeat(64),"declared_effect":"read",
                    "required_authority":["read"],"resources":{"kind":"connection_wide"},
                    "result_limits":{"maximum_bytes":4096,"maximum_items":10}}]}],
            "portable_mcp_entries":[{"name":"outside-server","component":"server","operation_tools":{"declared/read":"actual_read"}}]});
        let bytes = serde_json::to_vec_pretty(&value).unwrap();
        let proof = serde_json::from_slice::<toolkit_package::LocalSoftwareDeclaration>(&bytes)
            .unwrap()
            .validate(&typed)
            .unwrap();
        let plan = serde_json::to_value(
            toolkit_package::PythonInstallPlan::from_declaration(
                &typed,
                &proof,
                &toolkit_package::Id::new("server").unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        declaration = Some(plan["declared_service"].clone());
        (package, bytes, Some(plan))
    } else {
        (
            br#"{"fixture":"exact retained package input"}"#.to_vec(),
            serde_json::to_vec(
                &json!({"fixture":"declared selection", "declared_service":declaration}),
            )
            .unwrap(),
            None,
        )
    };
    let package_sha = format!("{:x}", Sha256::digest(&package));
    let activation_sha = format!("{:x}", Sha256::digest(&activation));
    let selected_name = if software {
        "declaration.json"
    } else {
        "activation.json"
    };
    std::fs::write(destination.join("inputs/package.json"), &package).unwrap();
    std::fs::write(destination.join("inputs").join(selected_name), &activation).unwrap();
    let document = json!({"mcpServers":{"outside-server":launch}});
    let receipt = br#"{"fixture":"software owner receipt"}"#;
    std::fs::write(
        destination.join("ready/mcp.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
    std::fs::write(destination.join("ready/installed.json"), receipt).unwrap();
    let mut check = json!({"scope":"selected_software_inputs_only", "validation_id":"a".repeat(32),
        "effect_sha256":"b".repeat(64),"profile":"python","component":"server",
        "selection_sha256":activation_sha,"manifest_digest":"c".repeat(64),
        "files":{"package.json":package_sha,"activation.json":activation_sha},
        "destination":destination,"mcp_name":"outside-server",
        "acquired_source":{"fixture":"inert inspected source evidence"}});
    let mut result = json!({"selection_kind":"python","component":"server","selection_sha256":activation_sha,
        "manifest_digest":"c".repeat(64),"effect_sha256":"b".repeat(64),
        "mcp_path":destination.join("ready/mcp.json"),"receipt_path":destination.join("ready/installed.json"),
        "receipt_sha256":format!("{:x}",Sha256::digest(receipt)),"mcp_configuration":document});
    if let Some(plan) = plan {
        for value in [&mut check, &mut result] {
            value.as_object_mut().unwrap().remove("manifest_digest");
            value["document_kind"] = plan["document_kind"].clone();
            value["declaration_digest"] = plan["declaration_digest"].clone();
        }
        check["files"]
            .as_object_mut()
            .unwrap()
            .remove("activation.json");
        check["files"]["declaration.json"] = json!(activation_sha);
    }
    if let Some(declaration) = declaration {
        check["declared_service"] = declaration.clone();
        result["declared_service"] = declaration;
    }
    *fixture.package.check.lock().unwrap() = check;
    *fixture.package.result.lock().unwrap() = result;
    let mut request = json!({"profile":"python","staged_id":"outside","component":"server"});
    if software {
        request["document_kind"] = json!("local_software_declaration");
    }
    fixture.call("setup_selected_check", request).await.unwrap();
    let args = serde_json::value::to_raw_value(
        &json!({"validation_id":"a".repeat(32),"effect_sha256":"b".repeat(64)}),
    )
    .unwrap();
    let call = meerkat_core::ToolCallView {
        id: "outside-install",
        name: "setup_selected_install",
        args: &args,
    };
    let held = fixture.owner.retain_action(call, &fixture.session).unwrap();
    fixture.owner.arm_action(held).unwrap();
    let context = meerkat_core::ToolDispatchContext::default()
        .with_runtime_identity(fixture.session.clone(), None);
    fixture
        .owner
        .dispatch_with_context(call, &context)
        .await
        .unwrap();
    fixture
        .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
        .await
        .unwrap();
    fixture
}

impl ConnectorInstallFixture {
    pub(crate) fn raw_selection(
        &self,
    ) -> Result<
        crate::service_install::InstalledConnectorSelection,
        crate::service_install::ConnectorSelectionError,
    > {
        self.owner
            .connector_selection(&self.realm, &self.session, &"3".repeat(32))
    }
}

#[tokio::test]
async fn selected_declarations_use_the_exact_owner_without_library_help_or_invented_credentials() {
    for mode in ["legacy", "none", "binding"] {
        let root = tempfile::tempdir().unwrap();
        let declaration = match mode {
            "legacy" => None,
            "none" => Some(json!({"authentication":{"kind":"none"},
                "operation_tools":{"declared/read":"actual_read"}})),
            _ => Some(
                json!({"authentication":{"kind":"binding","owner":"process","requirement":"key", "credential_audience":"service"},
                "operation_tools":{"declared/read":"actual_read"}}),
            ),
        };
        let fixture = selected_public_fixture(root.path(), declaration).await;
        assert!(
            fixture
                .owner
                .installed_selection(&fixture.realm, &fixture.session, &"3".repeat(32))
                .unwrap()
                .matched
        );
        if mode == "legacy" {
            assert!(fixture.raw_selection().is_err());
            continue;
        }
        let selected = fixture.raw_selection().ok().unwrap();
        assert_eq!(selected.no_auth(), mode == "none");
        assert!(
            !selected.no_credential_references,
            "never invent a library credential observation"
        );
        assert!(selected.native_connector.is_none());
        assert_eq!(
            selected.raw_operations,
            ["actual_read".to_owned()].into_iter().collect()
        );
        assert_eq!(
            selected.operation_basis()["kind"],
            "selected_manifest_declarations"
        );
        assert!(selected.selected_inputs().is_some());
        // A library help response cannot replace this selected manifest basis.
        fixture
            .call("setup_help", json!({"family":"services","profile":"slack"}))
            .await
            .unwrap();
        assert!(selected.check_current().is_ok());
    }
}

#[tokio::test]
async fn selected_auth_or_raw_declaration_change_invalidates_previous_current_selection() {
    for mode in [
        "removed",
        "authentication",
        "raw-name",
        "epoch",
        "retire",
        "recheck",
    ] {
        let root = tempfile::tempdir().unwrap();
        let fixture = selected_public_fixture(
            root.path(),
            Some(json!({"authentication":{"kind":"none"},
            "operation_tools":{"declared/read":"actual_read"}})),
        )
        .await;
        let selected = fixture.raw_selection().ok().unwrap();
        match mode {
            "epoch" => {
                fixture.package.epoch.fetch_add(1, Ordering::SeqCst);
            }
            "retire" => fixture.owner.retire(),
            "recheck" => {
                fixture.package.check.lock().unwrap()["declared_service"]["authentication"] = json!({
                    "kind":"binding","owner":"process","requirement":"key","credential_audience":"service"});
                assert!(
                    fixture
                        .call(
                            "setup_selected_check",
                            json!({"profile":"python","staged_id":"outside","component":"server"})
                        )
                        .await
                        .is_err()
                );
            }
            _ => {
                {
                    let mut result = fixture.package.result.lock().unwrap();
                    match mode {
                        "removed" => {
                            result.as_object_mut().unwrap().remove("declared_service");
                        }
                        "authentication" => {
                            result["declared_service"]["authentication"] = json!({
                            "kind":"binding","owner":"process","requirement":"key","credential_audience":"service"})
                        }
                        _ => {
                            result["declared_service"]["operation_tools"]["declared/read"] =
                                json!("other_raw")
                        }
                    }
                }
                assert!(
                    fixture
                        .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
                        .await
                        .is_err()
                );
            }
        }
        assert!(selected.check_current().is_err(), "{mode}");
    }
}

pub(crate) fn connector_operations() -> Value {
    json!({"slack_auth_test":{"required_scopes":["openid"],"destination_origin":"https://service.example"}})
}

#[tokio::test]
async fn connector_installed_job_selection_refuses_substitution_before_native_login() {
    for mode in [
        "job",
        "session",
        "realm",
        "raw-operation",
        "metadata",
        "missing-metadata",
        "proposal",
        "epoch",
        "configuration",
        "lost-reply",
        "caller-config",
        "unsupported-entry",
        "unbound-help",
        "absent-native-capability",
        "wrong-native-member",
        "wrong-native-mode",
        "invalid-native-hash",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (startup, _auth, control) = connector_fixture(root.path(), "http://127.0.0.1:1").await;
        let realm = startup.storage.locator.clone();
        let (mut d, mut rx) = driver();
        d.initialize_id = Some("h:init".into());
        d.scope = Some("test-scope".into());
        d.binding = Some(serde_json::from_value(generic_service_binding(root.path())).unwrap());
        d.session = Some(connector_native_session(startup).await);
        d.connector = Some(control);
        let session = if mode == "session" {
            meerkat_core::SessionId::new()
        } else {
            d.session.as_ref().unwrap().id().clone()
        };
        let installed = connector_installed_fixture(
            realm,
            session,
            mode == "unsupported-entry",
            mode != "unbound-help",
        )
        .await;
        d.lazy = Some(installed.lazy.clone());
        let selected = installed.selection(connector_operations()).ok();
        assert_eq!(
            selected.is_none(),
            matches!(mode, "unsupported-entry" | "unbound-help")
        );
        let mut request = connector_start("test-scope", "http://127.0.0.1:1");
        match mode {
            "job" => request["job_id"] = json!("4".repeat(32)),
            "realm" => {
                d.binding.as_mut().unwrap().realm.state_root =
                    root.path().join("other-state").display().to_string()
            }
            "raw-operation" => {
                request["operations"] = json!({"invented_tool":{"required_scopes":["openid"],"destination_origin":"https://service.example"}})
            }
            "metadata" => {
                installed.package.help.lock().unwrap()["operations"][0]["tool"] =
                    json!("different_actual_tool");
                installed
                    .call("setup_help", json!({"family":"services","profile":"slack"}))
                    .await
                    .unwrap();
                assert!(selected.as_ref().unwrap().check_current().is_err());
            }
            "missing-metadata" => {
                installed.package.help.lock().unwrap()["operations"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("tool");
                installed
                    .call("setup_help", json!({"family":"services","profile":"slack"}))
                    .await
                    .unwrap();
                assert!(selected.as_ref().unwrap().check_current().is_err());
            }
            "proposal" => {
                assert!(installed.call("setup_check",json!({"family":"services","profile":"slack",
                    "operations":["different_operation"],"values":{"token_env":"DIFFERENT_REFERENCE"}})).await.is_err());
                assert!(selected.as_ref().unwrap().check_current().is_err());
            }
            "epoch" => {
                installed.package.epoch.fetch_add(1, Ordering::SeqCst);
            }
            "configuration" => {
                installed.package.result.lock().unwrap()["mcp_configuration"]["mcpServers"]["toolkit-services-test"]
                    ["env"] = json!({"SECRET":"private-config-canary"});
                installed.package.revision.store(2, Ordering::SeqCst);
                assert!(
                    installed
                        .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
                        .await
                        .is_err()
                );
                assert!(selected.as_ref().unwrap().check_current().is_err());
            }
            "lost-reply" => {
                installed.package.lose_read.store(true, Ordering::SeqCst);
                assert!(
                    installed
                        .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
                        .await
                        .is_err()
                );
                assert!(selected.as_ref().unwrap().check_current().is_err());
            }
            "absent-native-capability"
            | "wrong-native-member"
            | "wrong-native-mode"
            | "invalid-native-hash" => {
                {
                    let mut result = installed.package.result.lock().unwrap();
                    match mode {
                        "absent-native-capability" => {
                            result.as_object_mut().unwrap().remove("native_connector");
                        }
                        "wrong-native-member" => {
                            result["native_connector"]["member"] = json!("other/member.py")
                        }
                        "wrong-native-mode" => result["native_connector"]["mode"] = json!("serve"),
                        _ => result["native_connector"]["member_sha256"] = json!("invalid"),
                    }
                }
                installed.package.revision.store(2, Ordering::SeqCst);
                let result = installed
                    .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
                    .await;
                assert_eq!(result.is_ok(), mode == "absent-native-capability");
                assert!(selected.as_ref().unwrap().check_current().is_err());
                if mode == "absent-native-capability" {
                    // Ordinary install remains matched; only native derivation is absent.
                    assert!(
                        installed
                            .owner
                            .installed_selection(
                                &installed.realm,
                                &installed.session,
                                &"3".repeat(32)
                            )
                            .unwrap()
                            .matched
                    );
                }
            }
            "caller-config" => {
                request["mcp_configuration"] =
                    json!({"command":"caller-chosen","env":{"SECRET":"private-config-canary"}})
            }
            _ => {}
        }
        connector_request(&mut d, "h:start", "start", request);
        assert!(d.tasks.is_empty(), "{mode} entered native discovery");
        assert_eq!(d.workers, 0);
        assert!(!d.connector_busy);
        let rows = drain(&mut rx);
        assert!(
            rows.iter()
                .any(|r| r["id"] == "h:start" && r.get("error").is_some())
        );
        assert!(
            !serde_json::to_string(&rows)
                .unwrap()
                .contains("private-config-canary")
        );
        if matches!(mode, "missing-metadata" | "unbound-help") {
            let error = &rows.iter().find(|r| r["id"] == "h:start").unwrap()["error"]["data"];
            assert_eq!(error["code"], "installation_metadata_required");
            assert_eq!(error["next_action"]["tool"], "setup_help");
            assert_eq!(
                error["next_action"]["arguments"],
                json!({"family":"services","profile":"slack"})
            );
            assert_eq!(
                error["next_action"]["then"],
                json!({"method":"connect/connector/login/start","job_id":"3".repeat(32)})
            );
            installed.package.help.lock().unwrap()["operations"][0]["tool"] =
                json!("slack_auth_test");
            installed
                .call("setup_help", json!({"family":"services","profile":"slack"}))
                .await
                .unwrap();
            assert!(installed.selection(connector_operations()).is_ok());
        }
        if matches!(mode, "unsupported-entry" | "absent-native-capability") {
            let error = &rows.iter().find(|r| r["id"] == "h:start").unwrap()["error"]["data"];
            assert_eq!(error["code"], "unsupported_native_entry");
            assert_eq!(error["next_action"]["job_id"], "3".repeat(32));
            assert!(
                error["next_action"]["instruction"]
                    .as_str()
                    .unwrap()
                    .contains("install a compatible integration")
            );
        }
        if mode == "lost-reply" {
            installed
                .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
                .await
                .unwrap();
            assert!(selected.as_ref().unwrap().check_current().is_ok());
            assert!(installed.selection(connector_operations()).is_ok());
        }
        d.connector.take();
        d.lazy.take();
        Arc::try_unwrap(d.session.take().unwrap())
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn connector_help_publication_belongs_to_the_current_observation() {
    for mode in [
        "conflicting-check",
        "newer-failed",
        "newer-changed",
        "newer-identical",
    ] {
        let root = tempfile::tempdir().unwrap();
        let (startup, _auth, control) = connector_fixture(root.path(), "http://127.0.0.1:1").await;
        let realm = startup.storage.locator.clone();
        let (mut d, mut rx) = driver();
        d.initialize_id = Some("h:init".into());
        d.scope = Some("test-scope".into());
        d.binding = Some(serde_json::from_value(generic_service_binding(root.path())).unwrap());
        d.session = Some(connector_native_session(startup).await);
        d.connector = Some(control);
        let installed = connector_installed_fixture(
            realm,
            d.session.as_ref().unwrap().id().clone(),
            false,
            true,
        )
        .await;
        d.lazy = Some(installed.lazy.clone());
        let selected = installed.selection(connector_operations()).unwrap();
        let original_help = installed.package.help.lock().unwrap().clone();
        let (entered, mut entered_rx) = oneshot::channel();
        let (release, released) = oneshot::channel();
        installed
            .package
            .help_pauses
            .lock()
            .unwrap()
            .push_back(ConnectorHelpPause {
                entered,
                release: released,
                response: Some(original_help.clone()),
            });
        let help_request = json!({"family":"services","profile":"slack"});
        let mut earlier = Box::pin(installed.call("setup_help", help_request.clone()));
        assert!(futures_util::poll!(&mut earlier).is_pending());
        entered_rx.try_recv().unwrap(); // A owns its slot and is held in the actual dispatcher.
        if mode == "conflicting-check" {
            assert!(
                installed
                    .call(
                        "setup_check",
                        json!({"family":"services","profile":"slack",
                "operations":["different_operation"],"values":{"token_env":"DIFFERENT_REFERENCE"}})
                    )
                    .await
                    .is_err()
            );
        } else if mode == "newer-failed" {
            let (entered, mut entered_rx) = oneshot::channel();
            let (release, released) = oneshot::channel();
            installed
                .package
                .help_pauses
                .lock()
                .unwrap()
                .push_back(ConnectorHelpPause {
                    entered,
                    release: released,
                    response: None,
                });
            let mut later = Box::pin(installed.call("setup_help", help_request.clone()));
            assert!(futures_util::poll!(&mut later).is_pending());
            entered_rx.try_recv().unwrap();
            release.send(()).unwrap();
            assert!(later.await.is_err());
        } else {
            if mode == "newer-changed" {
                installed.package.help.lock().unwrap()["operations"][0]["tool"] =
                    json!("new_selected_tool");
            }
            installed
                .call("setup_help", help_request.clone())
                .await
                .unwrap();
        }
        release.send(()).unwrap();
        assert!(earlier.await.is_err(), "late A must not publish: {mode}");
        // The exact old job read can restore software matched=true, but cannot
        // restore metadata invalidated by a newer help or conflicting proposal.
        installed.package.revision.store(2, Ordering::SeqCst);
        installed
            .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
            .await
            .unwrap();
        if mode == "newer-identical" {
            assert!(selected.check_current().is_ok());
            assert!(installed.selection(connector_operations()).is_ok());
        } else {
            assert!(selected.check_current().is_err());
            if mode == "newer-changed" {
                let current = installed
                    .owner
                    .connector_selection(&installed.realm, &installed.session, &"3".repeat(32))
                    .unwrap_or_else(|_| panic!("current B must remain"));
                assert_eq!(
                    current.raw_operations,
                    ["new_selected_tool".to_owned()].into_iter().collect()
                );
            }
            connector_request(
                &mut d,
                "h:start",
                "start",
                connector_start("test-scope", "http://127.0.0.1:1"),
            );
            assert!(d.tasks.is_empty());
            assert!(!d.connector_busy);
            assert_eq!(d.workers, 0);
            let rows = drain(&mut rx);
            let error = &rows.iter().find(|r| r["id"] == "h:start").unwrap()["error"]["data"];
            assert_eq!(
                error["code"],
                if mode == "newer-changed" {
                    "invalid_operation_binding"
                } else {
                    "installation_metadata_required"
                }
            );
            if mode != "newer-changed" {
                assert_eq!(error["next_action"]["tool"], "setup_help");
            }
            *installed.package.help.lock().unwrap() = original_help;
            installed.call("setup_help", help_request).await.unwrap();
            assert!(installed.selection(connector_operations()).is_ok());
        }
        d.connector.take();
        d.lazy.take();
        Arc::try_unwrap(d.session.take().unwrap())
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
    }
}

// Synthetic owner-envelope controls reuse the actual CheckedInstalls path.
// These are not claims that Slack is public, that a package ran, or that a
// native grant exists. No-auth and check evidence are independent owner facts.
#[tokio::test]
async fn public_auth_declaration_does_not_infer_absent_or_nonempty_credential_evidence() {
    for references in [
        None,
        Some(json!([])),
        Some(Value::Null),
        Some(json!({})),
        Some(json!([{"field":"token_env","reference":"SELECTED_TOKEN"}])),
    ] {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions {
            context_root: root.path().to_owned(),
            state_root: Some(root.path().join("state")),
            user_config_root: Some(root.path().join("user")),
        }
        .resolve("public-metadata")
        .unwrap();
        let fixture =
            connector_unprimed_fixture(storage.locator, meerkat_core::SessionId::new(), false);
        {
            let mut help = fixture.package.help.lock().unwrap();
            help["authentication"] = json!({"kind":"none"});
            help["fields"] = json!([]);
        }
        if let Some(value) = &references {
            fixture.package.check.lock().unwrap()["credential_environment_references"] =
                value.clone();
        }
        fixture.prime(true).await;
        let selected = fixture
            .owner
            .connector_selection(&fixture.realm, &fixture.session, &"3".repeat(32))
            .unwrap_or_else(|_| panic!("matched owner metadata must be selectable"));
        assert!(selected.no_auth());
        assert_eq!(
            selected.no_credential_references,
            references == Some(json!([]))
        );
        assert!(selected.check_current().is_ok());
        // A later altered check cannot retrofit missing or changed credential
        // evidence onto the already reviewed job, even with identical tools.
        fixture.package.check.lock().unwrap()["credential_environment_references"] =
            if references == Some(json!([])) {
                json!([{"field":"token_env","reference":"OTHER_TOKEN"}])
            } else {
                json!([])
            };
        assert!(fixture.call("setup_check", json!({"family":"services","profile":"slack",
            "operations":["auth_test"],"values":{"token_env":"FIXTURE_TOKEN","timeout_seconds":2,"max_response_bytes":65536}}))
            .await.is_err());
        assert!(selected.check_current().is_err());
    }
}

#[tokio::test]
async fn public_auth_reobservation_invalidates_review_without_changing_operation_names() {
    for mode in [
        "removed",
        "changed",
        "credential",
        "missing-credential-kind",
        "identical",
        "retired",
        "epoch",
    ] {
        let root = tempfile::tempdir().unwrap();
        let storage = crate::RealmOptions {
            context_root: root.path().to_owned(),
            state_root: Some(root.path().join("state")),
            user_config_root: Some(root.path().join("user")),
        }
        .resolve("public-metadata")
        .unwrap();
        let fixture =
            connector_unprimed_fixture(storage.locator, meerkat_core::SessionId::new(), false);
        {
            let mut help = fixture.package.help.lock().unwrap();
            help["authentication"] = json!({"kind":"none"});
            help["fields"] = json!([{"name":"public_host","credential_reference":false}]);
        }
        fixture.package.check.lock().unwrap()["credential_environment_references"] = json!([]);
        fixture.prime(true).await;
        let selected = fixture
            .owner
            .connector_selection(&fixture.realm, &fixture.session, &"3".repeat(32))
            .unwrap_or_else(|_| panic!("explicit original metadata must be selectable"));
        assert!(selected.no_auth());
        assert!(selected.no_credential_references);
        let original_help = fixture.package.help.lock().unwrap().clone();
        let (entered, mut entered_rx) = oneshot::channel();
        let (release, released) = oneshot::channel();
        fixture
            .package
            .help_pauses
            .lock()
            .unwrap()
            .push_back(ConnectorHelpPause {
                entered,
                release: released,
                response: Some(original_help.clone()),
            });
        let request = json!({"family":"services","profile":"slack"});
        let mut earlier = Box::pin(fixture.call("setup_help", request.clone()));
        assert!(futures_util::poll!(&mut earlier).is_pending());
        entered_rx.try_recv().unwrap();
        assert!(
            selected.check_current().is_err(),
            "an unsettled publisher is not current"
        );
        if mode == "retired" {
            fixture.owner.retire();
        } else if mode == "epoch" {
            fixture.package.epoch.fetch_add(1, Ordering::SeqCst);
        } else {
            {
                let mut help = fixture.package.help.lock().unwrap();
                match mode {
                    "removed" => {
                        help.as_object_mut().unwrap().remove("authentication");
                    }
                    "changed" => help["authentication"] = json!({"kind":"oauth"}),
                    "credential" => help["fields"][0]["credential_reference"] = json!(true),
                    "missing-credential-kind" => {
                        help["fields"][0]
                            .as_object_mut()
                            .unwrap()
                            .remove("credential_reference");
                    }
                    _ => {}
                }
                assert_eq!(help["operations"], original_help["operations"]);
            }
            let later = fixture.call("setup_help", request).await;
            assert_eq!(
                later.is_ok(),
                matches!(mode, "removed" | "identical"),
                "{mode}"
            );
        }
        release.send(()).unwrap();
        assert!(
            earlier.await.is_err(),
            "late public metadata cannot replace the new observation: {mode}"
        );
        assert_eq!(
            selected.check_current().is_ok(),
            mode == "identical",
            "{mode}"
        );
        if mode == "removed" {
            let current = fixture
                .owner
                .connector_selection(&fixture.realm, &fixture.session, &"3".repeat(32))
                .unwrap_or_else(|_| panic!("missing declaration remains ordinary metadata"));
            assert!(!current.no_auth());
            assert_eq!(current.raw_operations, selected.raw_operations);
            assert!(
                fixture
                    .owner
                    .installed_selection(&fixture.realm, &fixture.session, &"3".repeat(32))
                    .unwrap()
                    .matched,
                "loss of public classification does not erase software evidence"
            );
        }
    }
}

#[tokio::test]
async fn connector_native_entry_change_invalidates_held_selection_without_changing_receipt() {
    let root = tempfile::tempdir().unwrap();
    let (startup, _auth, _control) = connector_fixture(root.path(), "http://127.0.0.1:1").await;
    let installed = connector_installed_fixture(
        startup.storage.locator.clone(),
        meerkat_core::SessionId::new(),
        false,
        true,
    )
    .await;
    let selected = installed.selection(connector_operations()).unwrap();
    let original = installed
        .owner
        .installed_selection(&installed.realm, &installed.session, &"3".repeat(32))
        .unwrap()
        .installed;
    installed.package.result.lock().unwrap()["native_connector"]["package_sources_sha256"] =
        json!("b".repeat(64));
    installed.package.revision.store(2, Ordering::SeqCst);
    installed
        .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
        .await
        .unwrap();
    assert!(selected.check_current().is_err());
    assert!(installed.selection(connector_operations()).is_ok());
    assert_eq!(
        installed
            .owner
            .installed_selection(&installed.realm, &installed.session, &"3".repeat(32))
            .unwrap()
            .installed,
        original
    );
}

#[tokio::test]
async fn account_publication_rechecks_held_installation_after_software_readback() {
    use meerkat_core::{
        ApprovalActionKind, ApprovalOwnerRef, ApprovalProposedAction, ApprovalRequest,
        ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk,
        ApprovalService, ApprovalStatus, RealmLocator, SessionId, connection::RealmId,
    };
    use meerkat_store::FileApprovalStore;
    use std::time::SystemTime;

    let root = tempfile::tempdir().unwrap();
    let realm = RealmLocator {
        state_root: root.path().to_owned(),
        realm: RealmId::parse("work").unwrap(),
    };
    let session = SessionId::new();
    let installed = connector_installed_fixture(realm, session.clone(), false, true).await;
    let login = Arc::new(installed.selection(connector_operations()).unwrap());
    let original = installed
        .owner
        .installed_selection(&installed.realm, &session, &"3".repeat(32))
        .unwrap()
        .installed;
    let (approvals, mut presenter) = LocalApprovals::create(root.path()).unwrap();
    let approvals = Arc::new(approvals);
    approvals.bind_session(&session).unwrap();
    let body = json!({"job_id":"3".repeat(32), "operations":connector_operations()});
    let request = ApprovalRequest {
        requester: ApprovalPrincipalId::new("fixture-requester").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: session.clone(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("fixture-installed-connector"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Review the retained software selection".into(),
            body: Some(body.clone()),
        },
        risk: ApprovalRisk::Medium,
        request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .collect(),
        expires_at: Some((SystemTime::now() + Duration::from_secs(60)).into()),
        metadata: Default::default(),
        request_provenance: None,
    };
    let (record, ()) = tokio::join!(approvals.review_access(request), async {
        presenter
            .next()
            .await
            .unwrap()
            .decide(ApprovalDecision::Approve)
            .unwrap();
    });
    let record = record.unwrap();
    assert_eq!(record.status, ApprovalStatus::Approved);
    let decisions = Arc::new(
        ApprovalService::with_store(Arc::new(
            FileApprovalStore::open(approvals.store_path()).unwrap(),
        ))
        .unwrap(),
    );
    let decision_current: crate::installed_service_review::account_request::console::DecisionCheck = {
        let decisions = decisions.clone();
        let record = record.clone();
        Arc::new(move || {
            decisions
                .get(&record.approval_id)
                .is_ok_and(|current| current == record)
        })
    };
    let current = crate::installed_service_review::account_publication_current_for_test(
        login.clone(),
        approvals.clone(),
        session.clone(),
        Some(decision_current.clone()),
    );
    assert!(
        current(),
        "the retained selection and decision are initially eligible"
    );
    // Hold the actual production predicate across an await. This controls its
    // publication boundary, not native issuance, rollback or task completion.
    let (release, wait) = oneshot::channel();
    let retained = async move {
        wait.await.unwrap();
        current()
    };
    tokio::pin!(retained);
    assert!(retained.as_mut().now_or_never().is_none());
    installed.package.result.lock().unwrap()["native_connector"]["package_sources_sha256"] =
        json!("b".repeat(64));
    installed.package.revision.store(2, Ordering::SeqCst);
    let reconciled = installed
        .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
        .await;
    release.send(()).unwrap();
    let publication_current = retained.await;
    reconciled.unwrap();
    assert!(login.check_current().is_err());
    assert!(installed.selection(connector_operations()).is_ok());
    assert_eq!(
        installed
            .owner
            .installed_selection(&installed.realm, &session, &"3".repeat(32))
            .unwrap()
            .installed,
        original
    );
    assert!(approvals.check_session(&session).is_ok());
    assert!(decision_current());
    assert_eq!(decisions.get(&record.approval_id).unwrap(), record);
    assert!(
        !publication_current,
        "deferred publication must reject the changed software owner"
    );
}

fn connector_profile(issuer: &str) -> Value {
    json!({"id":"test-profile","issuer":issuer,"client":"test-client",
        "client_secret":"private-profile-canary","authorize_params":[],
        "resource_indicator":true,"registered_redirect_uris":["https://console.example/callback"]})
}
pub(crate) fn connector_start(scope: &str, issuer: &str) -> Value {
    json!({"scope":scope,"job_id":"3".repeat(32),"operations":connector_operations(),"connector":{"slot_id":"test-account","issuer":issuer,
        "client":"test-client","resource":"https://service.example","scopes":["openid"],
        "strategy_id":"oidc-userinfo-v1","account_selection":null,
        "client_auth_profile":"test-profile"},"redirect_uri":"https://console.example/callback"})
}
fn connector_request(d: &mut Driver, id: &str, verb: &str, params: Value) {
    d.frame(
        &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,
        "method":format!("connect/connector/login/{verb}"),"params":params}))
        .unwrap(),
    );
}

#[test]
fn connector_wire_is_closed_and_callback_cannot_choose_target() {
    let root = tempfile::tempdir().unwrap();
    let mut init = generic_service_binding(root.path());
    init["connector_auth"] = json!({"profiles":[connector_profile("http://127.0.0.1:1")]});
    let decoded: Initialize = serde_json::from_value(init.clone()).unwrap();
    let profiles = decoded.connector_auth.unwrap().into_native();
    assert_eq!(profiles.len(), 1);
    assert!(matches!(&profiles[0].client_secret,
        Some(meerkat::ConnectorClientSecret::Inline(value))
            if value == "private-profile-canary"));
    assert_eq!(
        profiles[0].registered_redirect_uris,
        vec!["https://console.example/callback".to_owned()]
    );
    assert!(!format!("{:?}", profiles[0]).contains("private-profile-canary"));
    let mut without_secret = init.clone();
    without_secret["connector_auth"]["profiles"][0]
        .as_object_mut()
        .unwrap()
        .remove("client_secret");
    let decoded: Initialize = serde_json::from_value(without_secret).unwrap();
    assert!(
        decoded.connector_auth.unwrap().into_native()[0]
            .client_secret
            .is_none()
    );
    for version in [1, 2] {
        let mut old = binding(root.path());
        old["protocol_version"] = json!(version);
        if version == 2 {
            old["adapter"] = json!("forge_reading");
        }
        old["connector_auth"] = init["connector_auth"].clone();
        assert!(serde_json::from_value::<Initialize>(old).is_err());
    }
    for value in [
        Value::Null,
        json!({"profiles":null}),
        json!({"profiles":[],"registry":"not-permitted"}),
    ] {
        let mut wrong = init.clone();
        wrong["connector_auth"] = value;
        assert!(serde_json::from_value::<Initialize>(wrong).is_err());
    }
    for (field, value) in [
        ("resource_indicator", json!("true")),
        ("authorize_params", json!({})),
        ("client_secret", Value::Null),
        (
            "client_secret",
            json!({"realm_store":{"realm":"not-a-wire-selection"}}),
        ),
        ("token_store", json!("elsewhere")),
    ] {
        let mut wrong = init.clone();
        wrong["connector_auth"]["profiles"][0][field] = value;
        assert!(serde_json::from_value::<Initialize>(wrong).is_err());
    }
    let callback = json!({"scope":"scope","state":"private-state-canary",
        "code":"private-code-canary","host_attempt_handle":"private-handle-canary"});
    assert!(serde_json::from_value::<ConnectorComplete>(callback.clone()).is_ok());
    let login = connector_start("scope", "https://issuer.example");
    assert!(serde_json::from_value::<ConnectorStart>(login.clone()).is_ok());
    for required in ["job_id", "operations"] {
        let mut wrong = login.clone();
        wrong.as_object_mut().unwrap().remove(required);
        assert!(serde_json::from_value::<ConnectorStart>(wrong).is_err());
    }
    for field in [
        "target",
        "realm",
        "redirect_uri",
        "client_auth_profile",
        "job_id",
        "operations",
        "mcp_configuration",
    ] {
        let mut wrong = callback.clone();
        wrong[field] = json!("not-permitted");
        assert!(serde_json::from_value::<ConnectorComplete>(wrong).is_err());
    }
    assert!(
        serde_json::from_value::<ConnectorCancel>(json!({"scope":"scope","state":"other"}))
            .is_err()
    );
}

#[tokio::test]
async fn connector_profiles_are_not_retained_or_replayed_by_initialization() {
    let root = tempfile::tempdir().unwrap();
    let mut init = generic_service_binding(root.path());
    init["connector_auth"] = json!({"profiles":[connector_profile("http://127.0.0.1:1")]});
    let (mut d, mut rx) = driver();
    d.frame(
        &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"h:init",
        "method":"connect/initialize","params":init}))
        .unwrap(),
    );
    assert!(d.binding.as_ref().unwrap().connector_auth.is_none());
    let work = d.tasks.next().await.unwrap();
    d.work(work);
    let rows = drain(&mut rx);
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("private-profile-canary")
    );
    assert!(rows.iter().any(|r| r["id"] == "h:init"
        && matches!(
            r["error"]["data"]["code"].as_str(),
            Some("unavailable" | "invalid_binding")
        )));
    assert!(d.connector.is_none());
    assert!(!root.path().join("state").exists());
}

async fn connector_fixture(
    root: &std::path::Path,
    issuer: &str,
) -> (
    SessionStartup,
    Arc<crate::connector_auth::ConnectorAuth>,
    crate::connector_auth::Control,
) {
    use meerkat_auth_core::auth_store::{
        FileLockCoordinator, FileTokenStore, ProviderAuthPersistence,
    };
    let storage = RealmOptions {
        context_root: root.into(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("user")),
    }
    .resolve("stdio-test")
    .unwrap();
    let persistence = ProviderAuthPersistence::new(
        Arc::new(FileTokenStore::new(root.join("credentials"))),
        Arc::new(FileLockCoordinator::new(root.join("locks"))),
    );
    connector_fixture_with_owners(root, issuer, storage, persistence, None).await
}

pub(crate) async fn connector_fixture_with_owners(
    root: &std::path::Path,
    issuer: &str,
    storage: meerkat_core::ResolvedStorage,
    persistence: meerkat_auth_core::auth_store::ProviderAuthPersistence,
    native: Option<meerkat::PersistenceBundle>,
) -> (
    SessionStartup,
    Arc<crate::connector_auth::ConnectorAuth>,
    crate::connector_auth::Control,
) {
    let mut startup = SessionStartup {
        storage,
        config: meerkat::Config::default(),
        factory: meerkat::AgentFactory::new(root.join("state/stdio-test"))
            .with_provider_auth_persistence(persistence),
        persistence: native,
        connector_auth: None,
        standalone: None,
    };
    let profiles: ConnectorProfiles =
        serde_json::from_value(json!({"profiles":[connector_profile(issuer)]})).unwrap();
    let auth = startup
        .configure_connectors(profiles.into_native())
        .await
        .unwrap();
    assert!(startup.configure_connectors(Vec::new()).await.is_err());
    let control = crate::connector_auth::Control::new(auth.clone());
    (startup, auth, control)
}

/// A governed realm's own native owner, opened by its durable posture exactly
/// as `StandaloneContinuation::reopen` reopens it. `SessionStartup`'s own lazy
/// open is ordinary, and Core refuses that for a governed realm
/// (`RealmGoverned`). Provider authority exported from this owner keeps its
/// execution custody but only a weak reference to its runtime store.
pub(crate) async fn realm_native_owner(
    storage: &meerkat_core::ResolvedStorage,
) -> meerkat::PersistenceBundle {
    let (_, bundle) = meerkat::governed_realm::open_stock_realm_persistence_as(
        &meerkat::DiskStorageProvider,
        &storage.locator.state_root,
        storage.locator.realm.as_str(),
        None,
        Some(meerkat_store::RealmOrigin::Explicit),
        Some(storage.layout.clone()),
        meerkat::governed_realm::stock::issuance::DEFAULT_STOCK_AGENT,
    )
    .await
    .unwrap();
    bundle
}

async fn connector_native_session(startup: SessionStartup) -> Arc<ConnectSession> {
    let artifacts = crate::resources::reserve_setup_artifacts(&startup.storage).unwrap();
    let (approvals, _presenter) = LocalApprovals::create_browser(
        &artifacts,
        ApprovalPrincipalId::new("test-viewer").unwrap(),
        std::time::SystemTime::now() + Duration::from_secs(60),
    )
    .unwrap();
    let router = startup.connector_auth.as_ref().unwrap().router();
    let (lazy, _worker) = LazySetup::fixture_services_with_router(router);
    assert!(lazy.paired_host().research().is_none());
    let mut options = lazy
        .session_options(SessionOptions {
            model: meerkat::CreateSessionModelResolutionRequest {
                model: Some("gpt-5.4".into()),
                ..Default::default()
            },
            build: meerkat_core::SessionBuildOptions {
                llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                    Arc::new(meerkat_client::TestClient::for_provider(
                        meerkat_core::Provider::OpenAI,
                    )),
                )),
                ..Default::default()
            },
        })
        .unwrap();
    approvals.attach_lazy_setup(&mut options, &lazy).unwrap();
    Arc::new(startup.open_lazy_setup(options, &lazy).await.unwrap())
}

pub(crate) async fn connector_http(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
    use tokio::io::AsyncReadExt;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        assert!(header.len() < 16384);
        header.push(socket.read_u8().await.unwrap());
    }
    let header = String::from_utf8(header).unwrap();
    let length = header
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(str::to_owned)
        })
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(length < 16384);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await.unwrap();
    (header, body)
}
pub(crate) async fn connector_http_reply(socket: &mut tokio::net::TcpStream, body: Value) {
    let body = serde_json::to_vec(&body).unwrap();
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
    socket.write_all(&body).await.unwrap();
    socket.shutdown().await.unwrap();
}
pub(crate) fn connector_metadata(origin: &str) -> Value {
    json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),
        "token_endpoint":format!("{origin}/token"),"userinfo_endpoint":format!("{origin}/userinfo"),
        "code_challenge_methods_supported":["S256"]})
}

#[tokio::test]
async fn connector_start_retirement_joins_discovery_and_retires_late_attempt() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (startup, auth, control) = connector_fixture(root.path(), &origin).await;
    let realm = startup.storage.locator.clone();
    let (mut d, mut rx) = driver();
    d.initialize_id = Some("h:init".into());
    d.scope = Some("test-scope".into());
    d.binding = Some(serde_json::from_value(generic_service_binding(root.path())).unwrap());
    d.session = Some(connector_native_session(startup).await);
    d.connector = Some(control);
    let installed =
        connector_installed_fixture(realm, d.session.as_ref().unwrap().id().clone(), false, true)
            .await;
    d.lazy = Some(installed.lazy.clone());
    connector_request(
        &mut d,
        "h:start",
        "start",
        connector_start("test-scope", &origin),
    );
    assert_eq!(d.workers, 1);
    assert!(d.connector_busy);
    let (mut socket, _) = tokio::select! { value=listener.accept()=>value.unwrap(),
    _=d.tasks.next()=>panic!("discovery must wait for controlled response") };
    // The native client writes its request only while the start worker is
    // polled (its connection must become ready first). Keep driving the
    // worker while reading, as the driver's own loop does.
    let (header, _) = tokio::select! { value=connector_http(&mut socket)=>value,
    _=d.tasks.next()=>panic!("discovery must wait for controlled response") };
    assert!(header.starts_with("GET /.well-known/oauth-authorization-server "));
    retire(&mut d);
    d.settle();
    assert!(!drain(&mut rx).iter().any(|r| r["id"] == "h:retire"));
    connector_http_reply(&mut socket, connector_metadata(&origin)).await;
    let work = d.tasks.next().await.unwrap();
    d.work(work);
    d.settle();
    let rows = drain(&mut rx);
    assert!(
        rows.iter()
            .any(|r| r["id"] == "h:start" && r["error"]["data"]["code"] == "binding_retired")
    );
    assert!(
        rows.iter()
            .any(|r| r["id"] == "h:retire" && r["result"]["entered_work"] == "settled")
    );
    assert!(rows.iter().all(|r| r["result"].get("started").is_none()));
    assert!(d.connector.as_mut().unwrap().cancel().is_err());
    let slot = meerkat_core::CredentialAccountRef {
        realm: auth.realm.clone(),
        account: meerkat_core::CredentialAccountId::parse("test-account").unwrap(),
    };
    let status = auth.facade.connector_status(&slot).await.unwrap();
    assert!(matches!(
        status.phase,
        meerkat::ConnectorAuthPhase::AuthorizationRequired
    ));
    d.connector.take();
    Arc::try_unwrap(d.session.take().unwrap())
        .ok()
        .unwrap()
        .close()
        .await
        .unwrap();
}

#[tokio::test]
async fn connector_remote_handle_and_scope_survive_startup_into_native_completion() {
    for cancellation_mode in [
        "retire-before",
        "retire-after",
        "cancel-before",
        "cancel-after",
        "cancel-after-selection-change",
    ] {
        let before_commit = cancellation_mode.ends_with("before");
        let explicit_cancel = cancellation_mode.starts_with("cancel");
        let selection_changed = cancellation_mode.ends_with("selection-change");
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let origin_server = origin.clone();
        let (token_entered, token_wait) = oneshot::channel();
        let (release_token, token_release) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut signal = Some(token_entered);
            let mut release = Some(token_release);
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
                let value = if expected == "POST /token " {
                    let body = String::from_utf8(body).unwrap();
                    assert!(body.contains("code=private-code-canary"));
                    assert!(body.contains("client_secret=private-profile-canary"));
                    signal.take().unwrap().send(()).unwrap();
                    release.take().unwrap().await.unwrap();
                    json!({"access_token":"private-token-canary","token_type":"Bearer","expires_in":3600,"scope":"openid"})
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
                connector_http_reply(&mut socket, value).await;
            }
        });
        let (startup, auth, control) = connector_fixture(root.path(), &origin).await;
        let weak = Arc::downgrade(&auth);
        let params: ConnectorStart =
            serde_json::from_value(connector_start("test-scope", &origin)).unwrap();
        let admitted_target = params.connector.into_native(auth.realm.clone()).unwrap();
        let realm = startup.storage.locator.clone();
        let (mut d, mut rx) = driver();
        d.initialize_id = Some("h:init".into());
        d.scope = Some("test-scope".into());
        d.binding = Some(serde_json::from_value(generic_service_binding(root.path())).unwrap());
        d.session = Some(connector_native_session(startup).await);
        d.connector = Some(control);
        let installed = connector_installed_fixture(
            realm,
            d.session.as_ref().unwrap().id().clone(),
            false,
            true,
        )
        .await;
        d.lazy = Some(installed.lazy.clone());
        connector_request(
            &mut d,
            "h:start",
            "start",
            connector_start("test-scope", &origin),
        );
        let work = d.tasks.next().await.unwrap();
        d.work(work);
        let rows = drain(&mut rx);
        let started = &rows.iter().find(|r| r["id"] == "h:start").unwrap()["result"]["started"];
        let state = started["state"].as_str().unwrap().to_owned();
        let handle = started["host_attempt_handle"].as_str().unwrap().to_owned();
        let typed = d.connector.as_ref().unwrap().started().unwrap();
        assert_eq!(typed.state, state);
        assert_eq!(typed.host_attempt_handle.as_deref(), Some(handle.as_str()));
        assert_eq!(
            typed.authorize_url,
            started["authorize_url"].as_str().unwrap()
        );
        let replacement = installed.selection(connector_operations()).unwrap();
        let callback = |scope: &str, handle: Option<&str>| {
            let mut value = json!({"scope":scope,"state":state,"code":"private-code-canary"});
            if let Some(handle) = handle {
                value["host_attempt_handle"] = json!(handle);
            }
            value
        };
        connector_request(
            &mut d,
            "h:wrong-scope",
            "complete",
            callback("other", Some(&handle)),
        );
        assert!(d.tasks.is_empty());
        for (id, wrong) in [("h:missing", None), ("h:wrong", Some("wrong-handle"))] {
            connector_request(&mut d, id, "complete", callback("test-scope", wrong));
            let work = d.tasks.next().await.unwrap();
            d.work(work);
        }
        connector_request(&mut d, "h:wrong-state", "complete", {
            let mut value = callback("test-scope", Some(&handle));
            value["state"] = json!("other-state");
            value
        });
        let work = d.tasks.next().await.unwrap();
        d.work(work);
        let rows = drain(&mut rx);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| r.get("error").is_some()));
        for canary in [
            &state,
            &handle,
            "private-code-canary",
            "private-profile-canary",
        ] {
            assert!(!serde_json::to_string(&rows).unwrap().contains(canary));
        }
        connector_request(
            &mut d,
            "h:complete",
            "complete",
            callback("test-scope", Some(&handle)),
        );
        tokio::select! { result=token_wait=>result.unwrap(), _=d.tasks.next()=>panic!("entered exchange must remain held") }
        let slot = meerkat_core::CredentialAccountRef {
            realm: auth.realm.clone(),
            account: meerkat_core::CredentialAccountId::parse("test-account").unwrap(),
        };
        if before_commit {
            if explicit_cancel {
                connector_request(
                    &mut d,
                    "h:cancel-wrong-scope",
                    "cancel",
                    json!({"scope":"other"}),
                );
                assert!(d.connector_cancel.is_some());
                let rows = drain(&mut rx);
                assert!(rows.iter().any(|r| r["id"] == "h:cancel-wrong-scope"
                    && r["error"]["data"]["code"] == "invalid_binding"));
                connector_request(&mut d, "h:cancel", "cancel", json!({"scope":"test-scope"}));
                assert!(!d.retired);
            } else {
                retire(&mut d);
            }
            d.settle();
            assert!(
                !drain(&mut rx)
                    .iter()
                    .any(|r| r["id"] == "h:retire" || r["id"] == "h:cancel")
            );
            // Poll the existing worker so it observes retirement while the native
            // token exchange is still held. The worker must remain retained.
            assert!(d.tasks.next().now_or_never().is_none());
            assert_eq!(d.workers, 1);
        }
        if selection_changed {
            // The native exchange is already entered. A new software observation
            // prevents configuration preparation without inventing auth failure.
            installed.package.help.lock().unwrap()["operations"][0]["tool"] =
                json!("different_actual_tool");
            installed
                .call("setup_help", json!({"family":"services","profile":"slack"}))
                .await
                .unwrap();
        }
        release_token.send(()).unwrap();
        let work = d.tasks.next().await.unwrap();
        match &work {
            Work::Connector(
                _,
                control,
                Ok(crate::connector_auth::Reply::Completed(completed)),
                _,
                _,
            ) if !before_commit => {
                // The actual native result survives the entered worker before any
                // wire projection. Authentication alone is historical evidence;
                // the separate prepared configuration is not permission or grant.
                assert!(std::ptr::eq(
                    control.completed().unwrap(),
                    completed.as_ref()
                ));
                assert_eq!(completed.target, admitted_target);
                assert_eq!(completed.native.slot, slot);
                assert_eq!(completed.native.verified_account.issuer, origin);
                assert_eq!(
                    completed.native.verified_account.strategy_id,
                    "oidc-userinfo-v1"
                );
                assert_eq!(
                    completed.native.verified_account.subject,
                    "verified-subject"
                );
                assert_eq!(completed.native.scopes, vec!["openid"]);
                assert_eq!(
                    completed.native.scope_evidence,
                    meerkat_auth_core::connector_oauth::ScopeEvidence::TokenEndpointResponse
                );
                assert!(completed.native.expires_at.is_some());
                assert!(!completed.native.has_refresh_token);
                if selection_changed {
                    assert!(matches!(
                        completed.prepared_installation(),
                        Err("installation_unconfirmed")
                    ));
                } else {
                    let prepared = completed.prepared_installation().unwrap();
                    let binding = prepared.config().connector.as_ref().unwrap();
                    assert_eq!(binding.realm_id, slot.realm.as_str());
                    assert_eq!(binding.slot_id, slot.account.as_str());
                    assert_eq!(binding.account, "verified-subject");
                    assert_eq!(
                        binding.operations,
                        serde_json::from_value::<
                            std::collections::BTreeMap<
                                String,
                                meerkat_core::mcp_config::McpConnectorOperation,
                            >,
                        >(connector_operations())
                        .unwrap()
                    );
                    let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) =
                        &prepared.config().transport
                    else {
                        panic!("stdio")
                    };
                    assert_eq!(launch.args[3], "serve-native");
                    let original = installed.selection(connector_operations()).unwrap();
                    assert_eq!(
                        original.original()["mcpServers"]["toolkit-services-test"]["args"][3],
                        "serve"
                    );
                    let historical =
                        crate::terminal_connector::reviewed_configuration(completed).unwrap();
                    assert_eq!(historical.connector, prepared.config().connector);
                    assert!(historical.access.is_some());
                    assert_eq!(
                        historical.access.as_ref().unwrap().allowed_operations,
                        binding.operations.keys().cloned().collect()
                    );
                    // This branch runs after a real entered native completion. Closing
                    // the selected owner cannot make its typed history live again.
                    installed.owner.retire();
                    assert!(crate::terminal_connector::reviewed_configuration(completed).is_err());
                    assert_eq!(
                        historical.connector.as_ref().unwrap().account,
                        "verified-subject"
                    );
                }
            }
            Work::Connector(_, control, Err(_), _, _) if before_commit => {
                assert!(control.completed().is_none());
            }
            _ => panic!("connector worker lost its native completion disposition"),
        }
        if !before_commit {
            // The native call has committed, but the driver has not consumed its
            // result. Cancellation now loses and cannot overwrite known success.
            assert!(matches!(
                auth.facade.connector_login_cancel(&slot, &state),
                Err(meerkat::HostAuthError::Connector(
                    meerkat::ConnectorLoginError::Flow(
                        meerkat_auth_core::oauth_flow::OAuthFlowError::Missing
                    )
                ))
            ));
            if explicit_cancel {
                connector_request(&mut d, "h:cancel", "cancel", json!({"scope":"test-scope"}));
                assert!(!d.retired);
            } else {
                retire(&mut d);
            }
            d.settle();
            assert!(
                !drain(&mut rx)
                    .iter()
                    .any(|r| r["id"] == "h:retire" || r["id"] == "h:cancel")
            );
        }
        d.work(work);
        d.settle();
        server.await.unwrap();
        let rows = drain(&mut rx);
        if before_commit {
            // The delivered 09642 mutation adapter maps the final flow refusal to
            // AuthLifecycle. Keep that fixed unavailable projection, never its text.
            assert!(rows.iter().any(|r| r["id"] == "h:complete"
                && r["error"]["data"]["code"] == "connector_unavailable"));
            assert!(rows.iter().all(|r| r["result"].get("completed").is_none()));
            let status = auth.facade.connector_status(&slot).await.unwrap();
            assert!(matches!(
                status.phase,
                meerkat::ConnectorAuthPhase::AuthorizationRequired
            ));
        } else {
            assert!(rows.iter().any(|r| r["id"] == "h:complete"
                && r["result"]["completed"]["verified_account"]["subject"] == "verified-subject"));
            let control = d.connector.as_mut().unwrap();
            let completed = control.completed().unwrap();
            assert_eq!(completed.target, admitted_target);
            let native = completed.native.clone();
            assert!(rows.iter().any(|r| r["id"] == "h:complete"
                && r["result"]["completed"] == json!(meerkat::connector_ready_to_wire(&native))));
            // Refused replacement and retirement must not erase known completed
            // auth or claim that native logout occurred. The driver still owns
            // liveness; this observation does not reopen an expired setup scope.
            let mut wrong = admitted_target.clone();
            wrong.slot.realm = meerkat_core::RealmId::parse("another-realm").unwrap();
            assert!(matches!(
                control
                    .start(
                        wrong,
                        "https://console.example/callback".into(),
                        replacement
                    )
                    .await,
                Err("invalid_binding")
            ));
            assert!(matches!(control.cancel(), Err("connector_refused")));
            assert!(control.retire().is_ok());
            assert_eq!(control.completed().unwrap().native, native);
        }
        if explicit_cancel {
            assert!(!d.retired);
            assert!(!d.connector_busy);
            assert_eq!(d.workers, 0);
            if before_commit {
                assert!(
                    rows.iter()
                        .any(|r| r["id"] == "h:cancel" && r["result"]["cancelled"] == true)
                );
            } else {
                assert!(
                    rows.iter().any(|r| r["id"] == "h:cancel"
                        && r["error"]["data"]["code"] == "connector_refused")
                );
            }
        } else {
            assert!(
                rows.iter()
                    .any(|r| r["id"] == "h:retire" && r["result"]["entered_work"] == "settled")
            );
        }
        assert!(d.last.is_none());
        for canary in [
            &state,
            &handle,
            "private-code-canary",
            "private-profile-canary",
            "private-token-canary",
        ] {
            assert!(!serde_json::to_string(&rows).unwrap().contains(canary));
            assert!(!d.state().to_string().contains(canary));
        }
        d.connector.take();
        drop(auth);
        assert!(weak.upgrade().is_some());
        Arc::try_unwrap(d.session.take().unwrap())
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
        assert!(weak.upgrade().is_none());
    }
}

#[tokio::test]
async fn terminal_failed_completion_retires_exact_attempt_before_retry_or_stop() {
    tokio::time::timeout(
        Duration::from_secs(30),
        Box::pin(async {
            for stop_before_completion in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let origin = format!("http://{}", listener.local_addr().unwrap());
                let requests = Arc::new(AtomicUsize::new(0));
                let tokens = Arc::new(AtomicUsize::new(0));
                let (server_origin, server_requests, server_tokens) =
                    (origin.clone(), requests.clone(), tokens.clone());
                let server = tokio::spawn(async move {
                    // The package is controlled, but discovery, token exchange,
                    // account verification and attempt retirement use native owners.
                    for _ in 0..8 {
                        let (mut socket, _) = listener.accept().await.unwrap();
                        let (header, body) = connector_http(&mut socket).await;
                        server_requests.fetch_add(1, Ordering::SeqCst);
                        let userinfo = header.starts_with("GET /userinfo ");
                        let value = if header.starts_with("POST /token ") {
                            let body = String::from_utf8(body).unwrap();
                            assert!(body.contains("code=second-code-canary"));
                            assert!(body.contains("client_secret=private-profile-canary"));
                            assert_eq!(server_tokens.fetch_add(1, Ordering::SeqCst), 0);
                            json!({"access_token":"second-token-canary","token_type":"Bearer",
                            "expires_in":3600,"scope":"openid"})
                        } else if userinfo {
                            assert!(
                                header
                                    .to_ascii_lowercase()
                                    .contains("authorization: bearer second-token-canary")
                            );
                            json!({"sub":"second-account"})
                        } else {
                            assert!(
                                header.starts_with("GET /.well-known/oauth-authorization-server ")
                                    || header.starts_with("GET /.well-known/openid-configuration ")
                            );
                            connector_metadata(&server_origin)
                        };
                        connector_http_reply(&mut socket, value).await;
                        if userinfo {
                            return;
                        }
                    }
                    panic!(
                        "native login exceeded the bounded expected discovery/exchange requests"
                    );
                });
                let (startup, auth, mut control) = connector_fixture(root.path(), &origin).await;
                let storage = startup.storage.clone();
                let config = crate::realm::config_source(&storage)
                    .unwrap()
                    .config_doc_path(&storage.locator.realm);
                let generation = config.with_file_name("config_state.json");
                let realm_paths = meerkat_store::realm_paths_in(
                    &storage.locator.state_root,
                    storage.locator.realm.as_str(),
                );
                let grant_owner = meerkat_store::grant_owner_store_path(&realm_paths.root);
                let observed_files = [config, generation, grant_owner];
                let snapshot = || {
                    observed_files
                        .iter()
                        .map(|path| match std::fs::read(path) {
                            Ok(bytes) => Some(bytes),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                            Err(error) => panic!("could not observe {}: {error}", path.display()),
                        })
                        .collect::<Vec<_>>()
                };
                let before = snapshot();
                let installed = connector_installed_fixture(
                    storage.locator.clone(),
                    meerkat_core::SessionId::new(),
                    false,
                    true,
                )
                .await;
                let params: ConnectorStart =
                    serde_json::from_value(connector_start("test-scope", &origin)).unwrap();
                let target = params.connector.into_native(auth.realm.clone()).unwrap();
                let redirect = params.redirect_uri;
                control
                    .start(
                        target.clone(),
                        redirect.clone(),
                        installed.selection(connector_operations()).unwrap(),
                    )
                    .await
                    .unwrap();
                let first = control.started().unwrap().clone();
                assert!(first.host_attempt_handle.is_some());
                let requests_before_refusal = requests.load(Ordering::SeqCst);

                // A known cleanup failure stays fatal and does not retry its native
                // cancellation. This is a typed-result control, not a failed store.
                assert!(
                    crate::terminal_connector::settle_terminal_completion(
                        &mut control,
                        Err("connector_unavailable"),
                        true
                    )
                    .is_err()
                );
                assert_eq!(control.started().unwrap().state, first.state);

                let old_help = installed.package.help.lock().unwrap().clone();
                installed.package.help.lock().unwrap()["operations"][0]["tool"] =
                    json!("changed_raw_tool");
                installed
                    .call("setup_help", json!({"family":"services","profile":"slack"}))
                    .await
                    .unwrap();
                let (_live, mut retired) = watch::channel(false);
                let (cancel, receive) = oneshot::channel();
                let mut cancel = Some(cancel);
                if stop_before_completion {
                    cancel.take().unwrap().send(()).unwrap();
                }
                let (reply, cancellation, cleanup_failed) = control
                    .complete(
                        first.state.clone(),
                        "first-code-must-not-exchange".into(),
                        first.host_attempt_handle.clone(),
                        &mut retired,
                        receive,
                    )
                    .await;
                assert!(matches!(&reply, Err("installation_unconfirmed")));
                // The early software refusal did not consume even an already
                // signalled cancellation. The terminal must settle before its stop
                // branch as well as before returning ordinary tool feedback.
                assert!(cancellation.is_none());
                assert!(!cleanup_failed);
                assert_eq!(control.started().unwrap().state, first.state);
                let refused = crate::terminal_connector::settle_terminal_completion(
                    &mut control,
                    reply,
                    cleanup_failed,
                )
                .unwrap();
                assert!(matches!(refused, Err("installation_unconfirmed")));
                assert!(control.started().is_none());
                assert!(control.completed().is_none());
                assert!(matches!(
                    auth.facade
                        .connector_login_cancel(&target.slot, &first.state),
                    Err(meerkat::HostAuthError::Connector(
                        meerkat::ConnectorLoginError::Flow(
                            meerkat_auth_core::oauth_flow::OAuthFlowError::Missing
                        )
                    ))
                ));
                assert!(matches!(
                    auth.facade
                        .connector_status(&target.slot)
                        .await
                        .unwrap()
                        .phase,
                    meerkat::ConnectorAuthPhase::AuthorizationRequired
                ));
                assert_eq!(requests.load(Ordering::SeqCst), requests_before_refusal);
                assert_eq!(tokens.load(Ordering::SeqCst), 0);
                assert_eq!(
                    snapshot(),
                    before,
                    "the failed callback cannot publish service config or grants"
                );
                drop(cancel);

                *installed.package.help.lock().unwrap() = old_help;
                installed
                    .call("setup_help", json!({"family":"services","profile":"slack"}))
                    .await
                    .unwrap();
                control
                    .start(
                        target.clone(),
                        redirect,
                        installed.selection(connector_operations()).unwrap(),
                    )
                    .await
                    .unwrap();
                let second = control.started().unwrap().clone();
                assert_ne!(second.state, first.state);
                // Private-pipe callback retries keep their existing behavior. The
                // terminal helper is intentionally not part of this shared path.
                let (_cancel, receive) = oneshot::channel();
                let (wrong, cancellation, failed) = control
                    .complete(
                        second.state.clone(),
                        "second-code-canary".into(),
                        Some("wrong-handle".into()),
                        &mut retired,
                        receive,
                    )
                    .await;
                assert!(wrong.is_err());
                assert!(cancellation.is_none());
                assert!(!failed);
                assert_eq!(control.started().unwrap().state, second.state);
                assert_eq!(tokens.load(Ordering::SeqCst), 0);

                let (_cancel, receive) = oneshot::channel();
                let (reply, cancellation, failed) = control
                    .complete(
                        second.state.clone(),
                        "second-code-canary".into(),
                        second.host_attempt_handle.clone(),
                        &mut retired,
                        receive,
                    )
                    .await;
                assert!(cancellation.is_none());
                assert!(!failed);
                let completed = match crate::terminal_connector::settle_terminal_completion(
                    &mut control,
                    reply,
                    failed,
                )
                .unwrap()
                {
                    Ok(completed) => completed,
                    Err(code) => panic!("second native login failed: {code}"),
                };
                assert_eq!(completed.native.verified_account.subject, "second-account");
                assert!(std::ptr::eq(
                    control.completed().unwrap(),
                    completed.as_ref()
                ));
                assert!(completed.prepared_installation().is_ok());
                assert_eq!(tokens.load(Ordering::SeqCst), 1);
                server.await.unwrap();

                // A later refused completion/terminal stop cannot erase the actual
                // successful account or turn cancellation into native logout.
                let (_cancel, receive) = oneshot::channel();
                let (late, cancellation, failed) = control
                    .complete(
                        second.state.clone(),
                        "not-replayed".into(),
                        second.host_attempt_handle,
                        &mut retired,
                        receive,
                    )
                    .await;
                assert!(cancellation.is_none());
                assert!(!failed);
                assert!(matches!(
                    crate::terminal_connector::settle_terminal_completion(
                        &mut control,
                        late,
                        failed
                    )
                    .unwrap(),
                    Err("connector_refused")
                ));
                assert!(std::ptr::eq(
                    control.completed().unwrap(),
                    completed.as_ref()
                ));
                let status = auth.facade.connector_status(&target.slot).await.unwrap();
                assert_eq!(status.phase, meerkat::ConnectorAuthPhase::Authorized);
                assert_eq!(
                    status.verified_account.unwrap(),
                    completed.native.verified_account
                );
                assert_eq!(
                    snapshot(),
                    before,
                    "authentication alone cannot finalize or grant a service"
                );
                assert!(control.retire().is_ok());
            }
        }),
    )
    .await
    .expect("both native retry schedules must settle");
}

#[tokio::test]
async fn connector_controls_refuse_expired_retired_and_frontend_bindings_before_entry() {
    for mode in ["expired", "retired", "v1", "v2"] {
        let root = tempfile::tempdir().unwrap();
        let (startup, _auth, control) = connector_fixture(root.path(), "http://127.0.0.1:1").await;
        let (mut d, mut rx) = driver();
        d.initialize_id = Some("h:init".into());
        d.scope = Some("test-scope".into());
        let mut bind = generic_service_binding(root.path());
        if matches!(mode, "v1" | "v2") {
            bind = binding(root.path());
        }
        if mode == "v2" {
            bind["protocol_version"] = json!(2);
            bind["adapter"] = json!("forge_reading");
        }
        if mode == "expired" {
            bind["viewer"]["expires_at_ms"] = json!(now() - 1);
        }
        d.binding = Some(serde_json::from_value(bind).unwrap());
        d.session = Some(connector_native_session(startup).await);
        d.connector = Some(control);
        if mode == "retired" {
            d.fence("logout");
        }
        connector_request(
            &mut d,
            "h:start",
            "start",
            connector_start("test-scope", "http://127.0.0.1:1"),
        );
        connector_request(
            &mut d,
            "h:complete",
            "complete",
            json!({"scope":"test-scope",
            "state":"private-state-canary","code":"private-code-canary"}),
        );
        assert!(d.tasks.is_empty());
        assert_eq!(d.workers, 0);
        assert!(!d.connector_busy);
        let rows = drain(&mut rx);
        for id in ["h:start", "h:complete"] {
            assert!(
                rows.iter()
                    .any(|r| r["id"] == id && r.get("error").is_some())
            );
        }
        assert!(
            !serde_json::to_string(&rows)
                .unwrap()
                .contains("private-code-canary")
        );
        d.connector.take();
        Arc::try_unwrap(d.session.take().unwrap())
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn access_review_uses_existing_private_prompt_and_rejects_replacement_scope_or_expired_login()
{
    use meerkat_core::{
        ApprovalActionKind, ApprovalOwnerRef, ApprovalProposedAction, ApprovalRequest,
        ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk,
        ApprovalService, ApprovalStatus, SessionId,
    };
    use meerkat_store::FileApprovalStore;
    use std::time::SystemTime;

    for expire in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (output, mut rows) = mpsc::channel(OUTPUT_FRAMES);
        let (mut d, _, mut exchanges) = new_driver(output, Arc::new(Semaphore::new(MAX_FRAME * 2)));
        d.binding = Some(serde_json::from_value(generic_service_binding(root.path())).unwrap());
        active(&mut d);
        let actor = ApprovalPrincipalId::new("actual-viewer").unwrap();
        let (gate, presenter) = LocalApprovals::create_browser(
            root.path(),
            actor.clone(),
            SystemTime::now() + Duration::from_secs(60),
        )
        .unwrap();
        let gate = Arc::new(gate);
        let session = SessionId::new();
        gate.bind_session(&session).unwrap();
        d.approvals = Some(gate.clone());
        let body = json!({"account":"fixture-account", "connection_revision":7,
            "operations":["messages.list"], "grantee":"fixture-member",
            "requester":"fixture-requester", "lifetime":"until revoked"});
        let request = ApprovalRequest {
            requester: ApprovalPrincipalId::new("fixture-requester").unwrap(),
            owner: ApprovalOwnerRef::Session {
                session_id: session,
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Runtime,
                id: ApprovalResourceId::new("fixture-selected-realm"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "Review the exact host-held access selection".into(),
                body: Some(body.clone()),
            },
            risk: ApprovalRisk::Medium,
            request_body: None,
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                .into_iter()
                .collect(),
            expires_at: Some((SystemTime::now() + Duration::from_secs(60)).into()),
            metadata: Default::default(),
            request_provenance: None,
        };
        let expected = request.clone();
        let owner = gate.clone();
        let review = tokio::spawn(async move { owner.review_access(request).await });
        let presenter = tokio::spawn(approvals(presenter, d.bridge.clone()));
        let exchange = tokio::time::timeout(Duration::from_secs(3), exchanges.recv())
            .await
            .unwrap()
            .unwrap();
        d.prompt(exchange);
        let pending = d.pending.as_ref().unwrap();
        let id = pending.id.clone();
        let approval_id: meerkat_core::ApprovalId =
            serde_json::from_value(pending.prompt["approval_id"].clone()).unwrap();
        assert_eq!(pending.prompt["body"], body);
        assert_eq!(
            pending.prompt["allowed_decisions"],
            json!(["approve", "deny"])
        );
        let emitted = drain(&mut rows);
        assert!(
            emitted
                .iter()
                .any(|row| row["id"] == id && row["params"]["prompt"]["body"] == body)
        );
        d.answer(json!({"jsonrpc":"2.0","id":"p:999","result":{"answer":{"kind":"approval","decision":"approve"}}}));
        d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"approval","decision":"approve","grantee":"replacement-member"}}}));
        assert_eq!(d.pending.as_ref().unwrap().id, id);
        assert!(!review.is_finished());
        if expire {
            d.binding.as_mut().unwrap().viewer.expires_at_ms = 0;
        }
        d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"approval","decision":"approve"}}}));
        let returned = tokio::time::timeout(Duration::from_secs(3), review)
            .await
            .unwrap()
            .unwrap();
        let persisted = ApprovalService::with_store(Arc::new(
            FileApprovalStore::open(gate.store_path()).unwrap(),
        ))
        .unwrap()
        .get(&approval_id)
        .unwrap();
        if expire {
            assert!(returned.is_err());
            assert_eq!(persisted.status, ApprovalStatus::Pending);
            assert!(persisted.decision.is_none());
            assert!(d.retired);
        } else {
            let actual = returned.unwrap();
            assert_eq!(actual, persisted);
            assert_eq!(actual.owner, expected.owner);
            assert_eq!(actual.proposed_action, expected.proposed_action);
            assert_eq!(actual.status, ApprovalStatus::Approved);
            assert_eq!(actual.decision.unwrap().actor, actor);
            d.answer(json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"approval","decision":"approve"}}}));
            assert!(
                drain(&mut rows)
                    .iter()
                    .any(|row| row["id"] == id && row["error"]["data"]["code"] == "stale_prompt")
            );
        }
        d.fence("test-complete");
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(3), presenter)
                .await
                .unwrap()
                .unwrap(),
            Work::Presenter(false)
        ));
        assert!(d.tasks.is_empty());
        // This exercises the actual driver/presenter/native decision store.
        // No native agent, account, grant, attachment or browser was opened.
    }
}

#[cfg(feature = "console")]
#[path = "installed_service_review/account_request/console/native_tests.rs"]
mod console_account_native;
#[path = "stdio_forge_application_tests.rs"]
mod forge_application;

// Controlled canonical export responses, not a live endpoint or package install.
fn remote_fixture_job() -> Value {
    json!({"id":"4".repeat(32),"kind":"export","family":"remote-recipe","status":"succeeded",
        "request_sha256":"5".repeat(64),"recovery_request":{"id":"4".repeat(32),"sha256":"6".repeat(64)}})
}
fn remote_fixture_observation(configuration: Value, revision: u64) -> Value {
    json!({"job_id":"4".repeat(32),"revision":revision,"status":"matched",
        "scope":"current_portable_export_files_only","result":{
            "scope":"current_portable_export_files_only","export_check":{"state":"matches"},
            "configuration":configuration}})
}
// This complete JSON is what a setup agent submits. The real package owner
// validates and exports it; the fixture transports that result through the
// real public-selection owner. Offline schema hashes are not native observations.
fn independent_remote_declaration_fixture() -> (Value, Value) {
    use toolkit_package::{ContentDigest, IntegrationBundle, RemoteExportDeclaration};
    let mut plugin: Value = serde_json::from_slice(include_bytes!(
        "../../toolkit-package/integrations/connectors/github-readonly/0.1.0/plugin.json"
    ))
    .unwrap();
    let mut recipes: Value = serde_json::from_slice(include_bytes!(
        "../../toolkit-package/integrations/connectors/github-readonly/0.1.0/recipes.json"
    ))
    .unwrap();
    let source = "https://fixture.invalid/docs";
    plugin["extensions"]["ai.rkat.toolkit"]["strategies"][0]["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!(source));
    recipes["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"inert-public-docs","url":source,
        "observed_sha256":ContentDigest::of_bytes(b"inert source text"),
        "upstream":{"kind":"documentation_observation"},"license":"Inert test data"}));
    recipes["recipes"][0]["prerequisites"] =
        json!([{"kind":"application_binding","instruction":"Host admission is still required."}]);
    recipes["recipes"][0]["plan"]["server"] = json!("outside");
    recipes["recipes"][0]["plan"]["authentication"] =
        json!({"kind":"unauthenticated","evidence":"inert-public-docs"});
    let mcp = json!({"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
        "mcpServers":{"outside":{"type":"streamable-http","url":"https://fixture.invalid/mcp"}}});
    let bundle = IntegrationBundle::parse(
        &serde_json::to_vec(&plugin).unwrap(),
        Some(&serde_json::to_vec(&mcp).unwrap()),
        &serde_json::to_vec(&recipes).unwrap(),
    )
    .unwrap();
    let observed = ContentDigest::of_bytes(b"inert schema fixture, not a native observation");
    let declaration = json!({"schema_version":1,"selection_kind":"remote_declaration",
        "package":bundle.package().identity,"package_content_digest":bundle.package().content_digest().unwrap(),
        "strategy":"hosted","components":[{"id":"remote","version":"inert-observation",
            "source":{"kind":"remote_service","endpoint":"https://fixture.invalid/mcp","publisher":"github",
                "observed_contract":observed,"transport":"mcp_streamable_http"},
            "runtime":"remote","placement":"remote","lifetime":"externally_managed",
            "activation_scope":{"kind":"independent"},"dependencies":[],"required_contracts":[],"host_services":[],"state":[],
            "readiness":{"owner":"fixture/client","checks":["observe-current-tools"]},
            "shutdown":{"owner":"fixture/client","operation":"disconnect","maximum_wait_ms":1000,"timeout":"preserve_uncertainty"},
            "rollback":{"kind":"irreversible","explanation":"The host does not own the service."}}],
        "exports":[{"role":"service_connector","id":"github-readonly.service","components":["remote"],
            "authentication":{"kind":"none"},"operations":[{"id":"read-item","schema_digest":observed,
                "declared_effect":"read","required_authority":["read"],"resources":{"kind":"connection_wide"},
                "result_limits":{"maximum_bytes":4096,"maximum_items":10}}],"events":[],
            "enforcement_contract":{"name":"fixture/operation-policy","version":{"major":1,"minimum_minor":0}},
            "diagnostics":"check-account","cleanup":"disconnect"}],
        "portable_mcp_entries":[{"name":"outside","component":"remote","operation_tools":{"read-item":"actual.lookup"}}]});
    assert!(declaration.get("configuration").is_none());
    assert!(declaration.get("policy").is_none());
    assert!(
        declaration["components"][0]["source"]
            .get("configuration_revision")
            .is_none()
    );
    let typed: RemoteExportDeclaration = serde_json::from_value(declaration.clone()).unwrap();
    let (portable, service) = bundle
        .export_portable_declaration(bundle.digest(), typed)
        .unwrap();
    let service = serde_json::to_value(service.unwrap()).unwrap();
    let request = json!({"publisher":bundle.package().identity.publisher,"name":bundle.package().identity.name,
        "version":bundle.package().identity.version,"digest":bundle.digest(),
        "declaration_json":serde_json::to_string(&declaration).unwrap()});
    let canonical = json!({"identity":bundle.package().identity,"digest":bundle.digest(),
        "selection_kind":"remote_declaration","selection_digest":service["declaration_digest"],
        "mcp_configuration":serde_json::from_slice::<Value>(portable.mcp_json().unwrap()).unwrap(),
        "declared_remote_service":service});
    (request, canonical)
}

#[cfg(feature = "console")]
pub(crate) async fn remote_public_fixture(root: &std::path::Path) -> ConnectorInstallFixture {
    remote_public_fixture_kind(root, false).await
}
pub(crate) async fn remote_public_fixture_kind(
    root: &std::path::Path,
    declaration: bool,
) -> ConnectorInstallFixture {
    let storage = crate::RealmOptions {
        context_root: root.to_owned(),
        state_root: Some(root.join("state")),
        user_config_root: Some(root.join("home")),
    }
    .resolve("remote-owner")
    .unwrap();
    let fixture =
        connector_unprimed_fixture(storage.locator, meerkat_core::SessionId::new(), false);
    let destination = root
        .join("remote-recipes")
        .join(format!("export-{}", "4".repeat(32)))
        .join("output");
    *fixture.package.result.lock().unwrap() = json!({"scope":"portable_mcp_configuration_only",
        "recipe":{"identity":{"publisher":"research","name":"outside","version":"1"},"digest":"1".repeat(64),
            "origin":{"kind":"local_investigation","decision":{"owner":"research","record":"selected","revision":"1"}}},
        "selection_digest":"b".repeat(64),"destination":destination,
        "plugin_path":destination.join("plugin.json"),"mcp_path":destination.join("mcp.json"),
        "mcp_configuration":{"$schema":"https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
            "mcpServers":{"outside":{"type":"streamable-http","url":"https://fixture.invalid/mcp"}}},
        "declared_remote_service":{"manifest_digest":"b".repeat(64),"component":"remote","export":"read-service",
            "mcp_name":"outside","authentication":{"kind":"none"},"operation_tools":{"read-item":"actual.lookup"}}});
    let request = if declaration {
        let (request, canonical) = independent_remote_declaration_fixture();
        let mut output = fixture.package.result.lock().unwrap();
        for field in [
            "selection_kind",
            "selection_digest",
            "mcp_configuration",
            "declared_remote_service",
        ] {
            output[field] = canonical[field].clone();
        }
        output["recipe"]["identity"] = canonical["identity"].clone();
        output["recipe"]["digest"] = canonical["digest"].clone();
        request
    } else {
        json!({"publisher":"research","name":"outside","version":"1",
        "digest":"1".repeat(64),"activation_json":"controlled canonical owner input"})
    };
    fixture.call("setup_export_recipe", request).await.unwrap();
    assert!(
        fixture
            .owner
            .public_selection(&fixture.realm, &fixture.session, &"4".repeat(32))
            .is_err(),
        "export response alone never proves current readback"
    );
    fixture
        .call("setup_reconcile", json!({"job_id":"4".repeat(32)}))
        .await
        .unwrap();
    fixture
}
impl ConnectorInstallFixture {
    pub(crate) fn public_remote(&self) -> crate::service_install::PublicServiceSelection {
        self.owner
            .public_selection(&self.realm, &self.session, &"4".repeat(32))
            .ok()
            .unwrap()
    }
}

#[tokio::test]
async fn remote_export_selection_uses_exact_declared_raw_names_without_an_install_receipt() {
    for declaration in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let fixture = remote_public_fixture_kind(root.path(), declaration).await;
        let selected = fixture.public_remote();
        let names = std::collections::BTreeSet::from(["actual.lookup".to_owned()]);
        let config = selected.public_configuration(&names).unwrap();
        assert_eq!(config.name, "outside");
        assert!(config.connector.is_none());
        assert_eq!(config.access.as_ref().unwrap().allowed_operations, names);
        assert!(
            selected
                .public_configuration(&std::collections::BTreeSet::from(["read-item".to_owned()]))
                .is_err()
        );
        assert!(
            selected
                .public_configuration(&std::collections::BTreeSet::from([
                    "unselected.write".to_owned()
                ]))
                .is_err()
        );
        assert_eq!(
            selected.operation_basis()["kind"],
            "remote_recipe_declarations"
        );
        let source = selected.review_source();
        assert!(source.get("software").is_none());
        assert_eq!(source["remote_export"]["job_id"], "4".repeat(32));
        assert!(
            fixture
                .owner
                .installed_selection(&fixture.realm, &fixture.session, &"4".repeat(32))
                .is_err()
        );
        assert!(
            fixture
                .owner
                .connector_selection(&fixture.realm, &fixture.session, &"4".repeat(32))
                .is_err()
        );
        assert!(
            fixture
                .owner
                .public_selection(
                    &fixture.realm,
                    &meerkat_core::SessionId::new(),
                    &"4".repeat(32)
                )
                .is_err()
        );
        let mut foreign = fixture.realm.clone();
        foreign.state_root = root.path().join("foreign");
        assert!(
            fixture
                .owner
                .public_selection(&foreign, &fixture.session, &"4".repeat(32))
                .is_err()
        );
        fixture
            .call("setup_job", json!({"job_id":"4".repeat(32)}))
            .await
            .unwrap();
        assert!(selected.public_configuration(&names).is_ok());
    }
}

#[tokio::test]
async fn remote_export_custody_refuses_changed_removed_or_stale_declarations() {
    for declaration in [false, true] {
        for mode in [
            "removed",
            "auth",
            "raw",
            "url",
            "headers",
            "manifest",
            "kind",
            "mixed-digest",
            "extra-server",
            "lower-revision",
            "lost",
            "epoch",
            "retire",
        ] {
            let root = tempfile::tempdir().unwrap();
            let fixture = remote_public_fixture_kind(root.path(), declaration).await;
            let selected = fixture.public_remote();
            let names = std::collections::BTreeSet::from(["actual.lookup".to_owned()]);
            match mode {
                "epoch" => {
                    fixture.package.epoch.fetch_add(1, Ordering::SeqCst);
                }
                "retire" => fixture.owner.retire(),
                _ => {
                    {
                        let mut export = fixture.package.result.lock().unwrap();
                        match mode {
                            "removed" => {
                                export
                                    .as_object_mut()
                                    .unwrap()
                                    .remove("declared_remote_service");
                            }
                            "auth" => {
                                export["declared_remote_service"]["authentication"] = json!({"kind":"binding","owner":"auth","requirement":"account","credential_audience":"provider"})
                            }
                            "raw" => {
                                export["declared_remote_service"]["operation_tools"]["read-item"] =
                                    json!("substituted.lookup")
                            }
                            "url" => {
                                export["mcp_configuration"]["mcpServers"]["outside"]["url"] =
                                    json!("https://different.invalid/mcp")
                            }
                            "headers" => {
                                export["mcp_configuration"]["mcpServers"]["outside"]["headers"] =
                                    json!({"Authorization":"not-a-credential"})
                            }
                            "manifest" => {
                                export["declared_remote_service"][if declaration {
                                    "declaration_digest"
                                } else {
                                    "manifest_digest"
                                }] = json!("c".repeat(64))
                            }
                            "kind" => {
                                export["selection_kind"] = json!(if declaration {
                                    "activation"
                                } else {
                                    "remote_declaration"
                                })
                            }
                            "mixed-digest" => {
                                export["declared_remote_service"][if declaration {
                                    "manifest_digest"
                                } else {
                                    "declaration_digest"
                                }] = json!("b".repeat(64))
                            }
                            "extra-server" => {
                                export["mcp_configuration"]["mcpServers"]["other"] = json!({"type":"streamable-http","url":"https://other.invalid/mcp"})
                            }
                            "lower-revision" => fixture.package.revision.store(0, Ordering::SeqCst),
                            "lost" => fixture.package.lose_read.store(true, Ordering::SeqCst),
                            _ => unreachable!(),
                        }
                    }
                    let _ = fixture
                        .call("setup_reconcile", json!({"job_id":"4".repeat(32)}))
                        .await;
                }
            }
            assert!(selected.public_configuration(&names).is_err(), "{mode}");
            assert!(
                fixture
                    .owner
                    .connector_selection(&fixture.realm, &fixture.session, &"4".repeat(32))
                    .is_err(),
                "{mode}"
            );
        }
    }
}

#[tokio::test]
async fn remote_export_cancelled_or_late_read_cannot_rearm_an_old_selection() {
    for declaration in [false, true] {
        for cancel in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let fixture = Arc::new(remote_public_fixture_kind(root.path(), declaration).await);
            let selected = fixture.public_remote();
            let names = std::collections::BTreeSet::from(["actual.lookup".to_owned()]);
            let (entered, wait) = oneshot::channel();
            let (release, released) = oneshot::channel();
            let response = json!({"observation":remote_fixture_observation(fixture.package.result.lock().unwrap().clone(),1)});
            fixture
                .package
                .remote_pauses
                .lock()
                .unwrap()
                .push_back(ConnectorHelpPause {
                    entered,
                    release: released,
                    response: Some(response),
                });
            let worker = {
                let fixture = fixture.clone();
                tokio::spawn(async move {
                    fixture
                        .call("setup_reconcile", json!({"job_id":"4".repeat(32)}))
                        .await
                })
            };
            wait.await.unwrap();
            assert!(selected.public_configuration(&names).is_err());
            if cancel {
                worker.abort();
                assert!(worker.await.unwrap_err().is_cancelled());
                drop(release);
                assert!(selected.public_configuration(&names).is_err());
            } else {
                fixture
                    .package
                    .result
                    .lock()
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove("declared_remote_service");
                fixture.package.revision.store(2, Ordering::SeqCst);
                fixture
                    .call("setup_reconcile", json!({"job_id":"4".repeat(32)}))
                    .await
                    .unwrap();
                release.send(()).unwrap();
                assert!(worker.await.unwrap().is_err());
                assert!(selected.public_configuration(&names).is_err());
            }
        }
    }
}

#[tokio::test]
async fn canonical_software_plan_reaches_checked_installation_with_exclusive_document_identity() {
    for change in [
        "valid",
        "kind",
        "dual-digest",
        "digest",
        "raw-digest",
        "retire",
        "recheck-file",
    ] {
        let root = tempfile::tempdir().unwrap();
        let fixture = selected_software_fixture(root.path()).await;
        let selected = fixture.raw_selection().ok().unwrap();
        let basis = selected.operation_basis();
        assert_eq!(basis["kind"], "selected_software_declarations");
        assert_eq!(basis["document_kind"], "local_software_declaration");
        assert!(basis.get("manifest_digest").is_none());
        let inputs = selected.selected_inputs().unwrap();
        assert_eq!(inputs.document.filename(), "declaration.json");
        assert_eq!(basis["selection_sha256"], inputs.document.sha256());
        assert!(selected.no_auth());
        assert!(
            !selected.no_credential_references,
            "package declaration is not a credential observation"
        );
        assert_eq!(selected.raw_operations, ["actual_read".to_owned()].into());
        if change == "valid" {
            assert!(selected.check_current().is_ok());
            continue;
        }
        if change == "retire" {
            fixture.owner.retire();
        } else if change == "recheck-file" {
            {
                let mut check = fixture.package.check.lock().unwrap();
                check["files"]["activation.json"] = check["files"]["declaration.json"].clone();
            }
            assert!(
                fixture
                    .call(
                        "setup_selected_check",
                        json!({"profile":"python","staged_id":"outside",
                "component":"server","document_kind":"local_software_declaration"})
                    )
                    .await
                    .is_err()
            );
        } else {
            {
                let mut result = fixture.package.result.lock().unwrap();
                match change {
                    "kind" => {
                        result.as_object_mut().unwrap().remove("document_kind");
                    }
                    "dual-digest" => result["manifest_digest"] = json!("c".repeat(64)),
                    "digest" => result["declaration_digest"] = json!("f".repeat(64)),
                    "raw-digest" => result["selection_sha256"] = json!("e".repeat(64)),
                    _ => unreachable!(),
                }
            }
            assert!(
                fixture
                    .call("setup_reconcile", json!({"job_id":"3".repeat(32)}))
                    .await
                    .is_err(),
                "{change}"
            );
        }
        assert!(selected.check_current().is_err(), "{change}");
    }
}
