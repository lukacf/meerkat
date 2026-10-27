//! Source controls for the explicit trusted application mode. No control opens
//! a native session, contacts Forge or submits an application change.
use super::*;

fn application_binding(root: &std::path::Path) -> Value {
    let mut value = binding(root);
    value["protocol_version"] = json!(4);
    value["adapter"] = json!("forge_application");
    value["application"]["application_id"] = json!("custom-app");
    value["forge"] = json!({
        "host": {"protocol":1,"host_id":format!("forge:{}", "a".repeat(64)),
            "application_id":"custom-app","source_digest":"b".repeat(64)},
        "private_fields":["PRIVATE_TOKEN","PRIVATE_KEY"]
    });
    value
}

#[test]
fn application_v4_retains_exact_host_and_declared_private_fields() {
    let root = tempfile::tempdir().unwrap();
    let value = application_binding(root.path());
    let decoded: Initialize = serde_json::from_value(value.clone()).unwrap();
    assert!(decoded.valid());
    assert!(decoded.adapter == Adapter::ForgeApplication);
    let selection = decoded.forge.unwrap();
    assert_eq!(
        serde_json::to_value(selection.host).unwrap(),
        value["forge"]["host"]
    );
    assert_eq!(
        selection.private_fields,
        ["PRIVATE_TOKEN".to_owned(), "PRIVATE_KEY".to_owned()].into()
    );
    let mut no_private_fields = value.clone();
    no_private_fields["forge"]["private_fields"] = json!([]);
    assert!(
        serde_json::from_value::<Initialize>(no_private_fields)
            .unwrap()
            .valid()
    );

    for (pointer, replacement) in [
        ("/forge/host/application_id", json!("other-app")),
        ("/forge/host/protocol", json!(2)),
        ("/forge/host/host_id", json!("forge:unverified")),
        ("/forge/host/source_digest", json!("B".repeat(64))),
        ("/forge/private_fields", Value::Null),
        ("/forge/private_fields", json!([1])),
        ("/forge/host", Value::Null),
        ("/forge", Value::Null),
        ("/application/origin", json!("http://localhost:12345")),
        ("/application/origin", json!("http://127.0.0.1:12345/")),
        ("/application/origin", json!("https://console.example")),
        ("/viewer/expires_at_ms", json!(0)),
    ] {
        let mut wrong = value.clone();
        *wrong.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            !serde_json::from_value::<Initialize>(wrong).is_ok_and(|v| v.valid()),
            "{pointer}"
        );
    }
    for pointer in ["/forge/host", "/forge", ""] {
        let mut wrong = value.clone();
        wrong.pointer_mut(pointer).unwrap()["extra"] = json!(true);
        assert!(serde_json::from_value::<Initialize>(wrong).is_err());
    }
    for field in ["forge", "adapter"] {
        let mut wrong = value.clone();
        wrong.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Initialize>(wrong).is_err());
    }
    let mut missing_fields = value;
    missing_fields["forge"]
        .as_object_mut()
        .unwrap()
        .remove("private_fields");
    assert!(serde_json::from_value::<Initialize>(missing_fields).is_err());
}

#[test]
fn application_v4_does_not_change_or_widen_legacy_closed_modes() {
    let root = tempfile::tempdir().unwrap();
    let application = application_binding(root.path());
    let mut reading_v2 = binding(root.path());
    reading_v2["protocol_version"] = json!(2);
    reading_v2["adapter"] = json!("forge_reading");
    for legacy in [
        binding(root.path()),
        reading_v2,
        service_binding(root.path()),
    ] {
        let decoded: Initialize = serde_json::from_value(legacy.clone()).unwrap();
        assert!(decoded.valid());
        assert!(decoded.forge.is_none());
        let mut extra = legacy.clone();
        extra["forge"] = application["forge"].clone();
        assert!(serde_json::from_value::<Initialize>(extra).is_err());
        let mut wrong_mode = legacy;
        wrong_mode["adapter"] = json!("forge_application");
        assert!(!serde_json::from_value::<Initialize>(wrong_mode).is_ok_and(|v| v.valid()));
    }
    for adapter in ["forge_reading", "paired_host", "forge_services", "services"] {
        let mut wrong = application.clone();
        wrong["adapter"] = json!(adapter);
        assert!(!serde_json::from_value::<Initialize>(wrong).unwrap().valid());
    }
    for field in ["paired_host", "services", "connector_auth", "credential"] {
        let mut wrong = application.clone();
        wrong[field] = json!({});
        assert!(serde_json::from_value::<Initialize>(wrong).is_err());
    }
}

#[tokio::test]
async fn application_v4_refuses_mismatch_and_joins_preentry_retirement_without_startup() {
    for mismatch in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut value = application_binding(root.path());
        if mismatch {
            value["forge"]["host"]["application_id"] = json!("other-app");
        }
        let (mut d, mut rx) = driver();
        d.frame(
            &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"h:init",
            "method":"connect/initialize","params":value}))
            .unwrap(),
        );
        if mismatch {
            assert!(!d.opening);
            assert!(d.initialize_id.is_none());
            assert!(d.tasks.is_empty());
            assert_eq!(
                drain(&mut rx)[0]["error"]["data"]["code"],
                "invalid_binding"
            );
        } else {
            assert!(d.opening);
            assert!(drain(&mut rx).is_empty());
            d.frame(br#"{"jsonrpc":"2.0","id":"h:retire","method":"connect/retire","params":{"target":{"initialize_request_id":"h:init"},"retirement_id":"logout","reason":"logout"}}"#);
            // The real driver has not polled open yet. Its existing custody
            // wrapper must observe retirement before any native startup.
            let work = tokio::time::timeout(Duration::from_secs(2), d.tasks.next())
                .await
                .expect("pre-entry retirement must settle")
                .expect("one initialized work item");
            d.work(work);
            d.settle();
            assert!(d.session.is_none());
            assert!(d.tasks.is_empty());
            let rows = drain(&mut rx);
            assert!(
                rows.iter()
                    .any(|row| row["id"] == "h:retire" && row.get("result").is_some())
            );
            assert!(rows.iter().all(|row| row["method"] != "connect/prompt"));
        }
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }
}

#[tokio::test]
async fn application_v4_composes_generic_schema_and_only_one_frontend_presenter() {
    let root = tempfile::tempdir().unwrap();
    let binding: Initialize = serde_json::from_value(application_binding(root.path())).unwrap();
    let selected = binding.forge.unwrap();
    let lazy =
        LazySetup::private_forge_application(selected.host, selected.private_fields).unwrap();
    let _presenter = lazy.take_forge_presenter().unwrap();
    assert!(lazy.take_forge_presenter().is_err());
    assert!(lazy.installs().is_none());
    assert!(!lazy.owner().allows_acquisition());
    let tools = lazy.dispatcher().tools();
    let prepare = tools
        .iter()
        .find(|tool| tool.name.as_ref() == crate::forge::FORGE_PREPARE)
        .unwrap();
    assert_eq!(
        prepare.input_schema["properties"]["expected_revision"],
        json!({})
    );
    assert_eq!(
        prepare.input_schema["required"],
        json!(["configuration", "expected_revision"])
    );
    let legacy = LazySetup::reading();
    let _legacy_presenter = legacy.take_forge_presenter().unwrap();
    let legacy_tools = legacy.dispatcher().tools();
    let legacy_prepare = legacy_tools
        .iter()
        .find(|tool| tool.name.as_ref() == crate::forge::FORGE_PREPARE)
        .unwrap();
    assert_eq!(
        legacy_prepare.input_schema["properties"]["expected_revision"]["type"],
        "integer"
    );
    // The actual composed definitions feed negotiation, not a parallel table.
    let names = lazy.declared_tools();
    let caps = capabilities(4, Adapter::ForgeApplication, false, &names, None).unwrap();
    assert_eq!(caps["tools"], json!(names));
    assert_eq!(caps["forge_configuration"], true);
    assert_eq!(caps["service_preparation"], false);
    assert_eq!(caps["paired_host"], false);
    assert!(
        !caps["prompt_kinds"]
            .as_array()
            .unwrap()
            .contains(&json!("provider_api_key"))
    );
}

#[tokio::test]
async fn application_v4_selection_reuses_private_bridge_and_joins_retired_prompt() {
    use meerkat_core::{AgentToolDispatcher, ToolCallView};
    let root = tempfile::tempdir().unwrap();
    let binding: Initialize = serde_json::from_value(application_binding(root.path())).unwrap();
    let selection = binding.forge.unwrap();
    let lazy =
        LazySetup::private_forge_application(selection.host, selection.private_fields).unwrap();
    let mut presenter = lazy.take_forge_presenter().unwrap();
    let dispatcher = lazy.dispatcher();
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let mut dispatch = Box::pin(dispatcher.dispatch(ToolCallView {
        id: "select",
        name: crate::forge::FORGE_SELECT_HOST,
        args: &args,
    }));
    assert!(dispatch.as_mut().now_or_never().is_none());
    let prompt = tokio::time::timeout(Duration::from_secs(2), presenter.next())
        .await
        .unwrap()
        .expect("the composed Forge receiver must receive the selection");
    let (bridge, mut prompts, retire) = PromptBridge::channel();
    let mut present = Box::pin(crate::forge::present_stdio(
        prompt,
        &bridge,
        &binding.application.application_id,
        &binding.application.origin,
    ));
    assert!(present.as_mut().now_or_never().is_none());
    let exchange = tokio::time::timeout(Duration::from_secs(2), prompts.recv())
        .await
        .unwrap()
        .expect("selection must use the existing private prompt bridge");
    // Do not answer the origin prompt: no HTTP, provider or native call can
    // begin. Retirement closes this exact presentation and original waiter.
    let captured_prompt = exchange.prompt.clone();
    retire.send_replace(true);
    let presentation_result = tokio::time::timeout(Duration::from_secs(2), &mut present).await;
    drop(present);
    let dispatch_result = tokio::time::timeout(Duration::from_secs(2), &mut dispatch).await;
    drop(dispatch);
    assert_eq!(
        captured_prompt,
        json!({"kind":"forge_origin",
        "permitted_origin":"http://127.0.0.1:12345"})
    );
    assert!(
        presentation_result
            .expect("retired presentation must join")
            .is_err()
    );
    assert!(
        dispatch_result
            .expect("retired dispatch must join")
            .is_err()
    );
    assert!(exchange.response.is_closed());
    assert!(prompts.try_recv().is_err());
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}
