//! Actual native task/parent/child/source fixture controls. The application and
//! administrator below are explicit fixture producers. The invocation marker
//! is mechanical custody, NOT a genuine native ReviewedEntry or Console login.
use super::*;
use crate::private_originals::service_approved_input::admission_tests::{acquisition, reviewed_config};
use futures::FutureExt;
use meerkat_runtime::SessionServiceRuntimeExt;
use std::{panic::AssertUnwindSafe, sync::atomic::AtomicUsize, time::Duration};

const WAIT: Duration = Duration::from_secs(10);
fn id(value: &str) -> Id { Id::new(value).unwrap() }

#[derive(Clone, Copy)]
enum Case { Positive, RequiresHuman, WrongProposal, CancelPrepared, LateDenied, LateUnavailable,
    LateNotSettled, LostParentReceipt }

struct FixtureService { owner: PrincipalRef, original: PrivateOriginalRef, acquisition: StoreKey,
    fresh: EvidenceId, config_sha256: String, expiry: u64 }
struct ServiceReservation { owner: PrincipalRef, provenance: serde_json::Map<String, serde_json::Value> }
impl ApplicationServicePolicy for FixtureService {
    fn reserve(&self, request: &ApplicationServicePolicyRequest<'_>) -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
        if request.owner != &self.owner || request.original != &self.original || request.acquisition != &self.acquisition
            || request.fresh_original != &self.fresh || digest(&canonical_json(request.configuration).unwrap()) != self.config_sha256
            || request.permission.operations != [id("read")].into()
            || request.permission.lifetime.expires_at_ms != Some(self.expiry)
        { return Err(HostLifetimeError::Denied); }
        Ok(ApplicationServicePolicyDecision::Permit(Box::new(ServiceReservation {
            owner: self.owner.clone(), provenance: [("fixture_service".into(), "exact-task-v1".into())].into_iter().collect(),
        })))
    }
}
impl ApplicationServicePolicyReservation for ServiceReservation {
    fn owner(&self) -> &PrincipalRef { &self.owner }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> { &self.provenance }
    fn check_current(&self) -> Result<(), HostLifetimeError> { Ok(()) }
}
struct UnavailableService;
impl ApplicationServicePolicy for UnavailableService {
    fn reserve(&self, _: &ApplicationServicePolicyRequest<'_>) -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
        Err(HostLifetimeError::Unavailable)
    }
}

struct FixtureAdmin { operator: PrincipalRef, rule: ServiceParentAdministration, deployment: Id,
    original: PrivateOriginalRef, acquisition: StoreKey, config_sha256: String,
    source_path: std::path::PathBuf, case: Case, active: Arc<AtomicBool>, calls: Arc<AtomicUsize>, gate: Option<Arc<WorkerGate>> }
struct AdminReservation { operator: PrincipalRef, rule: ServiceParentAdministration, proposal_sha256: String,
    provenance: serde_json::Map<String, serde_json::Value>, source_path: std::path::PathBuf,
    child: String, parent: String, case: Case, active: Arc<AtomicBool>, gate: Option<Arc<WorkerGate>> }
#[async_trait::async_trait]
impl ApplicationServiceParentAuthorizer for FixtureAdmin {
    async fn reserve(&self, request: &ServiceParentAuthorizationRequest<'_>) -> Result<ServiceParentAuthorization, HostLifetimeError> {
        let proposal = request.proposal();
        if proposal.operator != self.operator || proposal.rule != self.rule || proposal.original != self.original
            || proposal.deployment != self.deployment
            || proposal.acquisition != self.acquisition || proposal.decision.connection_revision.as_str() != self.config_sha256
            || proposal.decision.permission.operations != [id("read")].into()
            || proposal.decision.live_attachment.is_none()
        { return Err(HostLifetimeError::Denied); }
        self.calls.fetch_add(1, Ordering::SeqCst);
        if matches!(self.case, Case::RequiresHuman) { return Ok(ServiceParentAuthorization::RequiresHuman); }
        let proposal_sha256 = if matches!(self.case, Case::WrongProposal) { "0".repeat(64) }
            else { proposal.proposal_sha256()? };
        Ok(ServiceParentAuthorization::Reserved(Box::new(AdminReservation {
            operator: self.operator.clone(), rule: self.rule.clone(), proposal_sha256,
            provenance: [("fixture_administrator".into(), "actual-selected-root-exact-rule-v1".into())].into_iter().collect(),
            source_path: self.source_path.clone(), child: proposal.decision.decision.to_string(),
            parent: proposal.issuance_id.as_str().to_owned(), case: self.case, active: self.active.clone(), gate: self.gate.clone(),
        })))
    }
}
impl ServiceParentAdministrationReservation for AdminReservation {
    fn operator(&self) -> &PrincipalRef { &self.operator }
    fn rule(&self) -> &ServiceParentAdministration { &self.rule }
    fn proposal_sha256(&self) -> &str { &self.proposal_sha256 }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> { &self.provenance }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        if let Some(gate) = &self.gate { gate.at_parent_worker(&self.source_path, &self.parent)?; }
        if !self.active.load(Ordering::SeqCst) || !matches!(self.case, Case::LateDenied | Case::LateUnavailable | Case::LateNotSettled) {
            return Ok(());
        }
        let sql = rusqlite::Connection::open(&self.source_path).map_err(|_| HostLifetimeError::Unavailable)?;
        let pending: i64 = sql.query_row("SELECT count(*) FROM toolkit_records WHERE \
            json_extract(data, '$.schema') = 'toolkit.authenticated-service-access-decision' AND \
            json_extract(data, '$.body.decision.decision') = ?1 AND \
            json_extract(data, '$.body.issuance.state') = 'pending_native_issuance'",
            [&self.child], |row| row.get(0)).map_err(|_| HostLifetimeError::Unavailable)?;
        if pending == 0 { return Ok(()); }
        Err(match self.case { Case::LateDenied => HostLifetimeError::Denied,
            Case::LateNotSettled => HostLifetimeError::NotSettled, _ => HostLifetimeError::Unavailable })
    }
}

async fn actual_task(fixture: &ContinuationFixture) -> InstalledServiceTask {
    let a = fixture.submit().await;
    fixture.finish(&a).await.unwrap();
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(&fixture.session,
        dispatch.retained_work().unwrap().run_id())).await.unwrap();
    let snapshot = acquisition(fixture, &dispatch).await;
    let member = snapshot.request().host().continuation.native_original.as_ref().unwrap().member.as_ref().unwrap().member.clone();
    fixture.source.prepare_service_task(fixture.store.clone(), snapshot, "owned-parent", &member).await.unwrap().unwrap()
}

fn policy(task: &InstalledServiceTask, fixture: &ContinuationFixture, config: &meerkat_core::McpServerConfig,
    case: Case, gate: Option<Arc<WorkerGate>>) -> (Arc<InstalledApplicationServicePolicy>, Arc<AtomicBool>, Arc<AtomicUsize>) {
    let lifetime = fixture.lifetime().clone();
    let native = lifetime.selected_policy().native_host.as_ref().unwrap();
    let config_sha256 = digest(&canonical_json(config).unwrap());
    let service = FixtureService { owner: native.grant_administrator.clone(), original: task.parent.clone(),
        acquisition: task.acquisition.clone(), fresh: task.fresh.clone(), config_sha256: config_sha256.clone(), expiry: task.before_ms };
    let active = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let admin = FixtureAdmin { operator: native.remote_operator.clone().unwrap(),
        deployment: lifetime.selected_policy().deployment.clone(),
        rule: native.service_parent_administration.clone().unwrap(), original: task.parent.clone(),
        acquisition: task.acquisition.clone(), config_sha256, source_path: fixture.source_path.clone(),
        case, active: active.clone(), calls: calls.clone(), gate };
    (Arc::new(InstalledApplicationServicePolicy::with_task_parent_provisioning(lifetime,
        Arc::new(service), Arc::new(admin)).unwrap()), active, calls)
}
fn begin(task: InstalledServiceTask, policy: Arc<InstalledApplicationServicePolicy>, config: &meerkat_core::McpServerConfig)
    -> ApplicationServiceOperation {
    match meerkat::ConnectorMcpInstallation::from_config(config) {
        Some(installation) => task.begin_application_connector(policy, installation.unwrap(), [id("read")].into(), None),
        None => task.begin_application_no_connector(policy, config.clone(), [id("read")].into(), None),
    }.unwrap()
}
async fn prepare(operation: &mut ApplicationServiceOperation) -> Result<ApplicationServicePreparedStatus, PrivateOriginalError> {
    let control = operation.control();
    tokio::select! {
        prepared = control.prepared() => prepared,
        observed = operation.drive() => match observed.reason {
            Some(ServiceParentOperationReason::RequiresHuman) => Ok(ApplicationServicePreparedStatus::RequiresHuman),
            Some(ServiceParentOperationReason::Unavailable | ServiceParentOperationReason::NotSettled) => Err(PrivateOriginalError::Unavailable),
            _ => Err(PrivateOriginalError::Denied),
        },
    }
}

async fn exercise(case: Case, connector: bool) {
    let fixture = ContinuationFixture::for_task_parent_service_input().await;
    let mut operation = None;
    let result = AssertUnwindSafe(async {
        let task = actual_task(&fixture).await;
        let fresh = task.fresh.clone();
        let source_a = task.parent.clone();
        let route = fixture.source.route(&source_a.route).unwrap();
        assert!(route.application_parent.is_none(), "fresh real destination has no configured service parent");
        assert!(task.console_login.is_none(), "stock fixture source does not authenticate a Console Human");
        let mut config = reviewed_config(&fixture.scope.realm);
        if !connector { config.connector = None; config.access = Some(meerkat_core::McpServerAccess::allowing(["read".to_owned()])); }
        let (policy, active, calls) = policy(&task, &fixture, &config, case, None);
        let retry_task = task.clone();
        if matches!(case, Case::LostParentReceipt) {
            rusqlite::Connection::open(&fixture.source_path).unwrap().execute_batch(
                "CREATE TRIGGER fail_parent_receipt BEFORE UPDATE OF data ON toolkit_records \
                 WHEN json_extract(NEW.data, '$.schema') = 'toolkit.operator-application-parent' \
                 AND json_extract(NEW.data, '$.version') = 3 \
                 BEGIN SELECT RAISE(FAIL, 'lost parent receipt'); END;").unwrap();
        }
        operation = Some(begin(task, policy.clone(), &config));
        let op = operation.as_mut().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0, "creation starts no work");
        let prepared = tokio::time::timeout(WAIT, prepare(op)).await.unwrap();
        let control = op.control();
        if matches!(prepared, Ok(ApplicationServicePreparedStatus::PreparedForEntry)) {
            // Fixture-only invocation liveness. Real ticket.enter coverage is
            // required separately in the actual Connect setup-tool path.
            if matches!(case, Case::CancelPrepared) { control.begin_retire(); }
            else { control.enter(|| true).unwrap(); }
        }
        let observed = tokio::time::timeout(WAIT, op.drive()).await.unwrap();
        assert!(observed.custody == ServiceParentCustody::Joined);
        assert!(control.result().await == observed, "result only observes the same completion");
        let late_child = if matches!(case, Case::LateDenied | Case::LateUnavailable | Case::LateNotSettled) {
            // The existing key owner resolves the real task's decision row.
            // This lookup-only coordinate is never passed as grant authority.
            let reference = PrivateServiceGrantRef { route: source_a.route.clone(),
                decision: id(fresh.as_str()), decision_sha256: "0".repeat(64) };
            let key = crate::private_originals::service_grants::tests::fixture_access_key(&fixture.source, &reference).unwrap();
            let stored = fixture.store.get(&key).await.unwrap().unwrap();
            assert_eq!(stored.data.schema, "toolkit.authenticated-service-access-decision");
            assert_eq!(stored.data.body["decision"]["route"], serde_json::to_value(&reference.route).unwrap());
            assert_eq!(stored.data.body["decision"]["decision"], serde_json::to_value(&reference.decision).unwrap());
            let (state, reason) = match case {
                Case::LateDenied => ("native_refused", ServiceParentOperationReason::Denied),
                Case::LateNotSettled => ("pending_native_issuance", ServiceParentOperationReason::NotSettled),
                _ => ("pending_native_issuance", ServiceParentOperationReason::Unavailable),
            };
            assert_eq!(stored.data.body["issuance"], serde_json::json!({ "state": state }));
            assert!(observed.reason == Some(reason), "the original typed refusal is retained");
            assert!(observed.child.is_none() && observed.source.is_none());
            let child = EvidenceId::new(format!("access-{}", stored.key.id)).unwrap();
            assert!(fixture.source.owner.grants().issued_reference(&child).unwrap().is_none(), "the exact child was never issued");
            let fresh_key = StoreKey { realm: fixture.scope.realm.clone(), owner: fixture.scope.owner.clone(),
                id: crate::private_originals::source_key_id(&route.realm, fixture.source.owner.ingress_domain(),
                    &route.source_authority, &fresh).unwrap() };
            assert!(fixture.store.get(&fresh_key).await.unwrap().is_none(), "no fresh source was retained");
            Some((stored, fresh_key))
        } else { None };
        match case {
            Case::RequiresHuman | Case::WrongProposal | Case::CancelPrepared => {
                assert!(observed.outcome == Some(ServiceParentOperationOutcome::RefusedBeforeEntry));
                assert!(!observed.parent_issued && observed.source.is_none());
                assert!(!fixture.store.list_records(&fixture.scope, None, 256).await.unwrap().iter()
                    .any(|row| row.data.schema == "toolkit.operator-application-parent" && row.data.version == 3));
            }
            Case::Positive => {
                assert!(observed.outcome == Some(ServiceParentOperationOutcome::Succeeded) && observed.parent_issued);
                let operator = fixture.source.application_operator_setup(fixture.lifetime().clone()).unwrap();
                let issuance = observed.parent_issuance_id.as_ref().unwrap();
                let parent_key = StoreKey { realm: fixture.scope.realm.clone(), owner: fixture.scope.owner.clone(),
                    id: issuance.as_str().to_owned() };
                let parent = fixture.store.get(&parent_key).await.unwrap().unwrap();
                assert_eq!(parent.data.body["acquisition"], serde_json::to_value(&retry_task.acquisition).unwrap());
                let expected_expiry = Some(retry_task.before_ms);
                let instruction_sha256 = digest(retry_task.instruction.as_bytes());
                let fresh_id = id(retry_task.fresh.as_str());
                assert_eq!(operator.retained_service_parent_expiry(&retry_task.parent, &retry_task.acquisition,
                    &fresh_id, &instruction_sha256, None, None).await.unwrap(), expected_expiry);
                for (field, value) in [("realm", "other-realm"), ("owner", fixture.scope.owner.as_str()),
                    ("owner", "toolkit/acquisition/other-deployment")] {
                    let mut changed = parent.data.clone();
                    changed.body["acquisition"][field] = value.into();
                    let current = fixture.store.get(&parent_key).await.unwrap().unwrap();
                    let corrupt = fixture.store.compare_and_swap(toolkit_store::Transition {
                        key: parent_key.clone(), expected_revision: current.revision,
                        expected_digest: Some(current.content_digest), claim: None,
                        data: changed, events: vec![], effects: vec![],
                    }).await.unwrap();
                    assert!(operator.retained_service_parent_expiry(&retry_task.parent, &retry_task.acquisition,
                        &fresh_id, &instruction_sha256, None, None).await.is_err(), "{field}={value}");
                    fixture.store.compare_and_swap(toolkit_store::Transition {
                        key: parent_key.clone(), expected_revision: corrupt.revision,
                        expected_digest: Some(corrupt.content_digest), claim: None,
                        data: parent.data.clone(), events: vec![], effects: vec![],
                    }).await.unwrap();
                }
                assert_eq!(operator.retained_service_parent_expiry(&retry_task.parent, &retry_task.acquisition,
                    &fresh_id, &instruction_sha256, None, None).await.unwrap(), expected_expiry);
                let b = observed.source.clone().unwrap();
                assert!(b != source_a && b.original == fresh);
                let row = fixture.store.get(&fixture.source.key(&b).unwrap()).await.unwrap().unwrap();
                let record = fixture.source.decode_record(&row, &b).unwrap();
                let grant = &record.approved_task.as_ref().unwrap().singleton_grant().unwrap();
                let work = meerkat_authorization_contracts::work_association::OriginalWorkRef { authority: route.source_authority.clone(), work: fresh };
                let lineage = fixture.source.service_grant_lineage(grant, &work).await.unwrap();
                assert_eq!(lineage.len(), 2, "real newly issued root and exact attenuated child");
                assert!(Some(lineage[0].grant_id.clone()) == observed.parent_issuance_id);
                let resolved = fixture.source.owner.grants().resolve_lineage(&lineage,
                    &route.logical_executor, route.represented_subject.as_ref()).unwrap();
                resolved.check_current().unwrap();
                let approval = meerkat_core::mcp_config::McpConnectorApproval::of(config.connector.as_ref());
                let connector_values = crate::installed_host::application_operations::mcp_connector_operation(
                    &route.grant_caller, &route.logical_executor, &route.grant_audience, &config.name, &approval);
                assert_eq!(meerkat_authorization::mcp_installation::lineage_connector_bound(
                    resolved.lineage_restrictions(), resolved.restrictions(), &connector_values.action, &connector_values.resource_domain),
                    meerkat_authorization::mcp_installation::McpConnectorBound::Approved);
                let now = fixture.source.owner.clock().now().unwrap().unix_ms;
                for (operation, allowed) in [("read", true), ("write", false)] {
                    let values = crate::installed_host::application_operations::mcp_operation(
                        &route.grant_caller, &route.logical_executor, &route.grant_audience, &config.name, operation);
                    assert_eq!(resolved.restrictions().check_bounds(meerkat_authorization_contracts::constraints::OperationRestrictionValues {
                        action: &values.action, resource_domain: &values.resource_domain, processor: &values.processor,
                        audience: &values.audience, now_ms: now,
                    }).is_ok(), allowed);
                }
                assert!(fixture.source.route(&source_a.route).unwrap().application_parent.is_none(), "no route rewrite");
                fixture.source.reserve_service_continuation(&row).await.unwrap();
                fixture.source.step_service_input(&row).await.unwrap();
                assert_eq!(fixture.inputs().await, Ok(2), "actual fresh native B, never replay A");
                fixture.source.step_service_input(&row).await.unwrap();
                assert_eq!(fixture.inputs().await, Ok(2));
            }
            Case::LateDenied => {
                assert!(observed.parent_issued && observed.outcome == Some(ServiceParentOperationOutcome::FailedAfterEffects));
                assert!(observed.reason == Some(ServiceParentOperationReason::Denied) && observed.source.is_none());
            }
            Case::LateUnavailable | Case::LateNotSettled | Case::LostParentReceipt => {
                assert!(observed.parent_issued && observed.outcome == Some(ServiceParentOperationOutcome::Uncertain));
                let issuance = observed.parent_issuance_id.clone().unwrap();
                let actual = fixture.source.owner.grants().issued_reference(&issuance).unwrap().unwrap();
                active.store(false, Ordering::SeqCst);
                if matches!(case, Case::LostParentReceipt) {
                    // A real root already issued but its receipt is Pending.
                    // A later prepare refusal must retain that exact attempt.
                    let unavailable = Arc::new(InstalledApplicationServicePolicy::with_task_parent_provisioning(
                        fixture.lifetime().clone(), Arc::new(UnavailableService),
                        policy.parent_provisioning.as_ref().unwrap().2.clone()).unwrap());
                    *op = begin(retry_task.clone(), unavailable, &config);
                    let late = tokio::time::timeout(WAIT, op.drive()).await.unwrap();
                    assert!(late.custody == ServiceParentCustody::Joined
                        && late.outcome == Some(ServiceParentOperationOutcome::Uncertain)
                        && late.reason == Some(ServiceParentOperationReason::Unavailable)
                        && late.parent_issuance_id.as_ref() == Some(&issuance));
                    rusqlite::Connection::open(&fixture.source_path).unwrap().execute_batch("DROP TRIGGER fail_parent_receipt;").unwrap();
                }
                *op = begin(retry_task, policy, &config);
                assert!(matches!(tokio::time::timeout(WAIT, prepare(op)).await.unwrap(), Ok(ApplicationServicePreparedStatus::PreparedForEntry)));
                op.control().enter(|| true).unwrap();
                let replay = tokio::time::timeout(WAIT, op.drive()).await.unwrap();
                assert!(replay.parent_issuance_id == Some(issuance.clone()));
                assert!(fixture.source.owner.grants().issued_reference(&issuance).unwrap() == Some(actual), "same native identity, never root reissue");
                if matches!(case, Case::LostParentReceipt) { assert!(replay.outcome == Some(ServiceParentOperationOutcome::Succeeded)); }
                else {
                    assert!(replay.outcome == Some(ServiceParentOperationOutcome::Uncertain) && replay.source.is_none(), "child Pending is not permission to retry issue_child");
                    let (stored, fresh_key) = late_child.as_ref().unwrap();
                    assert_eq!(fixture.store.get(&stored.key).await.unwrap().as_ref(), Some(stored), "replay keeps the same Pending child attempt");
                    let child = EvidenceId::new(format!("access-{}", stored.key.id)).unwrap();
                    assert!(fixture.source.owner.grants().issued_reference(&child).unwrap().is_none());
                    assert!(fixture.store.get(fresh_key).await.unwrap().is_none());
                }
            }
        }
    }).catch_unwind().await;
    // Assertions/timeouts cannot abandon the operation's retained work.
    if let Some(operation) = operation.as_mut() { operation.retire().await; }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_connector_parent_child_and_fresh_native_source() { exercise(Case::Positive, true).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_no_connector_parent_child_and_fresh_native_source() { exercise(Case::Positive, false).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standing_parent_accepts_canonical_acquisition_scope_and_rejects_other_scopes() {
    let fixture = ContinuationFixture::for_task_parent_service_input().await;
    let result = AssertUnwindSafe(async {
        let task = actual_task(&fixture).await;
        // The key came from an admitted SetupStore snapshot, not a fixture-only owner.
        assert_eq!(task.acquisition.realm, fixture.scope.realm);
        assert_eq!(task.acquisition.owner, format!("toolkit/acquisition/{}", fixture.scope.owner));
        assert_ne!(task.acquisition.owner, fixture.scope.owner);
        let operator = fixture.source.application_operator_setup(fixture.lifetime().clone()).unwrap();
        let fresh = id(task.fresh.as_str());
        assert!(operator.service_parent_rule(&task.parent, &task.acquisition, &fresh).is_ok(),
            "the canonical acquisition key must be accepted for its selected deployment");
        for (field, value) in [("realm", "other-realm"), ("owner", fixture.scope.owner.as_str()),
            ("owner", "toolkit/acquisition/other-deployment")] {
            let mut wrong = task.acquisition.clone();
            match field { "realm" => wrong.realm = value.into(), _ => wrong.owner = value.into() };
            assert!(operator.service_parent_rule(&task.parent, &wrong, &fresh).is_err(), "{field}={value}");
        }
    }).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn administrative_requires_human_never_issues() { exercise(Case::RequiresHuman, false).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_administrative_proposal_never_issues() { exercise(Case::WrongProposal, false).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retiring_prepared_operation_never_enters_grants() { exercise(Case::CancelPrepared, false).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_issued_then_child_denied_is_joined_partial_failure() { exercise(Case::LateDenied, false).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_administrative_unavailable_preserves_pending_without_reissue() { exercise(Case::LateUnavailable, false).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_administrative_not_settled_retains_typed_reason() { exercise(Case::LateNotSettled, false).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_parent_receipt_recovers_actual_native_identity_once() { exercise(Case::LostParentReceipt, false).await; }

/// Blocks the actual parent spawn_blocking worker's final policy predicate,
/// after its durable Pending write and under its native setup fence. It does
/// not pretend that native issue_root has returned or that an ACK was lost.
struct WorkerGate {
    once: AtomicBool,
    entered: Mutex<Option<oneshot::Sender<()>>>,
    open: Mutex<bool>,
    changed: std::sync::Condvar,
}
impl WorkerGate {
    fn release(&self) {
        if let Ok(mut open) = self.open.lock() { *open = true; self.changed.notify_all(); }
    }
    fn at_parent_worker(&self, path: &std::path::Path, parent: &str) -> Result<(), HostLifetimeError> {
        if self.once.load(Ordering::SeqCst) { return Ok(()); }
        let sql = rusqlite::Connection::open(path).map_err(|_| HostLifetimeError::Unavailable)?;
        let pending: i64 = sql.query_row("SELECT count(*) FROM toolkit_records WHERE id = ?1 AND \
            json_extract(data, '$.schema') = 'toolkit.operator-application-parent' AND \
            json_extract(data, '$.body.issued.state') = 'pending'", [parent], |row| row.get(0))
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if pending == 0 || self.once.swap(true, Ordering::SeqCst) { return Ok(()); }
        if let Some(sender) = self.entered.lock().map_err(|_| HostLifetimeError::Unavailable)?.take() { let _ = sender.send(()); }
        let mut open = self.open.lock().map_err(|_| HostLifetimeError::Unavailable)?;
        while !*open { open = self.changed.wait(open).map_err(|_| HostLifetimeError::Unavailable)?; }
        Ok(())
    }
}
struct ReleaseGate(Arc<WorkerGate>);
impl Drop for ReleaseGate { fn drop(&mut self) { self.0.release(); } }
struct MechanicalCaller(Arc<AtomicUsize>);
impl Drop for MechanicalCaller { fn drop(&mut self) { self.0.fetch_add(1, Ordering::SeqCst); } }

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_drive_waiter_retains_actual_parent_worker_until_retired_join() {
    let fixture = ContinuationFixture::for_task_parent_service_input().await;
    let (entered, entered_receive) = oneshot::channel();
    let gate = Arc::new(WorkerGate { once: AtomicBool::new(false), entered: Mutex::new(Some(entered)),
        open: Mutex::new(false), changed: std::sync::Condvar::new() });
    let release = ReleaseGate(gate.clone());
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut operation = None;
    let result = AssertUnwindSafe(async {
        let task = actual_task(&fixture).await;
        let mut config = reviewed_config(&fixture.scope.realm);
        config.connector = None;
        config.access = Some(meerkat_core::McpServerAccess::allowing(["read".to_owned()]));
        let (policy, _, _) = policy(&task, &fixture, &config, Case::Positive, Some(gate.clone()));
        operation = Some(begin(task, policy, &config));
        let op = operation.as_mut().unwrap();
        assert!(matches!(tokio::time::timeout(WAIT, prepare(op)).await.unwrap(), Ok(ApplicationServicePreparedStatus::PreparedForEntry)));
        let control = op.control();
        let marker = MechanicalCaller(dropped.clone());
        control.enter(move || { let _mechanical_caller = &marker; true }).unwrap();
        let entered = {
            let drive = op.drive();
            tokio::pin!(drive);
            tokio::select! {
                entered = tokio::time::timeout(WAIT, entered_receive) => entered.is_ok_and(|value| value.is_ok()),
                _ = &mut drive => false,
            }
        }; // Only the borrowed drive waiter is dropped; op retains the handle.
        control.begin_retire();
        let still_retained = dropped.load(Ordering::SeqCst) == 0;
        let premature = tokio::time::timeout(Duration::from_millis(25), control.result()).await.is_ok();
        // Release and join before every assertion, including adverse schedules.
        gate.release();
        let observed = op.retire().await;
        assert!(entered && still_retained && !premature);
        assert!(observed.custody == ServiceParentCustody::Joined && !observed.parent_issued);
        assert!(observed.reason == Some(ServiceParentOperationReason::Cancelled));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }).catch_unwind().await;
    drop(release);
    if let Some(operation) = operation.as_mut() { operation.retire().await; }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[test]
fn joined_native_refusal_is_distinct_from_an_unknown_entered_outcome() {
    // Classification-only control. This is not a native issue_root fixture.
    let progress = ServiceOperationProgress::default();
    progress.entering();
    let refused = Err(ServiceOperationError::Source(PrivateOriginalError::Denied));
    assert!(operation_outcome(&refused, &progress) == ServiceParentOperationOutcome::Uncertain);
    progress.native_refused();
    assert!(operation_outcome(&refused, &progress) == ServiceParentOperationOutcome::RefusedBeforeEntry);
}

struct AwaitRetirement {
    inner: Arc<dyn ApplicationServiceParentAuthorizer>,
    entered: Mutex<Option<oneshot::Sender<()>>>,
}
#[async_trait::async_trait]
impl ApplicationServiceParentAuthorizer for AwaitRetirement {
    async fn reserve(&self, request: &ServiceParentAuthorizationRequest<'_>) -> Result<ServiceParentAuthorization, HostLifetimeError> {
        let actual = self.inner.reserve(request).await?;
        if let Some(entered) = self.entered.lock().map_err(|_| HostLifetimeError::Unavailable)?.take() { let _ = entered.send(()); }
        request.observer().wait_retiring().await;
        drop(actual); // The fixture's real pending producer has now settled.
        Err(HostLifetimeError::Denied)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_rust_authorizer_observes_retirement_before_entry() {
    let fixture = ContinuationFixture::for_task_parent_service_input().await;
    let mut operation = None;
    let result = AssertUnwindSafe(async {
        let task = actual_task(&fixture).await;
        let mut config = reviewed_config(&fixture.scope.realm);
        config.connector = None;
        config.access = Some(meerkat_core::McpServerAccess::allowing(["read".to_owned()]));
        let (base, _, _) = policy(&task, &fixture, &config, Case::Positive, None);
        let (entered, receive) = oneshot::channel();
        let authorizer = AwaitRetirement { inner: base.parent_provisioning.as_ref().unwrap().2.clone(), entered: Mutex::new(Some(entered)) };
        let selected = Arc::new(InstalledApplicationServicePolicy::with_task_parent_provisioning(
            fixture.lifetime().clone(), base.policy.clone(), Arc::new(authorizer)).unwrap());
        operation = Some(begin(task, selected, &config));
        let op = operation.as_mut().unwrap();
        let waiting = {
            let drive = op.drive();
            tokio::pin!(drive);
            tokio::select! {
                entered = tokio::time::timeout(WAIT, receive) => entered.is_ok_and(|value| value.is_ok()),
                _ = &mut drive => false,
            }
        };
        // Plain Rust retains and drives the same operation, without an actor
        // or pipe. The producer observes retirement and returns before Joined.
        let observed = op.retire().await;
        assert!(waiting && observed.custody == ServiceParentCustody::Joined
            && observed.outcome == Some(ServiceParentOperationOutcome::RefusedBeforeEntry)
            && observed.reason == Some(ServiceParentOperationReason::Cancelled));
        assert!(!observed.parent_issued && observed.child.is_none() && observed.source.is_none());
        assert!(!fixture.store.list_records(&fixture.scope, None, 256).await.unwrap().iter()
            .any(|row| row.data.schema == "toolkit.operator-application-parent" && row.data.version == 3));
    }).catch_unwind().await;
    if let Some(operation) = operation.as_mut() { operation.retire().await; }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

// The package callback is an exact test double only for filesystem readback.
// Parent/child issuance, source publication and consumer dispatch remain real.
struct ApplicationPackageReadback {
    config: meerkat_core::McpServerConfig,
    document: serde_json::Value,
    calls: AtomicUsize,
    store: Arc<dyn toolkit_store::ToolkitStore>,
    change_during_readback: tokio::sync::Mutex<Option<(StoreKey, toolkit_store::RecordData)>>,
}
#[async_trait::async_trait]
impl ServiceInstallationReadback for ApplicationPackageReadback {
    async fn readback(&self, _: &ApprovalRecord) -> Result<meerkat_core::McpServerConfig, PrivateOriginalError> {
        panic!("standing application decision must not become a human approval")
    }
    async fn readback_source(&self, source: &ServiceInstallationReadbackSource)
        -> Result<meerkat_core::McpServerConfig, PrivateOriginalError> {
        let ServiceInstallationReadbackSource::ApplicationPolicy(source) = source else {
            panic!("expected the retained application decision")
        };
        if source.document() != &self.document || source.configuration()? != self.config {
            return Err(PrivateOriginalError::Denied);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some((key, data)) = self.change_during_readback.lock().await.take() {
            let current = self.store.get(&key).await.unwrap().unwrap();
            self.store.compare_and_swap(toolkit_store::Transition { key,
                expected_revision: current.revision, expected_digest: Some(current.content_digest),
                claim: None, data, events: vec![], effects: vec![],
            }).await.unwrap();
        }
        Ok(self.config.clone())
    }
}

async fn application_installation_readback_case(include_source: bool) {
    let fixture = ContinuationFixture::for_task_parent_service_input().await;
    let mut operation = None;
    let result = AssertUnwindSafe(async {
        let mut task = actual_task(&fixture).await;
        let mut config = reviewed_config(&fixture.scope.realm);
        config.connector = None;
        config.access = Some(meerkat_core::McpServerAccess::allowing(["read".to_owned()]));
        let document = serde_json::json!({"software":{"receipt_sha256":"a".repeat(64)},
            "fixture_scope":"exact package readback only"});
        if include_source {
            task = task.with_installation_readback_source(&config, document.clone()).unwrap();
        }
        let (policy, _, _) = policy(&task, &fixture, &config, Case::Positive, None);
        let readback = Arc::new(ApplicationPackageReadback { config: config.clone(), document,
            calls: AtomicUsize::new(0), store: fixture.store.clone(),
            change_during_readback: tokio::sync::Mutex::new(None) });
        fixture.source.bind_service_installation_readback(readback.clone()).unwrap();
        operation = Some(begin(task, policy, &config));
        let op = operation.as_mut().unwrap();
        assert!(matches!(tokio::time::timeout(WAIT, prepare(op)).await.unwrap(),
            Ok(ApplicationServicePreparedStatus::PreparedForEntry)));
        op.control().enter(|| true).unwrap();
        let observed = tokio::time::timeout(WAIT, op.drive()).await.unwrap();
        assert!(observed.outcome == Some(ServiceParentOperationOutcome::Succeeded));
        assert!(observed.custody == ServiceParentCustody::Joined && observed.parent_issued);
        let b = observed.source.unwrap();
        let source = fixture.store.get(&fixture.source.key(&b).unwrap()).await.unwrap().unwrap();
        let record = fixture.source.decode_record(&source, &b).unwrap();
        assert_eq!(record.event.original_authentication.revision.as_str(), "application-service-policy-v1");
        let parent_key = StoreKey { realm: fixture.scope.realm.clone(), owner: fixture.scope.owner.clone(),
            id: observed.parent_issuance_id.unwrap().as_str().to_owned() };
        let parent = fixture.store.get(&parent_key).await.unwrap().unwrap();
        assert_eq!(parent.data.version, 3);
        assert!(parent.data.body.get("approval").is_none());
        assert_eq!(parent.data.body["issued"]["state"], "issued");
        let read = fixture.source.read_service_installation(&b).await;
        if !include_source {
            assert!(matches!(read, Err(PrivateOriginalError::Denied)), "old application records do not invent software selection");
            assert_eq!(readback.calls.load(Ordering::SeqCst), 0);
            return;
        }
        assert_eq!(read.expect("standing application source must reach exact package readback"), config);
        assert_eq!(readback.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.store.get(&source.key).await.unwrap().unwrap(), source, "readback does not admit B");
        assert_eq!(fixture.store.get(&parent_key).await.unwrap().unwrap(), parent, "readback never reissues parent");
        for field in ["administrative_decision", "original", "acquisition"] {
            let mut changed = parent.data.clone();
            changed.body[field] = serde_json::Value::Null;
            let current = fixture.store.get(&parent_key).await.unwrap().unwrap();
            let corrupt = fixture.store.compare_and_swap(toolkit_store::Transition { key: parent_key.clone(),
                expected_revision: current.revision, expected_digest: Some(current.content_digest),
                claim: None, data: changed, events: vec![], effects: vec![],
            }).await.unwrap();
            assert!(fixture.source.read_service_installation(&b).await.is_err(), "{field}");
            assert_eq!(readback.calls.load(Ordering::SeqCst), 1, "corrupt parent never reaches package owner");
            fixture.store.compare_and_swap(toolkit_store::Transition { key: parent_key.clone(),
                expected_revision: corrupt.revision, expected_digest: Some(corrupt.content_digest),
                claim: None, data: parent.data.clone(), events: vec![], effects: vec![],
            }).await.unwrap();
        }
        let grant = &record.approved_task.as_ref().unwrap().singleton_grant().unwrap();
        let key = crate::private_originals::service_grants::tests::fixture_access_key(&fixture.source, grant).unwrap();
        let access = fixture.store.get(&key).await.unwrap().unwrap();
        let mut changed = access.data.clone();
        changed.body["application_policy"]["installation_readback"]["document"]["software"]["receipt_sha256"] = "b".repeat(64).into();
        let corrupt = fixture.store.compare_and_swap(toolkit_store::Transition { key: key.clone(),
            expected_revision: access.revision, expected_digest: Some(access.content_digest),
            claim: None, data: changed, events: vec![], effects: vec![],
        }).await.unwrap();
        assert!(fixture.source.read_service_installation(&b).await.is_err(), "retained software document is bound by access/source digest");
        assert_eq!(readback.calls.load(Ordering::SeqCst), 1);
        fixture.store.compare_and_swap(toolkit_store::Transition { key,
            expected_revision: corrupt.revision, expected_digest: Some(corrupt.content_digest),
            claim: None, data: access.data, events: vec![], effects: vec![],
        }).await.unwrap();
        let mut changed = parent.data.clone();
        changed.body["administrative_decision"] = serde_json::Value::Null;
        *readback.change_during_readback.lock().await = Some((parent_key, changed));
        assert!(fixture.source.read_service_installation(&b).await.is_err(), "post-I/O administrative currentness remains required");
        assert_eq!(readback.calls.load(Ordering::SeqCst), 2);
        assert!(fixture.source.decode_record(&fixture.store.get(&source.key).await.unwrap().unwrap(), &b)
            .unwrap().service_admission.is_none());
    }).catch_unwind().await;
    if let Some(operation) = operation.as_mut() { operation.retire().await; }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standing_application_installation_readback_preserves_authority_and_software() {
    application_installation_readback_case(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_installation_readback_refuses_missing_legacy_selection() {
    application_installation_readback_case(false).await;
}
