//! Transport and currentness controls; no installed native probe is claimed.
use super::*;
use crate::service_probe::{
    ServiceProbeSelection,
    tests::{binding as host_binding, selected},
};

fn selected_binding(root: &std::path::Path) -> (Value, Arc<ServiceProbeSelection>) {
    let host = host_binding(root);
    let mut value = generic_service_binding(root);
    value["protocol_version"] = json!(6);
    value["application"] = json!({"application_id":host.application_id,"instance_id":host.instance_id,"origin":host.origin});
    value["viewer"] = json!({"principal_id":host.viewer_id,"login_scope_id":host.login_scope_id,"expires_at_ms":host.expires_at_ms});
    value["realm"] = json!({"realm_id":host.realm.realm.as_str(),"state_root":host.realm.state_root,
        "context_root":host.context_root,"user_config_root":host.user_config_root});
    (
        value,
        Arc::new(ServiceProbeSelection::new(host, vec![selected("one")]).unwrap()),
    )
}

#[test]
fn protocol_six_requires_exact_process_binding_and_closed_probe_selectors() {
    let root = tempfile::tempdir().unwrap();
    let (value, selection) = selected_binding(root.path());
    let (mut d, mut rx) = driver();
    let parsed: Initialize = serde_json::from_value(value.clone()).unwrap();
    assert!(!d.probe_initialization_matches(&parsed));
    d.probe_selection = Some(selection);
    assert!(parsed.valid() && d.probe_initialization_matches(&parsed));
    for pointer in [
        "/application/application_id",
        "/viewer/login_scope_id",
        "/viewer/principal_id",
        "/realm/realm_id",
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("another");
        assert!(!d.probe_initialization_matches(&serde_json::from_value(changed).unwrap()));
    }
    for field in [
        "operation",
        "arguments",
        "resource",
        "caller",
        "grant",
        "endpoint",
    ] {
        let mut request = json!({"scope":"scope","job_id":"job","profile_id":"one"});
        request[field] = json!("injected");
        assert!(serde_json::from_value::<Probe>(request).is_err());
    }
    d.scope = Some("scope".into());
    d.binding = Some(parsed);
    d.frame(br#"{"jsonrpc":"2.0","id":"h:probe","method":"connect/probe","params":{"scope":"scope","job_id":"job","profile_id":"one"}}"#);
    assert_eq!(d.workers, 0);
    assert!(d.tasks.is_empty());
    assert!(
        drain(&mut rx)
            .iter()
            .any(|row| row["error"]["data"]["code"] == "probe_unavailable")
    );
}

#[tokio::test]
async fn actual_host_retirement_retires_bridge_without_waiting_for_driver_poll() {
    let root = tempfile::tempdir().unwrap();
    let (_, selection) = selected_binding(root.path());
    let (mut bridge, mut receive, stop) = PromptBridge::channel();
    bridge.probe_lifetime = Some((selection.lifetime.clone(), selection.binding.expires_at_ms));
    let ask = bridge.ask(json!({"kind":"approval"}));
    tokio::pin!(ask);
    assert!(futures_util::poll!(ask.as_mut()).is_pending());
    let exchange = receive.recv().await.unwrap();
    selection.control().retire();
    let result = tokio::time::timeout(Duration::from_secs(2), ask)
        .await
        .unwrap();
    assert!(result.is_err());
    assert!(bridge.retired());
    assert!(
        !*stop.borrow(),
        "this control exercises direct host retirement, not the pipe watch"
    );
    assert!(exchange.response.is_closed());
    assert!(bridge.ask(json!({"kind":"service_secret"})).await.is_err());
}

#[test]
fn retired_host_fences_setup_before_next_frame_and_legacy_five_stays_separate() {
    let root = tempfile::tempdir().unwrap();
    let (value, selection) = selected_binding(root.path());
    let (mut d, mut rx) = driver();
    d.probe_selection = Some(selection.clone());
    d.binding = Some(serde_json::from_value(value).unwrap());
    d.scope = Some("scope".into());
    selection.control().retire();
    d.frame(br#"{"jsonrpc":"2.0","id":"h:probe","method":"connect/probe","params":{"scope":"scope","job_id":"job","profile_id":"one"}}"#);
    assert!(d.retired && *d.plain_cancel.borrow() && *d.stop.borrow());
    assert_eq!(d.workers, 0);
    let _ = drain(&mut rx);
    let (old, _) = driver();
    let mut v5 = generic_service_binding(root.path());
    v5["protocol_version"] = json!(5);
    let parsed = serde_json::from_value::<Initialize>(v5).unwrap();
    assert!(parsed.valid());
    assert!(old.probe_initialization_matches(&parsed));
    assert!(!old.probe_available());
}

#[tokio::test]
async fn post_close_review_accepts_only_retained_commission_prompt_and_old_presenter_refuses() {
    let root = tempfile::tempdir().unwrap();
    let (value, selection) = selected_binding(root.path());
    let (mut d, mut rx) = driver();
    d.probe_selection = Some(selection);
    d.binding = Some(serde_json::from_value(value).unwrap());
    d.scope = Some("scope".into());
    d.fence("finalizing");
    d.probe_commissioning = true;
    assert!(d.retired && d.commissioning_current());
    let (send, receive) = oneshot::channel();
    d.prompt(Exchange {commissioning:true,prompt:json!({"kind":"approval","approval_id":"actual-review",
        "summary":"Exact post-close review","body":{"realm":"private-probe"},"allowed_decisions":["approve","deny"]}),
        response:send,owner_close:None});
    let rows = drain(&mut rx);
    let id = rows
        .iter()
        .find(|row| row["method"] == "connect/prompt")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    d.frame(&serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":{"answer":{"kind":"approval","decision":"approve"}}})).unwrap());
    assert!(matches!(
        receive.await.unwrap().unwrap(),
        Answer::Approval {
            decision: Decision::Approve
        }
    ));
    // An old setup presenter cannot use the new phase to reopen its prompt.
    let (send, mut receive) = oneshot::channel();
    d.prompt(Exchange {
        commissioning: false,
        prompt: json!({"kind":"approval"}),
        response: send,
        owner_close: None,
    });
    assert!(matches!(
        receive.try_recv(),
        Err(oneshot::error::TryRecvError::Closed)
    ));
    assert!(*d.plain_cancel.borrow() && !d.commissioning_current());
    let _ = drain(&mut rx);
}

#[tokio::test]
async fn explicit_close_during_commissioning_fences_without_dropping_accepted_worker() {
    let root = tempfile::tempdir().unwrap();
    let (value, selection) = selected_binding(root.path());
    let (mut d, mut rx) = driver();
    d.probe_selection = Some(selection.clone());
    d.binding = Some(serde_json::from_value(value).unwrap());
    d.scope = Some("scope".into());
    d.fence("finalizing");
    d.probe_commissioning = true;
    let bridge = PromptBridge {
        prompts: d.bridge.prompts.clone(),
        retired: d.plain_cancel.subscribe(),
        probe_lifetime: Some((selection.lifetime.clone(), selection.binding.expires_at_ms)),
        commissioning: true,
    };
    let (release, released) = oneshot::channel();
    d.workers += 1;
    d.tasks.push(
        async move {
            let _ = released.await;
            Work::ProbeFinalized(Ok(None))
        }
        .boxed(),
    );
    d.frame(br#"{"jsonrpc":"2.0","id":"h:close","method":"connect/close","params":{"target":{"scope":"scope"}}}"#);
    assert!(d.closing && bridge.retired() && *d.plain_cancel.borrow());
    assert_eq!(d.workers, 1);
    assert!(!d.closed);
    let before = drain(&mut rx);
    assert!(
        !before
            .iter()
            .any(|row| row["id"] == "h:close" && row.get("result").is_some())
    );
    let _ = release.send(());
    let work = d.tasks.next().await.unwrap();
    d.work(work);
    d.settle();
    while let Some(work) = d.tasks.next().await {
        d.work(work);
        d.settle();
    }
    assert_eq!(d.workers, 0);
    assert!(!d.probe_commissioning);
    let after = drain(&mut rx);
    assert!(
        after
            .iter()
            .any(|row| row["id"] == "h:close" && row.get("result").is_some())
    );
}

#[test]
fn prior_setup_send_cannot_cancel_a_later_active_or_completed_probe() {
    for active in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (value, selection) = selected_binding(root.path());
        let (mut d, mut rx) = driver();
        d.probe_selection = Some(selection.clone());
        d.binding = Some(serde_json::from_value(value).unwrap());
        d.scope = Some("scope".into());
        d.fence("finalizing");
        d.last_send_id = Some("h:old-send".into());
        d.last_probe_id = Some("h:new-probe".into());
        d.probe_active = active.then(|| "h:new-probe".into());
        d.frame(br#"{"jsonrpc":"2.0","id":"h:cancel-old","method":"connect/cancel","params":{"scope":"scope","send_request_id":"h:old-send"}}"#);
        assert!(!*d.plain_cancel.borrow());
        assert!(selection.lifetime.current(selection.binding.expires_at_ms));
        assert_eq!(d.probe_active.as_deref(), active.then_some("h:new-probe"));
        let _ = drain(&mut rx);
        d.frame(br#"{"jsonrpc":"2.0","id":"h:cancel-current","method":"connect/cancel","params":{"scope":"scope","send_request_id":"h:new-probe"}}"#);
        assert!(*d.plain_cancel.borrow());
        assert!(!selection.lifetime.current(selection.binding.expires_at_ms));
        assert!(
            drain(&mut rx)
                .iter()
                .any(|row| row["id"] == "h:cancel-current"
                    && row["result"]["status"]
                        == if active {
                            "cancellation_requested"
                        } else {
                            "not_current"
                        })
        );
    }
}
