//! Engine guard controls, not native receipt or installed-flow acceptance.
//! No fake executor is installed in the production Ready path.
use super::*;
use crate::service_collection::{
    CollectionCall, CollectionCursor, CollectionLimits, CollectionProgram,
    RequiredCollectionOperation,
};
struct Never;
impl CollectionProgram for Never {
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("must not create a cursor")
    }
}
fn requirement(id: &str) -> RequiredCollectionOperation {
    RequiredCollectionOperation::new(
        id.into(),
        json!({"repository":id}),
        vec!["read".into()],
        Arc::new(Never),
    )
    .unwrap()
}
fn call(arguments: Value) -> CollectionCall {
    CollectionCall::new("read".into(), arguments).unwrap()
}
// Ledger controls below do not manufacture native completion. The production
// mint call remains after the real Ready disclosure boundary.
#[test]
fn read_handle_survives_intervening_pages_but_has_one_use_and_shared_budgets() {
    let r = requirement("one");
    let config =
        meerkat_core::McpServerConfig::stdio("one", "/bin/false", vec![], Default::default());
    let mut budget = Budget::new(CollectionLimits::new(3, 4096, 4096).unwrap());
    let first = CollectionCall::read_only("read".into(), json!({"page":1})).unwrap();
    let digest = budget.reserve(&r, &first).unwrap();
    let handle = budget
        .completed_read("source", &r, &config, 7, &first)
        .unwrap();
    budget.reserve(&r, &call(json!({"page":2}))).unwrap();
    assert_eq!(
        budget.reserve(&r, &first).unwrap_err().status,
        CollectionStatus::ReplayRefused
    );
    let copy = handle.clone();
    let again = budget.take_read(handle, "source", &r, &config, 7).unwrap();
    assert!(again.read_only);
    assert_eq!(again.arguments, first.arguments);
    assert_eq!(again.operation, first.operation);
    assert_eq!(budget.reserve_call(&r, &again, true).unwrap(), digest);
    assert_eq!(budget.calls, 3);
    assert_eq!(
        budget
            .take_read(copy, "source", &r, &config, 7)
            .err()
            .unwrap()
            .status,
        CollectionStatus::ReplayRefused
    );
    let next = budget
        .completed_read("source", &r, &config, 7, &again)
        .unwrap();
    let next_copy = next.clone();
    let next_call = budget.take_read(next, "source", &r, &config, 7).unwrap();
    assert_eq!(
        budget
            .reserve_call(&r, &next_call, true)
            .unwrap_err()
            .status,
        CollectionStatus::BudgetExceeded
    );
    assert!(
        budget
            .take_read(next_copy, "source", &r, &config, 7)
            .is_err(),
        "budget refusal cannot recycle a consumed observation"
    );
}
#[test]
fn read_handles_refuse_foreign_attempt_source_requirement_resource_config_and_generation() {
    let r = requirement("one");
    let other = requirement("other");
    let config =
        meerkat_core::McpServerConfig::stdio("one", "/bin/false", vec![], Default::default());
    let mut changed = config.clone();
    changed.name = "changed".into();
    let mut foreign_resource = requirement("one");
    foreign_resource.resource = json!({"repository":"foreign"});
    let mut budget = Budget::new(CollectionLimits::new(8, 4096, 4096).unwrap());
    let read = CollectionCall::read_only("read".into(), json!({})).unwrap();
    budget.reserve(&r, &read).unwrap();
    let handle = budget
        .completed_read("source", &r, &config, 9, &read)
        .unwrap();
    for (source, row, selected, generation) in [
        ("foreign", &r, &config, 9),
        ("source", &other, &config, 9),
        ("source", &foreign_resource, &config, 9),
        ("source", &r, &changed, 9),
        ("source", &r, &config, 10),
    ] {
        assert!(
            budget
                .take_read(handle.clone(), source, row, selected, generation)
                .is_err()
        );
    }
    let mut foreign = Budget::new(CollectionLimits::new(8, 4096, 4096).unwrap());
    assert!(
        foreign
            .take_read(handle.clone(), "source", &r, &config, 9)
            .is_err()
    );
    assert!(budget.take_read(handle, "source", &r, &config, 9).is_ok());
    assert!(
        budget
            .completed_read("source", &r, &config, 9, &call(json!({})))
            .is_none(),
        "ordinary results cannot become enforced reads"
    );
}
#[test]
fn read_reobservation_spends_argument_bytes_and_keeps_observed_result_overflow() {
    let r = requirement("one");
    let config =
        meerkat_core::McpServerConfig::stdio("one", "/bin/false", vec![], Default::default());
    let mut budget = Budget::new(CollectionLimits::new(8, 2, 2).unwrap());
    let read = CollectionCall::read_only("read".into(), json!({})).unwrap();
    budget.reserve(&r, &read).unwrap();
    let handle = budget
        .completed_read("source", &r, &config, 1, &read)
        .unwrap();
    let call = budget.take_read(handle, "source", &r, &config, 1).unwrap();
    assert_eq!(
        budget.reserve_call(&r, &call, true).unwrap_err().status,
        CollectionStatus::BudgetExceeded
    );
    assert_eq!(budget.calls, 1);
    assert_eq!(budget.arguments, 2);
    assert_eq!(
        budget.result(3).unwrap_err().status,
        CollectionStatus::BudgetExceeded
    );
    assert_eq!(budget.results, 3);
}
#[test]
fn replay_guard_is_per_exact_requirement_operation_and_canonical_arguments() {
    let mut budget = Budget::new(CollectionLimits::new(8, 4096, 4096).unwrap());
    let a = requirement("one");
    let b = requirement("two");
    let first = call(serde_json::from_str(r#"{"page":1,"nested":{"a":1,"b":2}}"#).unwrap());
    let reordered = call(serde_json::from_str(r#"{"nested":{"b":2,"a":1},"page":1}"#).unwrap());
    let digest = budget.reserve(&a, &first).unwrap();
    assert_eq!(
        budget.reserve(&a, &reordered).unwrap_err().status,
        CollectionStatus::ReplayRefused
    );
    assert_eq!(budget.calls, 1);
    // Distinct required rows remain distinct even when their tool arguments
    // happen to be equal. Their resource/program assessments cannot be shared.
    assert_eq!(budget.reserve(&b, &reordered).unwrap(), digest);
    assert!(budget.reserve(&a, &call(json!({"page":2}))).is_ok());
    assert_eq!(budget.calls, 3);
}
#[test]
fn finite_ceiling_and_total_budgets_refuse_before_another_native_attempt() {
    let r = requirement("one");
    let mut budget = Budget::new(CollectionLimits::new(1, 2, 8).unwrap());
    let denied = CollectionCall::new("write".into(), json!({})).unwrap();
    assert_eq!(
        budget.reserve(&r, &denied).unwrap_err().status,
        CollectionStatus::ProgramRefused
    );
    assert_eq!(budget.calls, 0);
    assert!(budget.reserve(&r, &call(json!({}))).is_ok());
    assert_eq!(
        budget
            .reserve(&r, &call(json!({"next":1})))
            .unwrap_err()
            .status,
        CollectionStatus::BudgetExceeded
    );
    assert_eq!(budget.calls, 1);
    assert_eq!(budget.arguments, 2);
    assert!(budget.result(8).is_ok());
    assert_eq!(
        budget.result(1).unwrap_err().status,
        CollectionStatus::BudgetExceeded
    );
    assert_eq!(
        budget.results, 9,
        "oversized observed result is retained as a failed budget observation"
    );
}
#[test]
fn zero_call_verified_cannot_borrow_another_rows_credit() {
    assert_eq!(
        finish_gate(ResourceAssessment::Verified, &[])
            .unwrap_err()
            .status,
        CollectionStatus::ProgramRefused
    );
    assert!(finish_gate(ResourceAssessment::Unknown, &[]).is_ok());
    assert!(finish_gate(ResourceAssessment::NotVerified, &[]).is_ok());
}
#[test]
fn callback_panic_is_typed_and_does_not_reset_partial_attempt_budget() {
    let mut budget = Budget::new(CollectionLimits::new(2, 4096, 4096).unwrap());
    let r = requirement("one");
    let selected = call(json!({"page":1}));
    budget.reserve(&r, &selected).unwrap();
    let error =
        callback::<()>(|| panic!("untrusted diagnostic must not enter a report")).unwrap_err();
    assert_eq!(error.status, CollectionStatus::ProgramPanicked);
    assert!(error.error.is_none());
    assert_eq!(
        budget.reserve(&r, &selected).unwrap_err().status,
        CollectionStatus::ReplayRefused
    );
}
#[test]
fn a_fresh_local_budget_does_not_authorize_reentry_of_a_retained_source() {
    let r = requirement("one");
    let selected = call(json!({"page":1}));
    let limits = CollectionLimits::new(1, 4096, 4096).unwrap();
    let mut a = Budget::new(limits);
    let mut b = Budget::new(limits);
    assert_eq!(
        a.reserve(&r, &selected).unwrap(),
        b.reserve(&r, &selected).unwrap()
    );
    assert_eq!(a.calls, 1);
    assert_eq!(b.calls, 1);
    assert_eq!(
        serde_json::to_value(ReusableSourceCoverage::UnqualifiedCredentialContinuity).unwrap(),
        json!("unqualified_credential_continuity")
    );
    let mut admitted = BTreeSet::new();
    assert!(admit_source(&mut admitted, "source-one").is_ok());
    assert_eq!(
        admit_source(&mut admitted, "source-one")
            .unwrap_err()
            .status,
        CollectionStatus::AttemptAlreadyObserved
    );
    assert!(admit_source(&mut admitted, "source-two").is_ok());
    // A fresh budget does not reset this retained Ready admission fence.
    // A new Ready requires another actual native finalization handoff.
}

#[test]
fn collection_preserves_native_unknown_failure_refusal_and_cleanup_distinctions() {
    assert_eq!(
        stopped_status(&ProbeStatus::OutcomeUnknown),
        CollectionStatus::OutcomeUnknown
    );
    assert_eq!(
        stopped_status(&ProbeStatus::ToolFailed),
        CollectionStatus::ToolFailed
    );
    assert_eq!(
        stopped_status(&ProbeStatus::NativeRefused),
        CollectionStatus::NativeRefused
    );
    assert_eq!(
        stopped_status(&ProbeStatus::Cancelled),
        CollectionStatus::Cancelled
    );
    assert_eq!(
        stopped_status(&ProbeStatus::CleanupUnconfirmed),
        CollectionStatus::CleanupUnconfirmed
    );
    assert_eq!(
        stopped_status(&ProbeStatus::ToolCompleted),
        CollectionStatus::NativeStopped
    );
}

#[test]
fn exact_original_host_arc_is_required_even_for_an_equal_realm_login_projection() {
    use crate::service_probe::tests::{binding, selected};
    let directory = tempfile::tempdir().unwrap();
    let a = binding(directory.path());
    let mut b = binding(directory.path());
    b.expires_at_ms = a.expires_at_ms;
    let original = Arc::new(ServiceProbeSelection::new(a, vec![selected("one")]).unwrap());
    let same = original.clone();
    let replaced = Arc::new(ServiceProbeSelection::new(b, vec![selected("one")]).unwrap());
    assert!(require_original_host(&original.host, &same.host).is_ok());
    assert!(require_original_host(&original.host, &replaced.host).is_err());
}
#[tokio::test]
async fn failed_collection_retires_actual_original_lifetime_without_claiming_cleanup() {
    use crate::service_probe::tests::{binding, selected};
    let directory = tempfile::tempdir().unwrap();
    let host =
        ServiceProbeSelection::new(binding(directory.path()), vec![selected("one")]).unwrap();
    retire_failed(&host, None);
    assert!(host.lifetime.current(host.binding.expires_at_ms));
    let error = api::refused();
    retire_failed(&host, Some(&error));
    assert!(!host.lifetime.current(host.binding.expires_at_ms));
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        host.lifetime.closed(host.binding.expires_at_ms),
    )
    .await
    .unwrap();
    assert!(
        host.lifetime
            .join(meerkat_authorization::publication::LocalAuthorizationPublication::new())
            .is_err()
    );
    // This is the actual admission lifetime only. The caller still retains
    // the original ConnectError; retirement is not physical cleanup evidence.
    drop(error);
}

#[tokio::test]
async fn actual_native_config_generation_refuses_changed_then_restored_descriptor_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    let state = directory.path().join("config_state.json");
    let runtime = || {
        meerkat_core::ConfigRuntime::new(
            Arc::new(meerkat_core::FileConfigStore::new(
                config.clone(),
                meerkat_models::canonical(),
            )),
            state.clone(),
        )
    };
    let first = runtime();
    let target = meerkat_core::McpRealmPersistTarget::new(&first);
    let server = |name: &str| {
        meerkat_core::McpServerConfig::stdio(name, "/bin/false", vec![], Default::default())
    };
    target.add(server("selected"), Some(0)).await.unwrap();
    let before = target.servers().await.unwrap();
    let mut observed = None;
    require_generation(&mut observed, before.1).unwrap();
    target
        .add(server("temporary"), Some(before.1))
        .await
        .unwrap();
    let changed = target.servers().await.unwrap();
    target.remove("temporary", Some(changed.1)).await.unwrap();
    // Reopen the actual native owner, rather than supplying a fabricated epoch.
    let reopened = runtime();
    let restored = meerkat_core::McpRealmPersistTarget::new(&reopened)
        .servers()
        .await
        .unwrap();
    assert_eq!(before.0, restored.0);
    assert_ne!(before.1, restored.1);
    assert!(require_generation(&mut observed, restored.1).is_err());
    assert_eq!(observed, Some(before.1));
}

#[test]
fn partial_correlation_does_not_promote_unconfirmed_close_or_disclose_a_result() {
    // Projection-only regression, not a fabricated native receipt.
    let input = meerkat_core::InputId::new();
    let run = meerkat_core::RunId::new();
    let mut report = ServiceProbeReport {
        profile_id: "row".into(),
        application_id: "app".into(),
        instance_id: "instance".into(),
        viewer_id: "viewer".into(),
        login_scope_id: "login".into(),
        origin: "https://invalid.example".into(),
        realm: "realm".into(),
        session_id: SessionId::new(),
        job_id: "job".into(),
        server: "server".into(),
        agent: "agent".into(),
        operation: "read".into(),
        resource: json!({"repository":"one"}),
        status: ProbeStatus::CleanupUnconfirmed,
        input_id: None,
        run_id: None,
        tool_call_id: None,
        result_sha256: None,
        installed_connection_sha256: None,
        dispatch_outcome: None,
        native_terminal_committed: false,
        native_close_joined: false,
    };
    PartialCall {
        input: input.clone(),
        run: Some(run.clone()),
        call: Some("call".into()),
        outcome: Some(LiveBridgeEffectOutcome::Committed),
    }
    .retain(&mut report);
    assert_eq!(report.input_id, Some(input));
    assert_eq!(report.run_id, Some(run));
    assert_eq!(report.tool_call_id.as_deref(), Some("call"));
    assert_eq!(
        report.dispatch_outcome,
        Some(LiveBridgeEffectOutcome::Committed)
    );
    assert!(matches!(report.status, ProbeStatus::CleanupUnconfirmed));
    assert!(
        !report.native_terminal_committed
            && !report.native_close_joined
            && report.result_sha256.is_none()
    );
}

#[test]
fn accepted_input_only_projection_does_not_invent_run_call_or_terminal_evidence() {
    // Projection-only control. Installed cancellation/output-loss controls
    // obtain the identity from a real accepted second native input.
    let input = meerkat_core::InputId::new();
    let mut report = ServiceProbeReport {
        profile_id: "row".into(),
        application_id: "app".into(),
        instance_id: "instance".into(),
        viewer_id: "viewer".into(),
        login_scope_id: "login".into(),
        origin: "https://invalid.example".into(),
        realm: "realm".into(),
        session_id: SessionId::new(),
        job_id: "job".into(),
        server: "server".into(),
        agent: "agent".into(),
        operation: "read".into(),
        resource: json!({"repository":"one"}),
        status: ProbeStatus::Cancelled,
        input_id: None,
        run_id: None,
        tool_call_id: None,
        result_sha256: None,
        installed_connection_sha256: None,
        dispatch_outcome: None,
        native_terminal_committed: false,
        native_close_joined: true,
    };
    PartialCall {
        input: input.clone(),
        run: None,
        call: None,
        outcome: None,
    }
    .retain(&mut report);
    assert_eq!(report.input_id, Some(input));
    assert!(report.run_id.is_none() && report.tool_call_id.is_none());
    assert!(report.dispatch_outcome.is_none() && report.result_sha256.is_none());
    assert!(!report.native_terminal_committed && report.native_close_joined);
    assert!(matches!(report.status, ProbeStatus::Cancelled));
}
