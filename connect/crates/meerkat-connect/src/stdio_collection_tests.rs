//! Driver/selection controls only. No synthetic Ready, native receipt or result
//! is supplied. Deferred workers below exercise transport custody, not tool proof.
use super::*;
use crate::service_collection::{
    CollectionCursor, CollectionLimits, CollectionProgram, CollectionStatus, CollectionTarget,
    ObservedSelectionFreshness, RequiredCollectionOperation, RequiredServiceCollection,
    ReusableSourceCoverage, ServiceCollectionReport, ServiceCollectionSelection,
};
use crate::service_probe::{
    ServiceSelection,
    tests::{binding as host_binding, selected},
};
struct Never;
impl CollectionProgram for Never {
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("driver control cannot supply native results")
    }
}
fn selected_binding(root: &std::path::Path) -> (Value, Arc<ServiceCollectionSelection>) {
    selected_binding_with_services(root, false)
}
fn selected_binding_with_services(
    root: &std::path::Path,
    multiple: bool,
) -> (Value, Arc<ServiceCollectionSelection>) {
    let host = host_binding(root);
    let mut value = generic_service_binding(root);
    value["protocol_version"] = json!(7);
    value["application"] = json!({"application_id":host.application_id,"instance_id":host.instance_id,"origin":host.origin});
    value["viewer"] = json!({"principal_id":host.viewer_id,"login_scope_id":host.login_scope_id,"expires_at_ms":host.expires_at_ms});
    value["realm"] = json!({"realm_id":host.realm.realm.as_str(),"state_root":host.realm.state_root,
        "context_root":host.context_root,"user_config_root":host.user_config_root});
    let source = RequiredServiceCollection::new(
        "source-one".into(),
        "github".into(),
        "enterprise".into(),
        "agent".into(),
        vec![
            RequiredCollectionOperation::new(
                "repository-read".into(),
                json!({"repository":"owner/one"}),
                vec!["read".into()],
                Arc::new(Never),
            )
            .unwrap(),
        ],
        CollectionLimits::new(4, 4096, 4096).unwrap(),
    )
    .unwrap();
    let selection = if multiple {
        let next = RequiredServiceCollection::new(
            "source-two".into(),
            "selected-python".into(),
            "python".into(),
            "agent".into(),
            vec![
                RequiredCollectionOperation::new(
                    "repository-read".into(),
                    json!({"repository":"owner/two"}),
                    vec!["read_repository".into()],
                    Arc::new(Never),
                )
                .unwrap(),
            ],
            CollectionLimits::new(4, 4096, 4096).unwrap(),
        )
        .unwrap();
        ServiceCollectionSelection::for_services(
            host,
            vec![("first".into(), vec![source]), ("next".into(), vec![next])],
        )
    } else {
        ServiceCollectionSelection::new(host, vec![source])
    };
    (value, Arc::new(selection.unwrap()))
}
fn bind(
    root: &std::path::Path,
) -> (
    Driver,
    mpsc::Receiver<OutputFrame>,
    Arc<ServiceCollectionSelection>,
) {
    bind_services(root, false)
}
fn bind_services(
    root: &std::path::Path,
    multiple: bool,
) -> (
    Driver,
    mpsc::Receiver<OutputFrame>,
    Arc<ServiceCollectionSelection>,
) {
    let (value, selection) = selected_binding_with_services(root, multiple);
    let (mut d, rx) = driver();
    d.collection_selection = Some(selection.clone());
    d.binding = Some(serde_json::from_value(value).unwrap());
    d.initialize_id = Some("h:init".into());
    d.scope = Some("scope".into());
    (d, rx, selection)
}
// Body-free projection input for transport tests only, with no known native
// input/run/call/result and no resource assessment or granted capability.
fn unobserved_report(status: CollectionStatus) -> ServiceCollectionReport {
    ServiceCollectionReport {
        source_id: "source-one".into(),
        attempt_id: "projection-only".into(),
        application_id: "application".into(),
        instance_id: "one".into(),
        viewer_id: "viewer".into(),
        login_scope_id: "login".into(),
        origin: "https://host.invalid".into(),
        realm: "private-probe".into(),
        session_id: meerkat_core::SessionId::new(),
        job_id: "projection-only".into(),
        family: "github".into(),
        profile: "enterprise".into(),
        agent: "agent".into(),
        status,
        requirements: vec![],
        observed_selection: ObservedSelectionFreshness::NotObserved,
        native_own_config_generation: None,
        reusable_source_coverage: ReusableSourceCoverage::UnqualifiedCredentialContinuity,
        limits: CollectionLimits::new(1, 1, 1).unwrap(),
        total_calls: 0,
        total_argument_bytes: 0,
        total_result_bytes: 0,
    }
}
#[test]
fn v7_requires_exact_original_host_and_refuses_wire_authority_fields() {
    let root = tempfile::tempdir().unwrap();
    let (value, selection) = selected_binding(root.path());
    let parsed: Initialize = serde_json::from_value(value.clone()).unwrap();
    let (mut d, _) = driver();
    assert!(parsed.valid());
    assert!(!d.probe_initialization_matches(&parsed));
    d.collection_selection = Some(selection.clone());
    assert!(d.probe_initialization_matches(&parsed));
    assert!(Arc::ptr_eq(&d.selected_host().unwrap(), &selection.host));
    for pointer in [
        "/application/application_id",
        "/application/instance_id",
        "/application/origin",
        "/viewer/login_scope_id",
        "/viewer/principal_id",
        "/realm/realm_id",
        "/realm/context_root",
        "/realm/state_root",
        "/realm/user_config_root",
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("other");
        assert!(!d.probe_initialization_matches(&serde_json::from_value(changed).unwrap()));
    }
    let mut changed = value.clone();
    changed["viewer"]["expires_at_ms"] = json!(selection.host.binding.expires_at_ms + 1);
    assert!(!d.probe_initialization_matches(&serde_json::from_value(changed).unwrap()));
    for field in [
        "collections",
        "profiles",
        "caller",
        "program",
        "connector_auth",
    ] {
        let mut changed = value.clone();
        changed[field] = json!({});
        assert!(serde_json::from_value::<Initialize>(changed).is_err());
    }
    for field in [
        "operation",
        "arguments",
        "resource",
        "program",
        "receipt",
        "profile_id",
        "caller",
        "grant",
    ] {
        let mut call = json!({"scope":"scope","job_id":"job","source_id":"source-one"});
        call[field] = json!({});
        assert!(serde_json::from_value::<Collect>(call).is_err());
    }
    d.probe_selection = Some(Arc::new(
        crate::service_probe::ServiceProbeSelection::new(
            host_binding(root.path()),
            vec![selected("one")],
        )
        .unwrap(),
    ));
    assert!(!d.probe_initialization_matches(&parsed));
    assert!(d.selected_host().is_none());
}
#[test]
fn v7_without_actual_ready_cannot_dispatch_or_mark_source_attempted() {
    let root = tempfile::tempdir().unwrap();
    let (mut d, mut rx, _) = bind(root.path());
    for (id, source) in [("h:one", "source-one"), ("h:foreign", "foreign")] {
        d.frame(
            &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":"connect/collect",
            "params":{"scope":"scope","job_id":"job","source_id":source}}))
            .unwrap(),
        );
    }
    assert_eq!(d.workers, 0);
    assert!(d.tasks.is_empty() && d.collected.is_empty());
    assert!(
        drain(&mut rx)
            .iter()
            .all(|row| row["error"]["data"]["code"] == "collection_unavailable")
    );
    let state = d.state();
    assert_eq!(state["can_collect"], false);
    assert_eq!(state["collection_sources"], json!(["source-one"]));
    assert_eq!(
        state["reusable_source_coverage"],
        "unqualified_credential_continuity"
    );
    assert!(state.get("can_probe").is_none() && state.get("probe_profiles").is_none());
}
#[test]
fn new_collect_method_and_state_do_not_expand_v1_through_v6() {
    let root = tempfile::tempdir().unwrap();
    for version in 1..=6 {
        let (mut d, mut rx) = driver();
        let mut value = generic_service_binding(root.path());
        value["protocol_version"] = json!(if version == 3 { 3 } else { 5 });
        let mut parsed: Initialize = serde_json::from_value(value).unwrap();
        parsed.protocol_version = version;
        d.binding = Some(parsed);
        d.scope = Some("scope".into());
        let before = d.state();
        d.frame(br#"{"jsonrpc":"2.0","id":"h:collect","method":"connect/collect","params":{"scope":"scope","job_id":"job","source_id":"source-one"}}"#);
        assert_eq!(before, d.state());
        assert_eq!(drain(&mut rx)[0]["error"]["code"], -32601);
        assert!(d.state().get("can_collect").is_none());
        assert_eq!(d.service_availability_kind(), "service_probe_state");
    }
}
#[test]
fn stale_setup_or_collection_ids_cannot_retire_a_new_collection_lineage() {
    for active in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut d, mut rx, selection) = bind(root.path());
        d.fence("finalizing");
        d.last_send_id = Some("h:setup".into());
        d.last_collection_id = Some("h:new".into());
        d.collection_active = active.then(|| "h:new".into());
        for (request, id) in [("h:stop-setup", "h:setup"), ("h:stop-old", "h:old")] {
            d.frame(
                &serde_json::to_vec(
                    &json!({"jsonrpc":"2.0","id":request,"method":"connect/cancel",
                "params":{"scope":"scope","send_request_id":id}}),
                )
                .unwrap(),
            );
            assert!(!*d.plain_cancel.borrow());
            assert!(
                selection
                    .host
                    .lifetime
                    .current(selection.host.binding.expires_at_ms)
            );
        }
        let _ = drain(&mut rx);
        d.frame(br#"{"jsonrpc":"2.0","id":"h:stop-new","method":"connect/cancel","params":{"scope":"scope","send_request_id":"h:new"}}"#);
        assert!(*d.plain_cancel.borrow());
        assert!(
            !selection
                .host
                .lifetime
                .current(selection.host.binding.expires_at_ms)
        );
        assert!(drain(&mut rx).iter().any(|row| row["id"] == "h:stop-new"
            && row["result"]["status"]
                == if active {
                    "cancellation_requested"
                } else {
                    "not_current"
                }));
    }
}
#[test]
fn latest_setup_cancel_still_retires_commissioning_before_any_collection() {
    let root = tempfile::tempdir().unwrap();
    let (mut d, mut rx, selection) = bind(root.path());
    d.fence("finalizing");
    d.probe_commissioning = true;
    d.last_send_id = Some("h:setup".into());
    assert!(d.commissioning_current());
    d.frame(br#"{"jsonrpc":"2.0","id":"h:stop","method":"connect/cancel","params":{"scope":"scope","send_request_id":"h:setup"}}"#);
    assert!(!d.commissioning_current());
    assert!(
        !selection
            .host
            .lifetime
            .current(selection.host.binding.expires_at_ms)
    );
    assert!(
        drain(&mut rx)
            .iter()
            .any(|row| row["result"]["status"] == "not_current")
    );
}
#[tokio::test]
async fn v7_host_control_reaches_actual_private_admission_and_prompt_bridge() {
    use meerkat_core::AdministrationEntry;
    let root = tempfile::tempdir().unwrap();
    let (mut d, mut rx, selection) = bind(root.path());
    let (mut bridge, mut prompts, pipe_stop) = PromptBridge::channel();
    bridge.probe_lifetime = Some((
        selection.host.lifetime.clone(),
        selection.host.binding.expires_at_ms,
    ));
    let admission = crate::terminal_connector::private_service::Admission::new(
        selection.host.binding.expires_at_ms,
        d.plain_cancel.subscribe(),
    )
    .with_host(Some(&selection.host));
    assert!(admission.enter().is_ok());
    let ask = bridge.ask(json!({"kind":"service_secret"}));
    tokio::pin!(ask);
    assert!(futures_util::poll!(ask.as_mut()).is_pending());
    let prompt = prompts.recv().await.unwrap();
    selection.control().retire();
    let answered = tokio::time::timeout(Duration::from_secs(2), ask).await;
    assert!(answered.unwrap().is_err());
    assert!(prompt.response.is_closed());
    assert!(!*pipe_stop.borrow());
    assert!(admission.enter().is_err());
    d.frame(br#"{"jsonrpc":"2.0","id":"h:collect","method":"connect/collect","params":{"scope":"scope","job_id":"job","source_id":"source-one"}}"#);
    assert!(*d.plain_cancel.borrow() && *d.stop.borrow());
    let _ = drain(&mut rx);
}
#[derive(Debug)]
struct HeldError {
    _custody: Arc<()>,
}
impl std::fmt::Display for HeldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "retained control error")
    }
}
impl std::error::Error for HeldError {}
#[tokio::test]
async fn close_joins_retained_collection_worker_and_preserves_cleanup_error_custody() {
    let root = tempfile::tempdir().unwrap();
    let (mut d, mut rx, selection) = bind(root.path());
    d.fence("finalizing");
    d.private_close_started = true;
    d.collection_active = Some("h:collect".into());
    d.last_collection_id = Some("h:collect".into());
    d.workers = 1;
    let custody = Arc::new(());
    let weak = Arc::downgrade(&custody);
    let (release, released) = oneshot::channel();
    d.tasks.push(
        async move {
            let _ = released.await;
            Work::Collected(
                "h:collect".into(),
                crate::session::service_probe::CollectionExecution {
                    report: unobserved_report(CollectionStatus::CleanupUnconfirmed),
                    error: Some(ConnectError::native(
                        "Retain control cleanup",
                        HeldError { _custody: custody },
                    )),
                },
            )
        }
        .boxed(),
    );
    d.frame(br#"{"jsonrpc":"2.0","id":"h:close","method":"connect/close","params":{"target":{"scope":"scope"}}}"#);
    d.settle();
    let pending_workers = d.workers;
    let pending_closed = d.closed;
    let early = drain(&mut rx);
    let _ = release.send(());
    while let Some(work) = d.tasks.next().await {
        d.work(work);
        d.settle();
    }
    let rows = drain(&mut rx);
    assert_eq!(pending_workers, 1);
    assert!(!pending_closed);
    assert!(
        !early
            .iter()
            .any(|row| row["id"] == "h:close" && row.get("result").is_some())
    );
    assert!(
        !selection
            .host
            .lifetime
            .current(selection.host.binding.expires_at_ms)
    );
    assert_eq!(d.workers, 0);
    assert!(d.closed && d.failed && d.close_failure.is_some());
    assert!(rows.iter().any(|row| row["id"] == "h:collect"
        && row["result"]["report"]["status"] == "cleanup_unconfirmed"));
    assert!(rows.iter().any(
        |row| row["id"] == "h:close" && row["error"]["data"]["code"] == "shutdown_unconfirmed"
    ));
    assert!(weak.upgrade().is_some());
    drop(d);
    assert!(weak.upgrade().is_none());
}
#[test]
fn retirement_preserves_unknown_and_partial_native_status_in_collection_projection() {
    for status in [
        CollectionStatus::OutcomeUnknown,
        CollectionStatus::NativeRefused,
        CollectionStatus::ToolFailed,
        CollectionStatus::CleanupUnconfirmed,
    ] {
        let root = tempfile::tempdir().unwrap();
        let (mut d, mut rx, selection) = bind(root.path());
        selection.control().retire();
        d.workers = 1;
        d.collection_active = Some("h:one".into());
        d.work(Work::Collected(
            "h:one".into(),
            crate::session::service_probe::CollectionExecution {
                report: unobserved_report(status),
                error: None,
            },
        ));
        let rows = drain(&mut rx);
        let result = rows.iter().find(|row| row["id"] == "h:one").unwrap();
        assert_eq!(
            result["result"]["report"]["status"],
            serde_json::to_value(status).unwrap()
        );
        assert_eq!(result["result"]["report"]["total_calls"], 0);
        assert_eq!(
            result["result"]["report"]["reusable_source_coverage"],
            "unqualified_credential_continuity"
        );
    }
}
#[tokio::test]
async fn actual_v7_pipe_eof_before_initialization_retires_the_selected_host() {
    let root = tempfile::tempdir().unwrap();
    let (_, selection) = selected_binding(root.path());
    let control = selection.control();
    let host = selection.host.clone();
    let (read, write) = tokio::io::duplex(128);
    drop(write);
    let mut owner = tokio::spawn(run_io_composed(
        read,
        tokio::io::sink(),
        None,
        Some(ServiceSelection::Collections(selection)),
    ));
    let result = tokio::time::timeout(Duration::from_secs(2), &mut owner).await;
    if result.is_err() {
        control.retire();
        owner.abort();
        let _ = owner.await;
        panic!("pipe-only owner did not join");
    }
    assert!(result.unwrap().unwrap().is_ok());
    assert!(!host.lifetime.current(host.binding.expires_at_ms));
}
#[tokio::test]
async fn output_loss_keeps_joined_worker_reports_and_original_error_on_v7_return() {
    let root = tempfile::tempdir().unwrap();
    let (mut d, rx, _) = bind(root.path());
    d.fence("finalizing");
    d.private_close_started = true;
    d.collection_active = Some("h:collect".into());
    d.last_collection_id = Some("h:collect".into());
    d.workers = 1;
    let custody = Arc::new(());
    let weak = Arc::downgrade(&custody);
    let (release, released) = oneshot::channel();
    let entered = Arc::new(AtomicUsize::new(0));
    let task_entered = entered.clone();
    d.tasks.push(
        async move {
            task_entered.store(1, Ordering::SeqCst);
            let _ = released.await;
            Work::Collected(
                "h:collect".into(),
                crate::session::service_probe::CollectionExecution {
                    report: unobserved_report(CollectionStatus::OutcomeUnknown),
                    error: Some(ConnectError::native(
                        "Retained original control error",
                        HeldError { _custody: custody },
                    )),
                },
            )
        }
        .boxed(),
    );
    {
        let next = d.tasks.next();
        tokio::pin!(next);
        let _ = futures_util::poll!(next.as_mut());
    }
    drop(rx);
    d.output(json!({"kind":"force_actual_output_loss"}));
    d.settle();
    let pending = (d.workers, d.closed, entered.load(Ordering::SeqCst));
    let _ = release.send(());
    while let Some(work) = d.tasks.next().await {
        d.work(work);
        d.settle();
    }
    let reports = std::mem::take(&mut d.collection_reports);
    let outcome = collection::retain_failure(true, reports, Err(d.close_failure.take().unwrap()));
    drop(d);
    assert_eq!(pending, (1, false, 1));
    assert!(weak.upgrade().is_some());
    let failure = outcome.unwrap_err();
    let ConnectError::Native { source, .. } = &failure else {
        panic!("typed retained failure");
    };
    let retained = source
        .downcast_ref::<crate::service_collection::ServiceCollectionFailure>()
        .unwrap();
    assert_eq!(retained.reports().len(), 1);
    assert_eq!(
        retained.reports()[0].status(),
        CollectionStatus::OutcomeUnknown
    );
    assert!(matches!(
        retained.original_error(),
        ConnectError::Native {
            operation: "Retained original control error",
            ..
        }
    ));
    assert!(!format!("{failure:?}").contains("source-one"));
    assert!(!failure.to_string().contains("source-one"));
    drop(failure);
    assert!(weak.upgrade().is_none());
    // Legacy errors keep their exact type/value and are not wrapped.
    assert!(matches!(
        collection::retain_failure(false, vec![], Err(ConnectError::ReviewCancelled)),
        Err(ConnectError::ReviewCancelled)
    ));
}

#[test]
fn next_service_cannot_reconstruct_ready_or_reset_retired_host() {
    let root = tempfile::tempdir().unwrap();
    let (mut d, mut rx, selection) = bind_services(root.path(), true);
    let payload = json!({"scope":"scope","service_id":"next"});
    for field in [
        "caller", "receipt", "grant", "family", "profile", "program", "binding", "job_id",
    ] {
        let mut value = payload.clone();
        value[field] = json!({});
        assert!(serde_json::from_value::<OpenCollectionService>(value).is_err());
    }
    d.fence("finalizing");
    d.private_close_started = true;
    d.plain_close_started = true;
    for (id, scope) in [("h:next", "scope"), ("h:foreign", "foreign")] {
        d.frame(
            &serde_json::to_vec(
                &json!({"jsonrpc":"2.0","id":id,"method":"connect/collection/open_next",
            "params":{"scope":scope,"service_id":"next"}}),
            )
            .unwrap(),
        );
    }
    assert!(
        d.tasks.is_empty() && d.probe_ready.is_none() && d.retained_collection_services.is_empty()
    );
    assert_eq!(d.collection_service_index, 0);
    assert!(!d.opening);
    selection.control().retire();
    d.frame(br#"{"jsonrpc":"2.0","id":"h:retired-next","method":"connect/collection/open_next","params":{"scope":"scope","service_id":"next"}}"#);
    assert!(d.tasks.is_empty() && d.retired && *d.plain_cancel.borrow());
    let rows = drain(&mut rx);
    for (id, code) in [
        ("h:next", "collection_unavailable"),
        ("h:foreign", "invalid_binding"),
        ("h:retired-next", "collection_unavailable"),
    ] {
        let reply = rows.iter().find(|row| row["id"] == id).unwrap();
        assert_eq!(reply["error"]["code"], -32010);
        assert_eq!(reply["error"]["data"]["code"], code);
    }
}

#[test]
fn next_setup_failure_preserves_first_fence_and_routes_to_transition_request() {
    let root = tempfile::tempdir().unwrap();
    let (mut d, mut rx, selection) = bind_services(root.path(), true);
    d.fence("finalizing");
    let first = serde_json::to_value(d.first_fence.as_ref().unwrap()).unwrap();
    assert_eq!(first["origin"], "other_native_transition");
    assert!(!*d.plain_cancel.borrow());
    assert!(
        selection
            .host
            .lifetime
            .current(selection.host.binding.expires_at_ms)
    );
    // Transport failure injection only: no Ready or native result is created.
    // Reproduce the input owner's reset after the real transition gate, then
    // deliver the real Work variant used when the new setup cannot open.
    d.retired = false;
    d.collection_service_index = 1;
    d.opening = true;
    d.collection_open_id = Some("h:next-open".into());
    let _ = drain(&mut rx);
    d.work(Work::Opened(Box::new(Err(OpenFailure::native(
        "native_auth_required",
        ConnectError::ReviewCancelled,
    )))));
    assert_eq!(
        serde_json::to_value(d.first_fence.as_ref().unwrap()).unwrap(),
        first
    );
    assert!(!d.opening && d.collection_open_id.is_none());
    assert!(d.closing && d.retired && d.failed && *d.plain_cancel.borrow());
    assert!(
        !selection
            .host
            .lifetime
            .current(selection.host.binding.expires_at_ms)
    );
    assert!(matches!(
        d.close_failure.as_ref(),
        Some(ConnectError::ReviewCancelled)
    ));
    assert!(d.probe_ready.is_none() && d.tasks.is_empty() && d.collection_reports.is_empty());
    let rows = drain(&mut rx);
    let replies = rows
        .iter()
        .filter(|row| row.get("id").is_some())
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["id"], "h:next-open");
    assert_eq!(replies[0]["error"]["data"]["code"], "native_auth_required");
    assert!(rows.iter().any(|row| row["params"]["first_fence"] == first));
}

#[test]
fn multi_service_state_exposes_only_the_selected_phase_and_keeps_legacy_shape() {
    let root = tempfile::tempdir().unwrap();
    let (single, _, _) = bind(root.path());
    let state = single.state();
    for field in ["service_ids", "service_id", "can_open_next_service"] {
        assert!(state.get(field).is_none());
    }
    let (mut multi, _, _) = bind_services(root.path(), true);
    for (index, id) in [(0, "first"), (1, "next")] {
        multi.collection_service_index = index;
        let state = multi.state();
        assert_eq!(state["service_ids"], json!(["first", "next"]));
        assert_eq!(state["service_id"], id);
        assert_eq!(state["can_open_next_service"], false);
        assert_eq!(state["can_collect"], false);
    }
    assert!(multi.probe_ready.is_none() && multi.tasks.is_empty());
}
#[test]
fn driver_commissioning_uses_only_the_current_preselected_service() {
    let root = tempfile::tempdir().unwrap();
    let (mut d, _) = driver();
    let make = |id: &str, profile: &str, op: &str| {
        RequiredServiceCollection::new(
            id.into(),
            "github".into(),
            profile.into(),
            "agent".into(),
            vec![
                RequiredCollectionOperation::new(
                    "read".into(),
                    json!({"repository":id}),
                    vec![op.into()],
                    Arc::new(Never),
                )
                .unwrap(),
            ],
            CollectionLimits::new(1, 1024, 1024).unwrap(),
        )
        .unwrap()
    };
    let selection = Arc::new(
        ServiceCollectionSelection::for_services(
            host_binding(root.path()),
            vec![
                ("official".into(), vec![make("one", "enterprise", "get_me")]),
                (
                    "python".into(),
                    vec![make("two", "python", "read_repository")],
                ),
            ],
        )
        .unwrap(),
    );
    d.collection_selection = Some(selection.clone());
    let first = d.selected_services().unwrap();
    d.collection_service_index = 1;
    let second = d.selected_services().unwrap();
    assert!(Arc::ptr_eq(first.host(), second.host()));
    assert!(Arc::ptr_eq(second.host(), &selection.host));
    let config = |op: &str| {
        serde_json::from_value(json!({"name":"selected","command":"/bin/false","args":[],"access":{"allowed_operations":[op]}})).unwrap()
    };
    let ops = |op: &str| [op.to_owned()].into_iter().collect();
    assert!(first.permits_review(
        "github",
        "enterprise",
        "agent",
        &ops("get_me"),
        &config("get_me")
    ));
    assert!(!second.permits_review(
        "github",
        "enterprise",
        "agent",
        &ops("get_me"),
        &config("get_me")
    ));
    assert!(second.permits_review(
        "github",
        "python",
        "agent",
        &ops("read_repository"),
        &config("read_repository")
    ));
    // This driver control selects no actual Ready and cannot run either tool.
    assert!(!d.collection_available());
    assert!(d.tasks.is_empty());
}
