//! Actual native issuance and retained source controls. No MCP process or input
//! is admitted here. The protected parent-review control lives with OperatorSetup.
use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::*;
use crate::original_ingress::tests::{claims, initialized, private_event, source};
use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
use meerkat_core::mcp_config::McpConnectorOperation;
use std::os::unix::fs::PermissionsExt;

fn directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn app() -> PrincipalRef {
    PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::ServiceAccount,
        "application",
        meerkat_core::TrustDomainId::new("toolkit-original-test").unwrap(),
    )
    .unwrap()
}
fn config() -> McpServerConfig {
    let mut config = McpServerConfig::stdio("mail", "not-executed", vec![], Default::default());
    let operation = McpConnectorOperation {
        required_scopes: ["mail.read".into()].into(),
        destination_origin: "https://mail.invalid".into(),
        read_only: true,
    };
    config.connector = Some(McpConnectorBinding {
        realm_id: "test-realm".into(),
        slot_id: "account-slot".into(),
        issuer: "https://issuer.invalid".into(),
        client: "reviewed-client".into(),
        resource: "https://mail.invalid".into(),
        strategy_id: "mail-oauth".into(),
        account: "provider-verified-account".into(),
        operations: [
            ("read-messages".into(), operation.clone()),
            ("list-labels".into(), operation),
        ]
        .into(),
    });
    config
}
fn selection(config: &McpServerConfig) -> ServiceMcpSelection {
    ServiceMcpSelection::connector(
        &meerkat::ConnectorMcpInstallation::from_config(config)
            .unwrap()
            .unwrap(),
    )
}
fn decision() -> PrivateServiceMcpAccessDecision {
    let old = super::super::tests::decision();
    PrivateServiceMcpAccessDecision {
        catalog: None,
        live_attachment: None,
        route: old.route,
        decision: old.decision,
        evidence: old.evidence,
        choice: old.choice,
        connection_revision: old.connection_revision,
        permission: PrivateServiceMcpPermission {
            target: old.permission.target,
            connection: old.permission.connection,
            operations: [
                Id::new("read-messages").unwrap(),
                Id::new("list-labels").unwrap(),
            ]
            .into(),
            resource: old.permission.resource,
            lifetime: old.permission.lifetime,
        },
    }
}
fn original(adapter: &PrivateOriginals, label: &str) -> OriginalWorkRef {
    OriginalWorkRef {
        authority: adapter.routes.get().unwrap()[0].source_authority.clone(),
        work: EvidenceId::new(label).unwrap(),
    }
}
async fn selected_adapter(
    owner: Arc<ToolkitOriginalOwner>,
    path: &std::path::Path,
    selected: &ServiceMcpSelection,
) -> Arc<PrivateOriginals> {
    selected_adapter_for_decision(owner, path, selected, &decision()).await
}
async fn selected_adapter_for_decision(
    owner: Arc<ToolkitOriginalOwner>,
    path: &std::path::Path,
    selected: &ServiceMcpSelection,
    decision: &PrivateServiceMcpAccessDecision,
) -> Arc<PrivateOriginals> {
    let mut adapter = crate::original_ingress::tests::private_originals(owner.clone(), path).await;
    let route = &mut Arc::get_mut(&mut adapter).unwrap().routes.get_mut().unwrap()[0];
    let operator = route.grant_caller.clone();
    route.grant_caller = app();
    let mut bounds = selected
        .restrictions(route, decision, owner.clock().now().unwrap().unix_ms)
        .unwrap();
    bounds.delegation_depth = DelegationDepth::remaining(1);
    let parent = owner
        .grants()
        .issue_root(
            &operator,
            EvidenceId::new("application-parent").unwrap(),
            app(),
            None,
            bounds,
        )
        .unwrap();
    route.application_parent = Some(vec![parent]);
    adapter
}
// Exercise the common issuance body using an actual fixture parent. Production
// enters only with ReviewedServiceMcpParent from the real operator review.
async fn apply(
    adapter: &PrivateOriginals,
    decision: PrivateServiceMcpAccessDecision,
    selected: ServiceMcpSelection,
) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
    let (_directory, lifetime) = crate::installed_host::operator_setup::test_service_lifetime();
    adapter
        .apply_service_access(
            AccessDecision::Mcp(decision),
            Some(selected.0),
            adapter.routes.get().unwrap()[0].application_parent.clone(),
            Some(Arc::downgrade(&lifetime)),
            None,
            // This mechanical fixture has no expiring Console action.
            Arc::new(|| Ok(())),
        )
        .await
}

#[tokio::test]
async fn application_policy_pending_record_keeps_exact_provenance_without_reissue() {
    // Exercises the same durable issuance owner, with actual native configured
    // parent and a seeded pending row. This is not acknowledgment-loss execution
    // or application authentication.
    let dir = directory();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let selected = selection(&config());
    let adapter = selected_adapter(owner, &dir.path().join("originals"), &selected).await;
    let route = adapter.routes.get().unwrap()[0].clone();
    let mut decision = decision();
    let evidence = ApplicationServiceEvidence {
        owner: route.grant_caller.clone(), deployment: Id::new(adapter.scope.owner.clone()).unwrap(),
        source_policy_sha256: "a".repeat(64),
        original: PrivateOriginalRef { route: route.id.clone(), original: EvidenceId::new("original-a").unwrap(), event_sha256: "b".repeat(64) },
        acquisition: StoreKey { realm: adapter.scope.realm.clone(), owner: adapter.scope.owner.clone(), id: "task".into() },
        instruction_sha256: "c".repeat(64),
        service_parent: None,
        tool_policy: None, installation_readback: None,
        provenance: serde_json::json!({"policy_decision":"actual-test-producer-decision"}).as_object().unwrap().clone(),
    };
    decision.evidence.sha256 = digest(&canonical_json(&evidence).unwrap());
    let reference = PrivateServiceGrantRef { route: decision.route.clone(), decision: decision.decision.clone(),
        decision_sha256: decision_digest(&AccessDecision::Mcp(decision.clone()), Some(&selected.0)).unwrap() };
    let key = adapter.access_key(&reference).unwrap();
    let record = AccessRecord { decision: AccessDecision::Mcp(decision.clone()), caller: route.grant_caller,
        parent: route.application_parent.clone().unwrap(), issuance: IssuanceObservation::PendingNativeIssuance,
        mcp: Some(selected.0.clone()), application_policy: Some(evidence.clone()) };
    let data = encode_access(&record).unwrap();
    assert_eq!(data.version, 4);
    adapter.store.admit(AdmissionRequest { key: key.clone(), admission_key: key.id.clone(), data,
        events: vec![], effects: vec![] }).await.unwrap();
    let before = std::fs::read(&path).unwrap();
    let (_host_dir, host) = crate::installed_host::operator_setup::test_service_lifetime();
    let replay = adapter.apply_service_access(AccessDecision::Mcp(decision.clone()), Some(selected.0.clone()),
        route.application_parent.clone(), Some(Arc::downgrade(&host)), Some(evidence.clone()),
        Arc::new(|| panic!("a saved uncertain attempt must not enter native again"))).await.unwrap();
    assert_eq!(replay.state, PrivateServiceGrantState::PendingReconciliation);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut other = evidence;
    other.provenance.insert("policy_decision".into(), serde_json::json!("different-decision"));
    assert!(matches!(adapter.apply_service_access(AccessDecision::Mcp(decision), Some(selected.0),
        route.application_parent, Some(Arc::downgrade(&host)), Some(other), Arc::new(|| Ok(()))).await,
        Err(PrivateOriginalError::Conflict)));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

async fn late_current_failure(error: PrivateOriginalError) {
    // Actual native parent and awaited canonical decision admission. The
    // currentness producer is mechanical; this is not full app-policy prepare
    // or native Console authentication. No native issue is seeded or mocked.
    let dir = directory();
    let path = dir.path().join("grants");
    let store_path = dir.path().join("originals");
    let selected = selection(&config());
    let adapter = selected_adapter(initialized(&path), &store_path, &selected).await;
    let decision = decision();
    let reference = PrivateServiceGrantRef { route: decision.route.clone(), decision: decision.decision.clone(),
        decision_sha256: decision_digest(&AccessDecision::Mcp(decision.clone()), Some(&selected.0)).unwrap() };
    let key = adapter.access_key(&reference).unwrap();
    assert!(adapter.store.get(&key).await.unwrap().is_none());
    let before = std::fs::read(&path).unwrap();
    let (_host_dir, host) = crate::installed_host::operator_setup::test_service_lifetime();
    let checked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_checked = checked.clone();
    let pending_key = key.id.clone();
    let refused = adapter.apply_service_access(AccessDecision::Mcp(decision.clone()), Some(selected.0.clone()),
        adapter.routes.get().unwrap()[0].application_parent.clone(), Some(Arc::downgrade(&host)), None,
        Arc::new(move || {
            // This callback runs in the entered worker only after the actual
            // store await. Observe that precise durable Pending attempt.
            let sql = rusqlite::Connection::open(&store_path).unwrap();
            let state: String = sql.query_row(
                "SELECT json_extract(data, '$.body.issuance.state') FROM toolkit_records WHERE id = ?1",
                [&pending_key], |row| row.get(0)).unwrap();
            assert_eq!(state, "pending_native_issuance");
            worker_checked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(error)
        })).await;
    assert!(matches!(refused, Err(actual) if actual == error));
    assert_eq!(checked.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&path).unwrap(), before, "late failure issues no native child");
    let stored = adapter.store.get(&key).await.unwrap().unwrap();
    let record = adapter.decode_access(&stored, &reference).unwrap();
    assert!(match error {
        PrivateOriginalError::Denied => matches!(record.issuance, IssuanceObservation::NativeRefused),
        _ => matches!(record.issuance, IssuanceObservation::PendingNativeIssuance),
    });
    let replay = adapter.apply_service_access(AccessDecision::Mcp(decision), Some(selected.0),
        adapter.routes.get().unwrap()[0].application_parent.clone(), Some(Arc::downgrade(&host)), None,
        Arc::new(|| panic!("a saved attempt never reruns policy or native issuance"))).await.unwrap();
    assert_eq!(replay.state, if error == PrivateOriginalError::Denied {
        PrivateServiceGrantState::GrantUnavailable
    } else { PrivateServiceGrantState::PendingReconciliation });
    assert_eq!(adapter.store.get(&key).await.unwrap().unwrap(), stored);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn late_policy_unavailable_keeps_actual_pending_attempt_and_never_reissues() {
    late_current_failure(PrivateOriginalError::Unavailable).await;
}

#[tokio::test]
async fn late_policy_denial_is_definite_and_never_reissues() {
    late_current_failure(PrivateOriginalError::Denied).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_caller_closes_entered_worker_but_retains_its_custody_until_settlement() {
    use std::sync::{Condvar, atomic::{AtomicBool, Ordering}};
    // The marker stands for owned invocation/policy custody. The actual
    // production callback owns native ReviewedEntry custody and reservation.
    // This control enters the real issuance worker and aborts its awaiting
    // caller at that boundary; it does not simulate an issued-grant rollback.
    struct Custody(Arc<AtomicBool>, Arc<tokio::sync::Notify>);
    impl Drop for Custody {
        fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); self.1.notify_one(); }
    }
    struct Close(Arc<AtomicBool>);
    impl Drop for Close {
        fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); }
    }
    let dir = directory();
    let path = dir.path().join("grants");
    let selected = selection(&config());
    let adapter = selected_adapter(initialized(&path), &dir.path().join("originals"), &selected).await;
    let before = std::fs::read(&path).unwrap();
    let (_host_dir, host) = crate::installed_host::operator_setup::test_service_lifetime();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let dropped = Arc::new(AtomicBool::new(false));
    let settled = Arc::new(tokio::sync::Notify::new());
    let open = Arc::new(AtomicBool::new(true));
    let worker_adapter = adapter.clone();
    let worker_selected = selected.clone();
    let worker_entered = entered.clone();
    let worker_release = release.clone();
    let worker_open = open.clone();
    let custody = Custody(dropped.clone(), settled.clone());
    let worker = tokio::spawn(async move {
        let _close = Close(worker_open.clone());
        worker_adapter.apply_service_access(AccessDecision::Mcp(decision()), Some(worker_selected.0),
            worker_adapter.routes.get().unwrap()[0].application_parent.clone(), Some(Arc::downgrade(&host)), None,
            Arc::new(move || {
                let _custody = &custody;
                worker_entered.notify_one();
                let (lock, wake) = &*worker_release;
                let mut released = lock.lock().unwrap();
                while !*released { released = wake.wait(released).unwrap(); }
                if worker_open.load(Ordering::SeqCst) { Ok(()) } else { Err(PrivateOriginalError::Denied) }
            })).await
    });
    let entered_result = tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified()).await;
    worker.abort();
    let cancelled = matches!(worker.await, Err(error) if error.is_cancelled());
    let closed = !open.load(Ordering::SeqCst);
    let held_after_drop = !dropped.load(Ordering::SeqCst);
    // Release and observe settlement before asserting any adverse condition,
    // including an entry timeout, so failure cannot strand a blocking worker.
    *release.0.lock().unwrap() = true;
    release.1.notify_one();
    let settlement = tokio::time::timeout(std::time::Duration::from_secs(10), settled.notified()).await;
    assert!(entered_result.is_ok());
    assert!(cancelled);
    assert!(closed);
    assert!(held_after_drop, "entered worker still owns custody");
    assert!(settlement.is_ok());
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let replay = apply(&adapter, decision(), selected).await.unwrap();
    assert_eq!(replay.state, PrivateServiceGrantState::PendingReconciliation);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
async fn bind(
    adapter: &PrivateOriginals,
    access: &PrivateServiceGrantRef,
    label: &str,
) -> (Arc<RetainedToolkitOriginal>, Vec<GrantLineageRef>) {
    let lineage = adapter
        .service_grant_lineage(access, &original(adapter, label))
        .await
        .unwrap();
    let mut candidate = claims(label, &lineage[0]).candidate().clone();
    candidate.authority_basis = WorkAuthorityBasis::GrantLineage {
        lineage: lineage.clone(),
    };
    let association = InputAuthorityAssociation::new(candidate).unwrap();
    let event = adapter.retain_event(private_event(label)).await.unwrap();
    (
        adapter
            .bind_access_source(&event, access, association.clone(), source(&association))
            .await
            .unwrap(),
        lineage,
    )
}

#[tokio::test]
async fn whole_set_replays_without_reissue_and_matches_only_its_access_bound_source() {
    let dir = directory();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let config = config();
    let selected = selection(&config);
    let adapter = selected_adapter(owner.clone(), &dir.path().join("originals"), &selected).await;
    let mut decision = decision();
    decision.permission.lifetime.scope = LifetimeScope::Task {
        task: Id::new("approved-b").unwrap(),
    };
    let observed = apply(&adapter, decision.clone(), selected.clone())
        .await
        .unwrap();
    assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
    let before = std::fs::read(&path).unwrap();
    let (retained, lineage) = bind(&adapter, &observed.grant, "approved-b").await;
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let now = owner.clock().now().unwrap().unix_ms;
    for raw in ["read-messages", "list-labels"] {
        assert!(
            adapter
                .check_service_mcp_operation(
                    retained.association(),
                    "mail",
                    raw,
                    &config.connector_approval(),
                    now
                )
                .is_ok()
        );
    }
    assert!(matches!(
        adapter
            .service_grant_lineage(&observed.grant, &original(&adapter, "old-a"))
            .await,
        Err(PrivateOriginalError::Denied)
    ));
    for (server, raw) in [("other", "read-messages"), ("mail", "write-message")] {
        assert!(matches!(
            adapter.check_service_mcp_operation(
                retained.association(),
                server,
                raw,
                &config.connector_approval(),
                now
            ),
            Err(PrivateOriginalError::Denied)
        ));
    }
    assert!(matches!(
        adapter.check_service_mcp_operation(
            &claims("unrelated", &lineage[0]),
            "mail",
            "read-messages",
            &config.connector_approval(),
            now
        ),
        Err(PrivateOriginalError::Unavailable)
    ));
    for field in [
        "realm",
        "slot",
        "issuer",
        "client",
        "resource",
        "strategy",
        "account",
        "scopes",
        "destination",
        "readonly",
    ] {
        let mut changed = config.clone();
        let binding = changed.connector.as_mut().unwrap();
        match field {
            "realm" => binding.realm_id = "other".into(),
            "slot" => binding.slot_id = "other".into(),
            "issuer" => binding.issuer = "https://other.invalid".into(),
            "client" => binding.client = "other".into(),
            "resource" => binding.resource = "other".into(),
            "strategy" => binding.strategy_id = "other".into(),
            "account" => binding.account = "other".into(),
            "scopes" => {
                binding
                    .operations
                    .get_mut("read-messages")
                    .unwrap()
                    .required_scopes
                    .insert("mail.write".into());
            }
            "destination" => {
                binding
                    .operations
                    .get_mut("read-messages")
                    .unwrap()
                    .destination_origin = "https://other.invalid".into()
            }
            "readonly" => {
                binding
                    .operations
                    .get_mut("read-messages")
                    .unwrap()
                    .read_only = false
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                adapter.check_service_mcp_operation(
                    retained.association(),
                    "mail",
                    "read-messages",
                    &changed.connector_approval(),
                    now
                ),
                Err(PrivateOriginalError::Denied)
            ),
            "{field}"
        );
    }
    assert_eq!(
        apply(&adapter, decision.clone(), selected)
            .await
            .unwrap()
            .state,
        PrivateServiceGrantState::GrantIssued
    );
    let mut changed = decision.clone();
    changed
        .permission
        .operations
        .remove(&Id::new("list-labels").unwrap());
    assert!(matches!(
        apply(&adapter, changed, selection(&config)).await,
        Err(PrivateOriginalError::Conflict)
    ));
    let mut changed = decision.clone();
    changed.connection_revision = Revision::new("binding-2").unwrap();
    assert!(matches!(
        apply(&adapter, changed, selection(&config)).await,
        Err(PrivateOriginalError::Conflict)
    ));
    let mut other = config.clone();
    other.connector.as_mut().unwrap().account = "other-account".into();
    assert!(matches!(
        apply(&adapter, decision.clone(), selection(&other)).await,
        Err(PrivateOriginalError::Conflict)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let native = crate::original_ingress::tests::machine(&owner);
    let mut custody = native.try_controller_grant_mutation().unwrap();
    owner
        .cancel_original(&retained, &app(), &mut custody)
        .unwrap();
    assert!(
        owner
            .grants()
            .resolve_lineage(&lineage, &adapter.routes.get().unwrap()[0].logical_executor, None)
            .is_err()
    );
    assert_eq!(
        apply(&adapter, decision, selection(&config))
            .await
            .unwrap()
            .state,
        PrivateServiceGrantState::GrantUnavailable
    );
}

#[tokio::test]
async fn wildcard_parent_cannot_supply_connector_and_empty_set_is_refused_before_writes() {
    let dir = directory();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let adapter = super::super::tests::adapter(owner, &dir.path().join("originals")).await;
    let before = std::fs::read(&path).unwrap();
    let mut empty = decision();
    empty.permission.operations.clear();
    assert!(matches!(
        apply(&adapter, empty, selection(&config())).await,
        Err(PrivateOriginalError::Denied)
    ));
    let db = rusqlite::Connection::open(dir.path().join("originals")).unwrap();
    let count: u64 = db
        .query_row("SELECT count(*) FROM toolkit_records", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        apply(&adapter, decision(), selection(&config()))
            .await
            .unwrap()
            .state,
        PrivateServiceGrantState::GrantUnavailable
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn explicit_no_connector_and_v2_bytes_have_distinct_meanings() {
    let dir = directory();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let mut config = config();
    assert!(matches!(
        ServiceMcpSelection::no_connector(&config),
        Err(PrivateOriginalError::Denied)
    ));
    config.connector = None;
    let selected = ServiceMcpSelection::no_connector(&config).unwrap();
    let adapter = selected_adapter(owner.clone(), &dir.path().join("originals"), &selected).await;
    let observed = apply(&adapter, decision(), selected).await.unwrap();
    assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
    let stored = adapter
        .store
        .get(&adapter.access_key(&observed.grant).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.data.version, 3);
    assert!(stored.data.body["mcp"]["connector"].is_null());
    let (retained, _) = bind(&adapter, &observed.grant, "no-connector-b").await;
    assert!(
        adapter
            .check_service_mcp_operation(
                retained.association(),
                "mail",
                "list-labels",
                &McpConnectorApproval::NoConnector,
                owner.clock().now().unwrap().unix_ms
            )
            .is_ok()
    );

    let dir = directory();
    let owner = initialized(&dir.path().join("grants"));
    let adapter = super::super::tests::adapter(owner.clone(), &dir.path().join("originals")).await;
    let legacy = super::super::tests::decision();
    let observed = adapter
        .apply_service_access_decision(legacy.clone())
        .await
        .unwrap();
    let stored = adapter
        .store
        .get(&adapter.access_key(&observed.grant).unwrap())
        .await
        .unwrap()
        .unwrap();
    let record = adapter.decode_access(&stored, &observed.grant).unwrap();
    let old_body = serde_json::json!({ "decision": legacy, "caller": record.caller, "parent": record.parent, "issuance": record.issuance });
    assert_eq!(stored.data.version, 2);
    assert!(stored.data.body.get("mcp").is_none());
    assert_eq!(
        canonical_json(&stored.data.body).unwrap(),
        canonical_json(&old_body).unwrap()
    );
    assert_eq!(
        observed.grant.decision_sha256,
        digest(&canonical_json(&legacy).unwrap())
    );
    let (retained, _) = bind(&adapter, &observed.grant, "legacy-b").await;
    assert!(matches!(
        adapter.check_service_mcp_operation(
            retained.association(),
            "mail",
            "read-messages",
            &McpConnectorApproval::NoConnector,
            owner.clock().now().unwrap().unix_ms
        ),
        Err(PrivateOriginalError::Unavailable)
    ));
    assert!(matches!(
        apply(
            &adapter,
            decision(),
            ServiceMcpSelection::no_connector(&config).unwrap()
        )
        .await,
        Err(PrivateOriginalError::Conflict)
    ));
}

#[tokio::test]
async fn narrowed_intermediate_parent_cannot_restore_removed_connector_approval() {
    let dir = directory();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let selected = selection(&config());
    let mut adapter = crate::original_ingress::tests::private_originals(
        owner.clone(),
        &dir.path().join("originals"),
    )
    .await;
    let route = &mut Arc::get_mut(&mut adapter).unwrap().routes.get_mut().unwrap()[0];
    let operator = route.grant_caller.clone();
    route.grant_caller = app();
    let mut bounds = selected
        .restrictions(route, &decision(), owner.clock().now().unwrap().unix_ms)
        .unwrap();
    bounds.delegation_depth = DelegationDepth::remaining(2);
    let root = owner
        .grants()
        .issue_root(
            &operator,
            EvidenceId::new("approved-root").unwrap(),
            app(),
            None,
            bounds.clone(),
        )
        .unwrap();
    bounds.actions = ExactRestriction::exact([
        ActionRef {
            feature: "mcp".into(),
            action: "read-messages".into(),
        },
        ActionRef {
            feature: "mcp".into(),
            action: "list-labels".into(),
        },
    ]);
    bounds.resource_domains = DomainRestriction::exact([ResourceDomain {
        authority: app(),
        namespace: "mcp-server/mail".into(),
    }]);
    bounds.delegation_depth = DelegationDepth::remaining(1);
    let child = owner
        .grants()
        .issue_child(
            &app(),
            &root,
            EvidenceId::new("narrowed-parent").unwrap(),
            app(),
            bounds,
        )
        .unwrap();
    route.application_parent = Some(vec![root, child]);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        apply(&adapter, decision(), selected).await.unwrap().state,
        PrivateServiceGrantState::GrantUnavailable
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

mod service_instruction;

#[path = "attachment_tests.rs"]
mod attachment;
