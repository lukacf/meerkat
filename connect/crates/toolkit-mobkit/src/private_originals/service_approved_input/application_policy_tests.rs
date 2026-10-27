//! Same persistent native fixture, with a finite exact parent selected while
//! its original owners are stopped. The policy below is an explicit fixture
//! producer. Stock operator ingress is not authenticated Console HTTP. Model
//! transport is scripted; these controls do not connect a live MCP account.
use super::*;
use crate::installed_host::controller_first_input_fixture::ApplicationServiceSource;
use crate::private_originals::service_approved_input::application::*;
use meerkat_authorization_contracts::{constraints::OperationRestrictionValues,
    work_association::{InputAuthorityAssociation, OriginalWorkRef}};
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore, input_state::StoredInputState};
use meerkat_toolkit::{authority::{LifetimeScope, RoleLifetime},
    continuation::SetupContinuationDelivery, host_lifetime::control::HostLifetimeError};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

type Fixture = ContinuationFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;

fn request_digest(request: &ApplicationServicePolicyRequest<'_>) -> String {
    let route = request.route;
    digest(&canonical_json(&(request.owner, request.deployment, request.source_policy_sha256,
        (&route.id, &route.grant_caller, &route.application_parent, &route.controller_parent,
            &route.grant_target, &route.grant_audience, &route.realm, &route.requester,
            &route.ingress_actor, &route.logical_executor, &route.represented_subject,
            &route.source_authority, &route.authentication_domain, &route.event_domain),
        request.original, request.authenticated_event, request.task, request.acquisition,
        request.fresh_original, request.instruction, request.configuration,
        request.permission, request.attachment)).unwrap())
}

#[derive(Default)]
struct PolicyProbe { reserved: AtomicUsize, checked: AtomicUsize, dropped: AtomicUsize }
struct ExactPolicy {
    expected: String,
    owner: PrincipalRef,
    probe: Arc<PolicyProbe>,
    current: Arc<AtomicBool>,
    requires_human: bool,
    late: Option<Arc<LatePolicy>>,
}
struct ReservedPolicy {
    owner: PrincipalRef,
    provenance: serde_json::Map<String, serde_json::Value>,
    probe: Arc<PolicyProbe>,
    current: Arc<AtomicBool>,
    late: Option<Arc<LatePolicy>>,
}

#[derive(Clone, Copy)]
enum LateFailure { Unavailable, NotSettled, Denied }
struct LatePolicy {
    source_path: std::path::PathBuf,
    realm: String,
    owner: String,
    fresh: String,
    failure: LateFailure,
    active: AtomicBool,
    observed_pending: AtomicUsize,
}
impl LatePolicy {
    fn check(&self) -> Result<(), HostLifetimeError> {
        if !self.active.load(Ordering::SeqCst) { return Ok(()); }
        let sql = rusqlite::Connection::open(&self.source_path).map_err(|_| HostLifetimeError::Unavailable)?;
        let pending: i64 = sql.query_row(
            "SELECT count(*) FROM toolkit_records WHERE realm = ?1 AND owner = ?2 \
             AND json_extract(data, '$.schema') = 'toolkit.authenticated-service-access-decision' \
             AND json_extract(data, '$.body.decision.decision') = ?3 \
             AND json_extract(data, '$.body.issuance.state') = 'pending_native_issuance'",
            [&self.realm, &self.owner, &self.fresh], |row| row.get(0))
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if pending == 0 { return Ok(()); }
        self.observed_pending.fetch_add(1, Ordering::SeqCst);
        Err(match self.failure {
            LateFailure::Unavailable => HostLifetimeError::Unavailable,
            LateFailure::NotSettled => HostLifetimeError::NotSettled,
            LateFailure::Denied => HostLifetimeError::WrongOwner,
        })
    }
}
impl ApplicationServicePolicy for ExactPolicy {
    fn reserve(&self, request: &ApplicationServicePolicyRequest<'_>)
        -> Result<ApplicationServicePolicyDecision, HostLifetimeError>
    {
        // This fixture knows the actual task and canonical source beforehand.
        // A string saying who the caller is cannot satisfy this comparison.
        if request_digest(request) != self.expected || request.owner != &self.owner {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.probe.reserved.fetch_add(1, Ordering::SeqCst);
        if self.requires_human { return Ok(ApplicationServicePolicyDecision::RequiresHuman); }
        Ok(ApplicationServicePolicyDecision::Permit(Box::new(ReservedPolicy {
            owner: self.owner.clone(), probe: self.probe.clone(), current: self.current.clone(), late: self.late.clone(),
            provenance: [("fixture_policy".into(), "exact-current-task-v1".into())].into_iter().collect(),
        })))
    }
}
impl ApplicationServicePolicyReservation for ReservedPolicy {
    fn owner(&self) -> &PrincipalRef { &self.owner }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> { &self.provenance }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        self.probe.checked.fetch_add(1, Ordering::SeqCst);
        if !self.current.load(Ordering::SeqCst) { return Err(HostLifetimeError::InvalidSelection); }
        self.late.as_ref().map(|late| late.check()).unwrap_or(Ok(()))
    }
}
impl Drop for ReservedPolicy {
    fn drop(&mut self) { self.probe.dropped.fetch_add(1, Ordering::SeqCst); }
}

async fn policy_for(task: &InstalledServiceTask, config: &meerkat_core::McpServerConfig,
    requires_human: bool) -> (InstalledApplicationServicePolicy, Arc<PolicyProbe>, Arc<AtomicBool>)
{
    policy_for_with_late(task, config, requires_human, None).await
}

async fn policy_for_with_late(task: &InstalledServiceTask, config: &meerkat_core::McpServerConfig,
    requires_human: bool, late: Option<Arc<LatePolicy>>)
    -> (InstalledApplicationServicePolicy, Arc<PolicyProbe>, Arc<AtomicBool>)
{
    let lifetime = task.source.host_lifetime.get().and_then(Weak::upgrade).unwrap();
    let selected = lifetime.selected_policy();
    let owner = &selected.native_host.as_ref().unwrap().grant_administrator;
    let row = task.source.store.get(&task.source.key(&task.parent).unwrap()).await.unwrap().unwrap();
    let parent = task.source.decode_record(&row, &task.parent).unwrap();
    let route = task.source.route(&task.parent.route).unwrap();
    let permission = PrivateServiceMcpPermission {
        target: route.grant_target.clone(), connection: id(&config.name), resource: id(&config.name),
        operations: [id("read")].into(), lifetime: RoleLifetime {
            scope: LifetimeScope::Task { task: id(task.fresh.as_str()) }, expires_at_ms: Some(task.before_ms),
        },
    };
    let attachment = PrivateServiceMcpLiveAttachment { destination: task.parent_association.candidate().target.clone() };
    let expected = request_digest(&ApplicationServicePolicyRequest {
        owner, deployment: &selected.deployment, source_policy_sha256: &selected.source_policy_sha256,
        route, original: &task.parent, authenticated_event: &parent.event, task: &task.task,
        acquisition: &task.acquisition, fresh_original: &task.fresh, instruction: &task.instruction,
        configuration: config, permission: &permission, attachment: &attachment, tool_policy: None,
    });
    let probe = Arc::new(PolicyProbe::default());
    let current = Arc::new(AtomicBool::new(true));
    let producer = ExactPolicy { expected, owner: owner.clone(), probe: probe.clone(),
        current: current.clone(), requires_human, late };
    (InstalledApplicationServicePolicy::new(lifetime, Arc::new(producer)).unwrap(), probe, current)
}

async fn task(fixture: &Fixture) -> (InstalledServiceTask, SetupSnapshot, InputAuthorityAssociation, String) {
    let admitted = fixture.submit().await;
    fixture.finish(&admitted).await.unwrap();
    let Ok(PrivateOriginalSubmission::Admitted { input_id, .. }) = admitted else { panic!("actual native A") };
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(
        &fixture.session, dispatch.retained_work().unwrap().run_id())).await.unwrap();
    let snapshot = acquisition(fixture, &dispatch).await;
    let member = snapshot.request().host().continuation.native_original.as_ref().unwrap()
        .member.as_ref().unwrap().member.clone();
    let task = fixture.source.prepare_service_task(fixture.store.clone(), snapshot.clone(),
        "application-policy", &member).await.unwrap().unwrap();
    let a = fixture.binding().await.unwrap().unwrap();
    assert!(task.parent == *fixture.original_reference());
    assert!(task.parent_association == a);
    assert!(task.console_login.is_none(), "fixture ingress is not an authenticated Console login");
    (task, snapshot, a, input_id)
}

async fn prepare(task: InstalledServiceTask, config: &meerkat_core::McpServerConfig,
    policy: &InstalledApplicationServicePolicy) -> ApplicationServicePreparation
{
    match meerkat::ConnectorMcpInstallation::from_config(config) {
        Some(installation) => task.prepare_application_connector(policy, &installation.unwrap(), [id("read")].into(), None).await,
        None => task.prepare_application_no_connector(policy, config, [id("read")].into(), None).await,
    }.unwrap()
}

// Use the actual admitted acquisition key. Rehash each altered evidence so a
// stale digest cannot hide a missing deployment or realm isolation check.
#[tokio::test]
async fn application_evidence_rejects_swapped_acquisition_scopes() {
    let (fixture, _store, config, continuation) = Fixture::for_application_service(
        ApplicationServiceSource::StockOperator, |realm| config(realm, false)).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (task, snapshot, _, _) = task(&fixture).await;
        let selected = fixture.lifetime().selected_policy();
        let route = fixture.source.route(&task.parent.route).unwrap();
        let evidence = ApplicationServiceEvidence {
            owner: route.grant_caller.clone(), deployment: selected.deployment.clone(),
            source_policy_sha256: selected.source_policy_sha256.clone(),
            original: task.parent.clone(), acquisition: snapshot.key().clone(),
            instruction_sha256: digest(task.instruction.as_bytes()),
            provenance: [("fixture_policy".into(), "scope-isolation".into())].into_iter().collect(),
            service_parent: None, tool_policy: None, installation_readback: None,
        };
        let mut decision = PrivateServiceMcpAccessDecision {
            catalog: None, route: route.id.clone(), decision: id(task.fresh.as_str()),
            evidence: PrivateSourceEvidence { resource_id: task.fresh.clone(),
                revision: EvidenceId::new("application-service-policy-v1").unwrap(),
                sha256: digest(&canonical_json(&evidence).unwrap()) },
            choice: PrivateAccessChoice::Approve,
            permission: PrivateServiceMcpPermission {
                target: route.grant_target.clone(), connection: id(&config.name),
                resource: id(&config.name), operations: [id("read")].into(),
                lifetime: RoleLifetime { scope: LifetimeScope::Task { task: id(task.fresh.as_str()) },
                    expires_at_ms: Some(task.before_ms) },
            },
            live_attachment: None,
            connection_revision: revision(&digest(&canonical_json(&config).unwrap())),
        };
        assert_eq!(evidence.validate(route, &decision, &fixture.scope.owner), Ok(()));
        for variant in ["bare-owner", "other-owner", "other-realm", "other-deployment"] {
            let mut altered = evidence.clone();
            match variant {
                "bare-owner" => altered.acquisition.owner = fixture.scope.owner.clone(),
                "other-owner" => altered.acquisition.owner = "toolkit/acquisition/another-deployment".into(),
                "other-realm" => altered.acquisition.realm = "another-realm".into(),
                "other-deployment" => {
                    altered.deployment = id("another-deployment");
                    altered.acquisition.owner = "toolkit/acquisition/another-deployment".into();
                },
                _ => unreachable!(),
            }
            decision.evidence.sha256 = digest(&canonical_json(&altered).unwrap());
            assert_eq!(altered.validate(route, &decision, &fixture.scope.owner),
                Err(PrivateOriginalError::Conflict), "{variant}");
        }
    })).catch_unwind().await;
    let cleanup = fixture.close_durable_owners().await;
    drop(continuation);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

async fn fresh_source(fixture: &Fixture, fresh: &EvidenceId) -> Option<(PrivateOriginalRef, StoredRecord, SourceRecord)> {
    fixture.store.list_records(&fixture.scope, None, 256).await.unwrap().into_iter()
        .filter(|row| row.data.schema == SOURCE_SCHEMA)
        .find_map(|row| {
            let record = fixture.source.decode_stored(&row).unwrap();
            (&record.event.original == fresh).then(|| {
                let reference = PrivateOriginalRef { route: record.event.route.clone(), original: fresh.clone(),
                    event_sha256: digest(&canonical_json(&record.event).unwrap()) };
                (reference, row, record)
            })
        })
}

async fn history(fixture: &Fixture, store: &dyn RuntimeStore) -> Vec<StoredInputState> {
    tokio::time::timeout(WAIT, store.load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session)))
        .await.expect("native history deadline").expect("canonical input history")
}

fn config(realm: &str, connector: bool) -> meerkat_core::McpServerConfig {
    let mut config = reviewed_config(realm);
    if !connector {
        config.connector = None;
        config.access = Some(meerkat_core::McpServerAccess::allowing(["read".to_owned()]));
    }
    config
}

async fn positive(source_kind: ApplicationServiceSource, connector: bool) {
    let (fixture, store, config, continuation) = Fixture::for_application_service(
        source_kind, |realm| config(realm, connector)).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (task, snapshot, a, a_id) = task(&fixture).await;
        let parent = task.parent.clone();
        let fresh = task.fresh.clone();
        let before_ms = task.before_ms;
        let a_row = fixture.store.get(&fixture.source.key(&parent).unwrap()).await.unwrap().unwrap();
        let a_record = fixture.source.decode_record(&a_row, &parent).unwrap();
        let route = fixture.source.route(&parent.route).unwrap();
        let configured_parent = route.application_parent.clone().unwrap();
        assert_eq!(configured_parent.len(), 1, "one real preselected finite parent");
        assert!(a_record.approved_task.is_none(), "A has no service task grant");
        let (policy, probe, _) = policy_for(&task, &config, false).await;
        let ApplicationServicePreparation::Prepared(prepared) = prepare(task, &config, &policy).await
            else { panic!("exact fixture application policy permits this task") };
        assert!(prepared.apply(|| true).await.unwrap());
        assert_eq!(probe.reserved.load(Ordering::SeqCst), 1);
        assert!(probe.checked.load(Ordering::SeqCst) > 1);
        assert_eq!(probe.dropped.load(Ordering::SeqCst), 1, "settled direct invocation releases policy custody");
        assert_eq!(history(&fixture, store.as_ref()).await.len(), 1, "issuing access is not native B admission");
        let (b, row, record) = fresh_source(&fixture, &fresh).await.unwrap();
        assert!(b != parent && record.event.content_sha256 == digest(INSTRUCTION.as_bytes()));
        assert!(record.source == a_record.source && record.event.contributors == a_record.event.contributors);
        assert!(record.final_association.is_none() && record.service_admission.is_none());
        let approved = record.approved_task.as_ref().unwrap();
        assert!(approved.parent == parent && approved.acquisition == *snapshot.key());
        assert_eq!(approved.before_ms, before_ms);
        assert_eq!(approved.original_task.origin_session_id, fixture.session);
        let work = OriginalWorkRef { authority: route.source_authority.clone(), work: fresh.clone() };
        let lineage = fixture.source.service_grant_lineage(approved.singleton_grant().unwrap(), &work).await.unwrap();
        assert_eq!(lineage.len(), 2);
        assert!(lineage[..1] == configured_parent[..], "native child attenuates the selected parent");
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
        for (operation, expected) in [("read", true), ("write", false)] {
            let values = crate::installed_host::application_operations::mcp_operation(
                &route.grant_caller, &route.logical_executor, &route.grant_audience, &config.name, operation);
            assert_eq!(resolved.restrictions().check_bounds(OperationRestrictionValues {
                action: &values.action, resource_domain: &values.resource_domain, processor: &values.processor,
                audience: &values.audience, now_ms: now,
            }).is_ok(), expected);
        }
        let values = crate::installed_host::application_operations::mcp_operation(
            &route.grant_caller, &route.logical_executor, &route.grant_audience, &config.name, "read");
        assert!(resolved.restrictions().check_bounds(OperationRestrictionValues {
            action: &values.action, resource_domain: &values.resource_domain, processor: &values.processor,
            audience: &values.audience, now_ms: before_ms.checked_add(1).unwrap(),
        }).is_err(), "native child access is finite even while its fixture parent remains current");
        let configuration_revision = revision(&digest(&canonical_json(&config).unwrap()));
        for (configuration_revision, expected) in [(configuration_revision, true), (revision(&"0".repeat(64)), false)] {
            let selection_digest = digest(&canonical_json(&(&config.name, &configuration_revision, &a.candidate().target)).unwrap());
            let values = crate::installed_host::application_operations::mcp_live_attachment(
                &route.grant_caller, &route.logical_executor, &route.grant_audience, &selection_digest);
            assert_eq!(resolved.restrictions().check_bounds(OperationRestrictionValues {
                action: &values.action, resource_domain: &values.resource_domain, processor: &values.processor,
                audience: &values.audience, now_ms: now,
            }).is_ok(), expected, "attachment authority matches the exact real destination and configuration");
        }
        let rows = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
        let decision = rows.iter().find(|row| row.data.version == 4
            && row.data.body["application_policy"]["original"] == serde_json::to_value(&parent).unwrap()).unwrap();
        assert_eq!(decision.data.body["application_policy"]["instruction_sha256"], digest(INSTRUCTION.as_bytes()));
        assert_eq!(decision.data.body["issuance"]["state"], "issued");
        assert_eq!(decision.data.body["parent"], serde_json::to_value(&configured_parent).unwrap());
        assert!(fixture.binding().await.unwrap().unwrap() == a, "application approval never relabels A");

        // Enter the existing native B source/input path. Physical MCP Apply is
        // intentionally not performed by this source-admission control.
        fixture.source.reserve_service_continuation(&row).await.unwrap();
        fixture.source.step_service_input(&row).await.unwrap();
        let saved = fixture.store.get(&row.key).await.unwrap().unwrap();
        let b_record = fixture.source.decode_record(&saved, &b).unwrap();
        let admission = b_record.service_admission.as_ref().unwrap();
        assert_ne!(admission.input_id, a_id);
        let b_association = b_record.final_association.as_ref().unwrap();
        let native = history(&fixture, store.as_ref()).await;
        assert_eq!(native.len(), 2);
        for (id, association) in [(&a_id, &a), (&admission.input_id, b_association)] {
            let row = native.iter().find(|row| row.state.input_id.to_string() == *id).unwrap();
            assert!(row.state.authority_contributors.iter().any(|contributor|
                contributor.input_id() == &row.state.input_id && contributor.association() == association));
        }
        let obligation = fixture.source.service_setup_store().unwrap().committed_continuation(
            &id(&snapshot.key().id)).await.unwrap().unwrap();
        let evidence = fixture.source.observe_service_continuation(&obligation).await.unwrap().unwrap();
        assert_eq!(evidence.durable_input.id.as_str(), admission.input_id);
        assert!(continuation.admit(&obligation).await.unwrap().durable_input == evidence.durable_input);
        fixture.source.step_service_input(&saved).await.unwrap();
        assert!(continuation.admit(&obligation).await.unwrap().durable_input == evidence.durable_input);
        assert_eq!(history(&fixture, store.as_ref()).await.len(), 2, "no replay of A or duplicate B");
    })).catch_unwind().await;
    let cleanup = fixture.close_durable_owners().await;
    drop(continuation);
    drop(store);
    drop(fixture);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_service_connector_parent_issues_child_and_fresh_native_continuation() {
    positive(ApplicationServiceSource::ApplicationService, true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_service_no_connector_parent_issues_child_and_fresh_native_continuation() {
    positive(ApplicationServiceSource::ApplicationService, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_operator_policy_uses_same_fresh_native_continuation_without_console_claim() {
    positive(ApplicationServiceSource::StockOperator, true).await;
}

#[derive(Clone, Copy)]
enum Stop { RequiresHuman, CancelBeforeIssue, PolicyInvalidated, CancelAfterIssue }
async fn stopped(stop: Stop) {
    let (fixture, store, config, continuation) = Fixture::for_application_service(
        ApplicationServiceSource::ApplicationService, |realm| config(realm, false)).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (task, _, _, _) = task(&fixture).await;
        let parent = task.parent.clone();
        let fresh = task.fresh.clone();
        let (policy, probe, current) = policy_for(&task, &config, matches!(stop, Stop::RequiresHuman)).await;
        let before = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
        let prepared = prepare(task, &config, &policy).await;
        if matches!(stop, Stop::RequiresHuman) {
            assert!(matches!(prepared, ApplicationServicePreparation::RequiresHuman));
            assert_eq!(fixture.store.list_records(&fixture.scope, None, 256).await.unwrap(), before);
            assert_eq!(probe.dropped.load(Ordering::SeqCst), 0, "no reservation was issued");
        } else {
            let ApplicationServicePreparation::Prepared(prepared) = prepared else { panic!("reserved exact task") };
            if matches!(stop, Stop::CancelBeforeIssue) {
                fixture.source.record_task_group_cancelled(&parent).await.unwrap();
            }
            if matches!(stop, Stop::PolicyInvalidated) { current.store(false, Ordering::SeqCst); }
            let settled = prepared.apply(|| true).await;
            if matches!(stop, Stop::CancelAfterIssue) {
                assert!(settled.unwrap());
                let (b, row, _) = fresh_source(&fixture, &fresh).await.unwrap();
                fixture.source.record_task_group_cancelled(&parent).await.unwrap();
                let current = fixture.store.get(&row.key).await.unwrap().unwrap();
                let cancelled = fixture.source.decode_record(&current, &b).unwrap();
                assert!(!fixture.source.service_task_submission_current(&cancelled).await.unwrap());
                let _ = fixture.source.step_service_input(&current).await;
                assert!(fixture.store.list_records(&fixture.scope, None, 256).await.unwrap().iter()
                    .any(|row| row.data.version == 4 && row.data.body["issuance"]["state"] == "issued"),
                    "cancellation does not claim to roll back an already issued native child");
            } else {
                assert!(settled.is_err());
                assert!(fresh_source(&fixture, &fresh).await.is_none());
                assert!(!fixture.store.list_records(&fixture.scope, None, 256).await.unwrap().iter()
                    .any(|row| row.data.version == 4), "pre-entry refusal has no native issue decision");
            }
            assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
        }
        assert_eq!(probe.reserved.load(Ordering::SeqCst), 1);
        assert_eq!(history(&fixture, store.as_ref()).await.len(), 1, "no cancelled, denied or human-required B input");
    })).catch_unwind().await;
    let cleanup = fixture.close_durable_owners().await;
    drop(continuation);
    drop(store);
    drop(fixture);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_policy_requires_human_has_no_native_issue_or_fresh_source() { stopped(Stop::RequiresHuman).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_task_cancellation_before_issue_keeps_only_native_a() { stopped(Stop::CancelBeforeIssue).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalidated_application_policy_keeps_only_native_a() { stopped(Stop::PolicyInvalidated).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_task_cancellation_after_issue_never_admits_native_b() { stopped(Stop::CancelAfterIssue).await; }

async fn prepared_then_late_failure(failure: LateFailure) {
    let (fixture, store, config, continuation) = Fixture::for_application_service(
        ApplicationServiceSource::ApplicationService, |realm| config(realm, false)).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let (task, _, _, _) = task(&fixture).await;
        let fresh = task.fresh.clone();
        let late = Arc::new(LatePolicy {
            source_path: fixture.source_path.clone(), realm: fixture.scope.realm.clone(),
            owner: fixture.scope.owner.clone(), fresh: fresh.as_str().to_owned(), failure,
            active: AtomicBool::new(true), observed_pending: AtomicUsize::new(0),
        });
        let (policy, probe, _) = policy_for_with_late(&task, &config, false, Some(late.clone())).await;
        let ApplicationServicePreparation::Prepared(prepared) = prepare(task.clone(), &config, &policy).await
            else { panic!("actual application policy preparation succeeds before the later store await") };
        assert_eq!(probe.reserved.load(Ordering::SeqCst), 1);
        assert_eq!(late.observed_pending.load(Ordering::SeqCst), 0);
        let grant_path = fixture.source_path.parent().unwrap().join("grants.jsonl");
        let before = std::fs::read(&grant_path).unwrap();
        let result = prepared.apply(|| true).await;
        let expected = match failure { LateFailure::Denied => PrivateOriginalError::Denied, _ => PrivateOriginalError::Unavailable };
        assert!(matches!(result, Err(actual) if actual == expected));
        assert_eq!(late.observed_pending.load(Ordering::SeqCst), 1,
            "late failure originates from the existing entered worker after real Pending admission");
        assert_eq!(probe.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read(&grant_path).unwrap(), before, "no native child was issued");
        assert!(fresh_source(&fixture, &fresh).await.is_none());
        let rows = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
        let decision = rows.iter().find(|row| row.data.version == 4
            && row.data.body["decision"]["decision"] == fresh.as_str()).unwrap().clone();
        assert_eq!(decision.data.body["issuance"]["state"], match failure {
            LateFailure::Denied => "native_refused", _ => "pending_native_issuance",
        });
        // Re-prepare the same actual task after policy becomes observable.
        // The historical access record cannot authorize a second native issue.
        late.active.store(false, Ordering::SeqCst);
        let ApplicationServicePreparation::Prepared(replay) = prepare(task, &config, &policy).await
            else { panic!("same exact request remains preparable") };
        assert!(matches!(replay.apply(|| true).await, Err(PrivateOriginalError::Unavailable)));
        assert_eq!(fixture.store.get(&decision.key).await.unwrap().unwrap(), decision);
        assert_eq!(std::fs::read(&grant_path).unwrap(), before);
        assert!(fresh_source(&fixture, &fresh).await.is_none());
        assert_eq!(history(&fixture, store.as_ref()).await.len(), 1);
    })).catch_unwind().await;
    let cleanup = fixture.close_durable_owners().await;
    drop(continuation);
    drop(store);
    drop(fixture);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_application_policy_late_unavailable_preserves_pending_without_reissue() {
    prepared_then_late_failure(LateFailure::Unavailable).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_application_policy_late_not_settled_preserves_pending_without_reissue() {
    prepared_then_late_failure(LateFailure::NotSettled).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepared_application_policy_late_denial_is_definite_without_reissue() {
    prepared_then_late_failure(LateFailure::Denied).await;
}
