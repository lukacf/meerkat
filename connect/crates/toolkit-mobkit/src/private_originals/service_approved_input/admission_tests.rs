//! Real native A/B admissions and canonical SQLite acknowledgement failure.
//! The existing installed fixture scripts model transport and the protected
//! review decision; it does not exercise an OS presenter or an MCP server.
use super::*;
use futures::FutureExt;
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalResourceId, ApprovalResourceKind,
    ApprovalResourceRef, ApprovalRisk, ApprovalService, ToolDispatchContext,
    lifecycle::InputId, turn_execution_authority::TurnTerminalOutcome,
};
use meerkat_runtime::SessionServiceRuntimeExt;
use meerkat_toolkit::{
    authority::{Action, ActionPermission, Ceiling, SetupProposalConstraints,
        SetupRequestEvidence, SetupUseContext},
    request::{ContextReference, ContextReferenceOwner, FindAndInstallRequest, HostSetupRequest,
        RequestError, SetupLimits, TaskContinuation, validate_request},
};
use std::{panic::AssertUnwindSafe, time::Duration};

const WAIT: Duration = Duration::from_secs(10);
const INSTRUCTION: &str = "Read the reviewed inbox for this task";
#[path = "continuation_tests.rs"]
mod continuation_tests;
#[path = "application_policy_tests.rs"]
mod application_policy_tests;
#[path = "compensation_tests.rs"]
mod compensation_tests;
#[path = "service_recovery_tests.rs"]
mod service_recovery_tests;
#[path = "batch_recovery_tests.rs"]
pub(super) mod batch_recovery_tests;
fn id(value: &str) -> Id { Id::new(value).unwrap() }
fn revision(value: &str) -> Revision { Revision::new(value).unwrap() }

struct NoReferences;
impl ContextReferenceOwner for NoReferences {
    fn verify_access(&self, _: &ContextReference, _: &SetupRequestEvidence,
        _: &SetupUseContext<'_>) -> Result<(), RequestError>
    { Err(RequestError::ContextUnavailable) }
}

pub(super) async fn acquisition<S>(fixture: &ContinuationFixture<S>, dispatch: &ToolDispatchContext)
    -> SetupSnapshot
{
    let association = fixture.binding().await.unwrap().unwrap();
    acquisition_for_association(fixture, dispatch, association).await
}

// The caller must read this association from the actual admitted source. The
// ordinary fixture keeps its historical source; Console fixtures use the source
// retained by their real verified ingress instead of a synthetic login handle.
async fn acquisition_for_association<S>(fixture: &ContinuationFixture<S>, dispatch: &ToolDispatchContext,
    association: meerkat_authorization_contracts::work_association::InputAuthorityAssociation) -> SetupSnapshot
{
    let tenant = id(&fixture.scope.realm);
    let deployment = id(&fixture.scope.owner);
    let policy = revision("test-policy");
    let target = association.candidate().target.clone();
    let authorization = dispatch.work_authorization().unwrap();
    let retained = dispatch.retained_work().unwrap().clone();
    let meerkat_core::exact_operation::OperationExecutionScope::RuntimeInput {
        owner_session_id, canonical_input_id, ..
    } = authorization.execution_scope() else { panic!("actual staged A input required") };
    assert_eq!(owner_session_id, &fixture.session);
    let now = u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap();
    let evidence = SetupRequestEvidence::new(association, SetupProposalConstraints {
        tenant: tenant.clone(), deployment: deployment.clone(), policy_revision: policy.clone(),
        issued_at_ms: now, expires_at_ms: now + 60_000,
        ceiling: Ceiling {
            actions: [ActionPermission { action: Action::FindAndInstall, resource: id("agent") }].into(),
            services: Default::default(), frontends: Default::default(),
            output_audiences: [id("person")].into(),
        },
    }).unwrap();
    let context = SetupUseContext {
        tenant: &tenant, deployment: &deployment,
        realm: &evidence.association().candidate().ingress_namespace.realm,
        target: &target, policy_revision: &policy, now_ms: now, authorization,
    };
    let request = validate_request(FindAndInstallRequest {
        request: INSTRUCTION.into(), user_context: vec![], caller_observations: vec![],
    }, HostSetupRequest {
        setup_helper: None,
        target: id("agent"), correlation: id("service-admission-ack"), parent_operation: None,
        continuation: TaskContinuation {
            owner: id("service-admission-test"), task: id(&retained.run_id().to_string()),
            revision: revision(&canonical_input_id.to_string()),
            delivery_key: id("host:service-admission:original"), audience: [id("person")].into(),
            native_original: Some(NativeOriginalTask {
                origin_session_id: fixture.session.clone(), retained_work: retained,
                member: Some(fixture.lifetime().selected_policy().native_host.as_ref().unwrap().members[0].member.clone()),
            }),
        },
        environment_revision: revision("test-environment"),
        limits: SetupLimits { deadline_ms: now + 60_000, max_workers: 1, max_effect_attempts: 2 },
    }, &evidence, &context, &NoReferences).unwrap();
    SetupStore::new_conditional(fixture.store.as_ref(), SetupScope::new(tenant.clone(), deployment.clone())).unwrap()
        .admit(&request, &evidence, &context, &NoReferences).await.unwrap().snapshot
}

pub(super) fn reviewed_config(realm: &str) -> meerkat_core::mcp_config::McpServerConfig {
    let mut config = meerkat_core::mcp_config::McpServerConfig::stdio(
        "mail", "not-executed", vec![], Default::default());
    config.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
        realm_id: realm.to_owned(), slot_id: "slot".into(), issuer: "https://issuer.invalid".into(),
        client: "client".into(), resource: "https://mail.invalid".into(),
        strategy_id: "oauth".into(), account: "reviewed-account".into(),
        operations: [("read".into(), meerkat_core::mcp_config::McpConnectorOperation {
            required_scopes: ["mail.read".into()].into(),
            destination_origin: "https://mail.invalid".into(), read_only: true,
        })].into(),
    });
    config
}

pub(super) async fn approve_b(fixture: &ContinuationFixture, snapshot: SetupSnapshot)
    -> PrivateOriginalRef
{
    approve_b_operations(fixture, snapshot, [id("read")].into()).await
}

pub(super) fn reviewed_config_operations(realm: &str, operations: &std::collections::BTreeSet<Id>)
    -> meerkat_core::mcp_config::McpServerConfig
{
    let mut config = reviewed_config(realm);
    let connector = config.connector.as_mut().unwrap();
    let selected = connector.operations["read"].clone();
    connector.operations = operations.iter().map(|operation|
        (operation.as_str().to_owned(), selected.clone())).collect();
    config
}

pub(super) async fn approve_b_operations<S>(fixture: &ContinuationFixture<S>, snapshot: SetupSnapshot,
    operations: std::collections::BTreeSet<Id>) -> PrivateOriginalRef
{
    approve_b_configuration(fixture, snapshot, operations, None).await
}

pub(super) async fn approve_b_configuration<S>(fixture: &ContinuationFixture<S>, snapshot: SetupSnapshot,
    operations: std::collections::BTreeSet<Id>, config: Option<meerkat_core::McpServerConfig>) -> PrivateOriginalRef
{
    approve_b_call(fixture, snapshot, operations, config, "service-admission-ack").await
}

pub(super) async fn approve_b_configuration_with_tool_policy<S>(
    fixture: &ContinuationFixture<S>, snapshot: SetupSnapshot,
    operations: std::collections::BTreeSet<Id>, config: meerkat_core::McpServerConfig,
    tool_policy: ServiceToolPolicySelection,
) -> PrivateOriginalRef {
    approve_b_call_with_tool_policy(fixture, snapshot, operations, Some(config), "service-admission-ack",
        Some(tool_policy)).await
}

async fn approve_b_call<S>(fixture: &ContinuationFixture<S>, snapshot: SetupSnapshot,
    operations: std::collections::BTreeSet<Id>, config: Option<meerkat_core::McpServerConfig>,
    call: &str) -> PrivateOriginalRef
{
    approve_b_call_with_tool_policy(fixture, snapshot, operations, config, call, None).await
}

async fn approve_b_call_with_tool_policy<S>(fixture: &ContinuationFixture<S>, snapshot: SetupSnapshot,
    operations: std::collections::BTreeSet<Id>, config: Option<meerkat_core::McpServerConfig>,
    call: &str, tool_policy: Option<ServiceToolPolicySelection>) -> PrivateOriginalRef
{
    let member = snapshot.request().host().continuation.native_original.as_ref().unwrap()
        .member.as_ref().unwrap().clone();
    let task = fixture.source.prepare_service_task(fixture.store.clone(), snapshot,
        call, &member.member).await.unwrap().unwrap();
    let fresh = task.fresh.clone();
    let realm = task.parent_association.candidate().ingress_namespace.realm.to_string();
    let config = config.unwrap_or_else(|| reviewed_config_operations(&realm, &operations));
    let request = ApprovalRequest {
        requester: ApprovalPrincipalId::new("installed-service-fixture").unwrap(),
        owner: ApprovalOwnerRef::Session { session_id: fixture.session.clone() },
        resource: ApprovalResourceRef { kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-application") },
        proposed_action: ApprovalProposedAction { kind: ApprovalActionKind::Other,
            summary: "Approve the fresh inbox instruction".into(), body: Some(serde_json::json!({})) },
        risk: ApprovalRisk::High, request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: None, metadata: Default::default(), request_provenance: None,
    };
    let selected_policy = tool_policy.clone();
    let review = match meerkat::ConnectorMcpInstallation::from_config(&config) {
        Some(installation) => task.prepare_review_with_tool_policy(
            &installation.unwrap(), operations, None, request, tool_policy).await,
        None => task.prepare_no_connector_review_with_tool_policy(
            &config, operations, None, request, tool_policy).await,
    }.unwrap();
    let body = review.request().proposed_action.body.as_ref().unwrap();
    let original_target = fixture.binding().await.unwrap().unwrap().candidate().target.clone();
    assert_eq!(body["service_access"]["live_attachment"]["destination"], serde_json::to_value(&original_target).unwrap());
    assert_eq!(body["access_request"]["service_instruction"]["configuration_persistence"], "not authorized by this review");
    let policy_proposal = selected_policy.as_ref().map(|selection| {
        let proposal: ServiceToolPolicyProposal = serde_json::from_value(
            body["access_request"]["member_tool_policy"]["proposal"].clone()).unwrap();
        let candidate = meerkat_core::CompiledApplicationToolPolicy::parse_canonical_json(
            proposal.candidate_canonical_json.as_bytes()).unwrap();
        let initial = &fixture.lifetime().selected_policy().native_host.as_ref().unwrap()
            .application_tool_policy;
        assert_eq!(proposal.member, member.member);
        assert_eq!(&proposal.selection, selection);
        assert_eq!(proposal.catalog_session, selection.catalog.as_ref().map(|_| fixture.session.to_string()));
        assert_eq!(proposal.prior.revision, meerkat_core::PolicyRevision(1));
        assert_eq!(proposal.prior.digest.as_str(), format!("sha256:{}", initial.sha256));
        assert_eq!(proposal.candidate.revision, meerkat_core::PolicyRevision(2));
        assert!(candidate.default_deny);
        // This captured Original begins with installation discovery only.
        // The real owner must preserve it in the complete reviewed successor.
        let bootstrap_grant = meerkat_core::CompiledMemberToolGrant {
            tool_name: crate::FIND_AND_INSTALL.into(),
            action: meerkat_core::CompiledMemberToolAction::Invoke,
            consequence: meerkat_core::CompiledToolConsequence::R2,
        };
        let mut expected_grants = selection.grants().unwrap();
        expected_grants.push(bootstrap_grant);
        expected_grants.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        assert_eq!(candidate.members, vec![meerkat_core::CompiledMemberToolGrants {
            member_identity: member.member.clone(), grants: expected_grants,
        }], "the full reviewed successor preserves bootstrap and adds only the selected permissions");
        assert!(!fixture.source.service_tool_policy.get().unwrap()
            .is_observed_current(proposal.candidate_canonical_json.as_bytes()).unwrap(),
            "the proposed policy is not current before the human decision");
        proposal
    });
    if selected_policy.is_none() {
        assert!(body["access_request"].get("member_tool_policy").is_none(),
            "ordinary fixture reviews do not add application tool grants");
    }
    let approvals = ApprovalService::new();
    let pending = approvals.request(review.request().clone()).unwrap();
    let approved = approvals.decide(&pending.approval_id, ApprovalDecision::Approve,
        ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(), None, None).unwrap();
    assert!(review.approve(approved, || true).await.unwrap());
    if let Some(proposal) = policy_proposal {
        assert!(fixture.source.service_tool_policy.get().unwrap()
            .is_observed_current(proposal.candidate_canonical_json.as_bytes()).unwrap(),
            "the actual human-reviewed complete policy must be observed current");
    }
    let page = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
    let record = page.iter().filter(|row| row.data.schema == SOURCE_SCHEMA)
        .map(|row| fixture.source.decode_stored(row).unwrap())
        .find(|record| record.event.original == fresh).unwrap();
    assert!(record.approved_task.is_some());
    assert!(record.final_association.is_none());
    assert!(record.service_admission.is_none());
    PrivateOriginalRef { route: record.event.route.clone(), original: fresh,
        event_sha256: digest(&canonical_json(&record.event).unwrap()) }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_b_survives_lost_source_ack_after_a_terminal_is_overwritten() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let a = fixture.submit().await;
        fixture.finish(&a).await.unwrap();
        let dispatch = fixture.probe.as_ref().unwrap().captured();
        let a_run = dispatch.retained_work().unwrap().run_id().clone();
        tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(&fixture.session, &a_run))
            .await.unwrap();
        assert_eq!(fixture.machine.run_turn_terminal(&fixture.session, &a_run).await,
            Some(TurnTerminalOutcome::Completed));
        assert_eq!(fixture.inputs().await, Ok(1));
        let requests_before_b = fixture.model_requests();
        let snapshot = acquisition(&fixture, &dispatch).await;
        let b = approve_b(&fixture, snapshot).await;
        let source = &fixture.source;
        let key = source.key(&b).unwrap();
        let stored = source.store.get(&key).await.unwrap().unwrap();
        assert!(source.service_task_submission_current(&source.decode_record(&stored, &b).unwrap()).await.unwrap());

        // Fail only the real canonical acknowledgement UPDATE. B's initial
        // row, native input, source association and native grants stay real.
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(&format!(
            "CREATE TRIGGER fail_service_admission_ack BEFORE UPDATE OF data ON toolkit_records \
             WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.service_admission') IS NOT NULL \
             BEGIN SELECT RAISE(ABORT, 'fixture lost service admission write'); END;",
            key.id.replace('\'', "''"),
        )).unwrap();
        assert!(matches!(source.step_service_input(&stored).await, Err(PrivateOriginalError::Unavailable)));
        assert_eq!(fixture.inputs().await, Ok(2), "native accepted B despite the failed source acknowledgement");
        let unacknowledged = source.store.get(&key).await.unwrap().unwrap();
        let record = source.decode_record(&unacknowledged, &b).unwrap();
        assert!(record.service_admission.is_none());
        assert!(record.final_association.is_some());
        let held = source.retained.lock().unwrap().iter().find(|held| held.reference == b).unwrap().clone();
        assert_eq!(held.unresolved.lock().unwrap().len(), 1, "settled service input stays in its original custody");
        let Some(PrivateOriginalSubmission::Admitted { session_id, input_id,
            source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, .. }) =
            source.reconcile_original_submission(&b).await.unwrap()
            else { panic!("exact native input must reconcile with the same settled source guard") };
        assert_eq!(held.unresolved.lock().unwrap().len(), 1, "reconciliation is not a durable source acknowledgement");
        assert_eq!(session_id, fixture.session.to_string());
        let native_input = InputId::from_uuid(input_id.parse().unwrap());
        let mut settlements = fixture.machine.subscribe_run_settlements();
        tokio::time::timeout(WAIT, async {
            loop {
                let actual = fixture.machine.input_state(&fixture.session, &native_input).await.unwrap().unwrap();
                if let Some(run) = &actual.seed.last_run_id {
                    if fixture.machine.run_turn_terminal(&fixture.session, run).await == Some(TurnTerminalOutcome::Completed)
                        && fixture.machine.current_run(&fixture.session).await.as_ref() != Some(run)
                    { break; }
                }
                settlements.changed().await.unwrap();
            }
        }).await.expect("actual B run completes and settles");
        assert_eq!(fixture.machine.run_turn_terminal(&fixture.session, &a_run).await, None);
        assert!(!source.service_task_submission_current(&record).await.unwrap(), "A's overwritten witness cannot permit fresh submission");
        assert_eq!(fixture.model_requests(), requests_before_b + 1);

        sql.execute_batch("DROP TRIGGER fail_service_admission_ack;").unwrap();
        let mut after = None;
        loop {
            let page = source.step_service_inputs(&fixture.store, after.as_deref(), 1).await.unwrap();
            assert!(!page.had_unavailable_records);
            after = page.next_after_id;
            if after.is_none() { break; }
        }
        let saved = source.store.get(&key).await.unwrap().unwrap();
        let acknowledged = source.decode_record(&saved, &b).unwrap();
        assert!(acknowledged.service_admission == Some(ServiceInputAdmission { session_id, input_id }));
        assert!(acknowledged.final_association == record.final_association);
        assert_eq!(source.service_task_snapshot(&acknowledged).await.unwrap().state().lifecycle_phase,
            toolkit_catalog::AcquisitionPhase::Ready,
            "durable B admission must also settle the existing A acquisition");
        assert!(held.unresolved.lock().unwrap().is_empty());
        assert!(source.reconcile_original_submission(&b).await.unwrap().is_none());
        source.step_service_input(&saved).await.unwrap();
        assert_eq!(source.store.get(&key).await.unwrap().unwrap(), saved);
        assert_eq!(fixture.inputs().await, Ok(2));
        assert_eq!(fixture.model_requests(), requests_before_b + 1, "recovery makes no new native input or model request");
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_attachment_preparation_reopens_cancelled_task_and_never_submits_b() {
    use meerkat_core::mcp_attachment_control::{McpAttachmentControlRequest, McpAttachmentEffect, McpAttachmentFacts};
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let a = fixture.submit().await;
        fixture.finish(&a).await.unwrap();
        let dispatch = fixture.probe.as_ref().unwrap().captured();
        let run = dispatch.retained_work().unwrap().run_id();
        tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(&fixture.session, run)).await.unwrap();
        let snapshot = acquisition(&fixture, &dispatch).await;
        let b = approve_b(&fixture, snapshot).await;
        let source = &fixture.source;
        let parent = fixture.binding().await.unwrap().unwrap();
        let claims = parent.candidate();
        let config = reviewed_config(&claims.ingress_namespace.realm.to_string());
        let control = McpAttachmentControlRequest::from_trusted_ingress(fixture.session.clone(), config,
            McpAttachmentFacts {
                requester: claims.requester.clone(), actor: claims.ingress_actor.clone(),
                realm: claims.ingress_namespace.realm.clone(),
                destination: meerkat_runtime::meerkat_machine::scoped_destination_target(&claims.target),
            }, Arc::new(())).unwrap();
        let binding = control.effect_binding(McpAttachmentEffect::Apply);
        let key = source.key(&b).unwrap();
        let before = source.store.get(&key).await.unwrap().unwrap();
        let requests = fixture.model_requests();
        source.observe_service_mcp_attachment(&b, &control, &binding).await.unwrap();
        assert_eq!(source.store.get(&key).await.unwrap().unwrap(), before);
        assert_eq!(fixture.inputs().await, Ok(1));
        assert_eq!(fixture.model_requests(), requests);
        source.record_task_group_cancelled(&b).await.unwrap();
        let cancelled = source.store.get(&key).await.unwrap().unwrap();
        assert!(source.decode_record(&cancelled, &b).unwrap().operation_cancelled);
        assert!(matches!(source.observe_service_mcp_attachment(&b, &control, &binding).await,
            Err(PrivateOriginalError::Denied)));
        assert_eq!(source.store.get(&key).await.unwrap().unwrap(), cancelled);
        assert_eq!(fixture.inputs().await, Ok(1), "preparation did not admit B");
        assert_eq!(fixture.model_requests(), requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[path = "source_status_tests.rs"]
mod source_status_tests;

#[path = "attachment_entry_tests.rs"]
mod attachment_entry_tests;


/// A barrier on the mechanical package port, not an attachment authority. The
/// real A, B, review, parent grant, SQLite writer and currentness code run below.
struct HeldPackageReadback {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    config: meerkat_core::McpServerConfig,
}
#[async_trait::async_trait]
impl ServiceInstallationReadback for HeldPackageReadback {
    async fn readback(&self, _: &ApprovalRecord)
        -> Result<meerkat_core::McpServerConfig, PrivateOriginalError>
    {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(self.config.clone())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn package_readback_keeps_old_publication_and_cancellation_wins_before_b() {
    for mutation in ["none", "cas", "cancel"] {
        let cancel = mutation == "cancel";
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let a = fixture.submit().await;
            fixture.finish(&a).await.unwrap();
            let dispatch = fixture.probe.as_ref().unwrap().captured();
            let run = dispatch.retained_work().unwrap().run_id().clone();
            tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(&fixture.session, &run)).await.unwrap();
            assert_eq!(fixture.machine.run_turn_terminal(&fixture.session, &run).await, Some(TurnTerminalOutcome::Completed));
            let snapshot = acquisition(&fixture, &dispatch).await;
            let b = approve_b(&fixture, snapshot).await;
            let source = fixture.source.clone();
            let key = source.key(&b).unwrap();
            let stored = source.store.get(&key).await.unwrap().unwrap();
            let source_record = source.decode_record(&stored, &b).unwrap();
            let parent = source_record.approved_task.as_ref().unwrap().parent.clone();
            let readback = Arc::new(HeldPackageReadback {
                entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new(),
                config: reviewed_config(&source.scope.realm),
            });
            source.bind_service_installation_readback(readback.clone()).unwrap();
            let model_requests = fixture.model_requests();
            let work = source.prepare_service_mcp_installation(&fixture.store, &b);
            tokio::pin!(work);
            // Poll the actual preparation through the durable review read to
            // the held package call, then mutate its real existing owner.
            tokio::select! {
                _ = readback.entered.notified() => (),
                result = &mut work => panic!("package barrier was not reached: {}", result.is_ok()),
                _ = tokio::time::sleep(WAIT) => panic!("package readback was not entered"),
            }
            if cancel {
                source.record_task_group_cancelled(&parent).await.unwrap();
            } else if mutation == "cas" {
                // A successful canonical CAS invalidates the old publication
                // even with unchanged values. Re-capturing after readback would
                // incorrectly turn this control into a newly current one.
                source.store.compare_and_swap(Transition {
                    key: stored.key.clone(), expected_revision: stored.revision,
                    expected_digest: Some(stored.content_digest.clone()), claim: None,
                    data: stored.data.clone(), events: vec![], effects: vec![],
                }).await.unwrap();
            }
            readback.release.notify_one();
            let prepared = tokio::time::timeout(WAIT, &mut work).await.unwrap();
            if mutation == "none" {
                assert!(prepared.is_ok(), "the unchanged actual B owner reaches control preparation");
            } else {
                assert!(matches!(prepared, Err(PrivateOriginalError::Denied | PrivateOriginalError::Unavailable)));
            }
            let after = source.store.get(&key).await.unwrap().unwrap();
            let after = source.decode_record(&after, &b).unwrap();
            assert_eq!(after.operation_cancelled, cancel);
            assert!(after.final_association.is_none() && after.service_admission.is_none());
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), model_requests);
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_pending_cleanup_observes_only_definite_canonical_terminal_without_new_input() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let a = fixture.submit().await;
        fixture.finish(&a).await.unwrap();
        let dispatch = fixture.probe.as_ref().unwrap().captured();
        let run = dispatch.retained_work().unwrap().run_id();
        tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(&fixture.session, run)).await.unwrap();
        let snapshot = acquisition(&fixture, &dispatch).await;
        let member = snapshot.request().host().continuation.native_original.as_ref().unwrap()
            .member.as_ref().unwrap().member.clone();
        let task = fixture.source.prepare_service_task(fixture.store.clone(), snapshot,
            "console-terminal-observation", &member).await.unwrap().unwrap();
        let key = fixture.source.key(&task.parent).unwrap();
        let before = fixture.source.store.get(&key).await.unwrap().unwrap();
        let requests = fixture.model_requests();
        assert_eq!(task.console_terminal().await, Ok(false));
        let mut mismatched = task.clone();
        mismatched.input_digest = "wrong-acquisition".into();
        assert_eq!(mismatched.console_terminal().await, Err(PrivateOriginalError::Conflict));
        assert_eq!(fixture.source.store.get(&key).await.unwrap().unwrap(), before);
        fixture.source.record_task_group_cancelled(&task.parent).await.unwrap();
        let cancelled = fixture.source.store.get(&key).await.unwrap().unwrap();
        assert!(fixture.source.decode_record(&cancelled, &task.parent).unwrap().operation_cancelled);
        assert_eq!(task.console_terminal().await, Ok(true));
        assert_eq!(fixture.source.store.get(&key).await.unwrap().unwrap(), cancelled);
        assert_eq!(fixture.inputs().await, Ok(1));
        assert_eq!(fixture.model_requests(), requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[path = "cold_service_tests.rs"]
mod cold_service_tests;


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_mcp_review_uses_the_actual_waiting_task_and_separate_no_connector_grant() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let original = fixture.submit().await;
        fixture.finish(&original).await.unwrap();
        let dispatch = fixture.probe.as_ref().unwrap().captured();
        let snapshot = acquisition(&fixture,&dispatch).await;
        assert!(matches!(fixture.source.prepare_service_task(fixture.store.clone(),snapshot.clone(),
            "wrong-consumer", "unrelated-agent").await, Err(PrivateOriginalError::Denied)));
        let captured = snapshot.request().host().continuation.native_original.as_ref().unwrap()
            .member.as_ref().unwrap().member.clone();
        let task = fixture.source.prepare_service_task(fixture.store.clone(), snapshot.clone(),
            "captured-consumer", &captured).await.unwrap().unwrap();
        assert_eq!(task.consumer_member(), Some(captured.as_str()));
        let before = fixture.source.store.get(&fixture.source.key(&task.parent).unwrap()).await.unwrap().unwrap();
        assert_eq!(task.consumer_member(), Some(captured.as_str()));
        assert_eq!(fixture.source.store.get(&before.key).await.unwrap().unwrap(), before,
            "consumer guidance does not issue a grant or publish an input");
        let mut config = reviewed_config(fixture.source.scope.realm.as_str());
        config.connector = None;
        config.access = Some(meerkat_core::McpServerAccess::allowing(["read".to_owned()]));
        let b = approve_b_configuration(&fixture,snapshot,[id("read")].into(),Some(config.clone())).await;
        let source = &fixture.source;
        let stored = source.store.get(&source.key(&b).unwrap()).await.unwrap().unwrap();
        let record = source.decode_record(&stored,&b).unwrap();
        let approved = record.approved_task.as_ref().unwrap();
        let route = source.route(&b.route).unwrap();
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: route.source_authority.clone(), work: b.original.clone(),
        };
        let lineage = source.service_grant_lineage(approved.singleton_grant().unwrap(),&original).await.unwrap();
        let resolved = source.owner.grants().resolve_lineage(&lineage,&route.logical_executor,
            route.represented_subject.as_ref()).unwrap();
        let operation = crate::installed_host::application_operations::mcp_connector_operation(
            &route.grant_caller,&route.logical_executor,&route.grant_audience,&config.name,
            &meerkat_core::mcp_config::McpConnectorApproval::NoConnector);
        assert_eq!(meerkat_authorization::mcp_installation::lineage_connector_bound(
            resolved.lineage_restrictions(),resolved.restrictions(),&operation.action,&operation.resource_domain),
            meerkat_authorization::mcp_installation::McpConnectorBound::Approved);
        resolved.check_current().unwrap();
        assert_eq!(approved.original_task.origin_session_id,fixture.session);
        assert_eq!(fixture.inputs().await,Ok(1),"the protected decision does not submit or replay the original");
        source.record_task_group_cancelled(&approved.parent).await.unwrap();
        assert!(!source.service_task_submission_current(&record).await.unwrap());
        assert_eq!(fixture.inputs().await,Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

fn persistence_review_request<S>(fixture: &ContinuationFixture<S>) -> ApprovalRequest {
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("installed-service-fixture").unwrap(),
        owner: ApprovalOwnerRef::Session { session_id: fixture.session.clone() },
        resource: ApprovalResourceRef { kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-credential-service") },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Review credential persistence and access for this task".into(),
            body: Some(serde_json::json!({
                "authentication": {"kind": "declared_secret_slots"},
                "notice": "This review requests defining-realm configuration and credential persistence, followed by access for this task. Partial effects can remain after failure."
            })),
        },
        risk: ApprovalRisk::High, request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: None, metadata: Default::default(), request_provenance: None,
    }
}

fn persistence_config() -> meerkat_core::McpServerConfig {
    let mut config = meerkat_core::McpServerConfig::stdio(
        "mail", "not-executed", vec![], Default::default());
    let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &mut config.transport else {
        unreachable!("stdio constructor");
    };
    launch.secret_env = ["MAIL_TOKEN".to_owned()].into();
    config.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
    config
}

async fn persistence_task<S>(fixture: &ContinuationFixture<S>) -> InstalledServiceTask {
    let original = fixture.submit().await;
    fixture.finish(&original).await.unwrap();
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    let snapshot = acquisition(fixture, &dispatch).await;
    let member = snapshot.request().host().continuation.native_original.as_ref().unwrap()
        .member.as_ref().unwrap().member.clone();
    fixture.source.prepare_service_task(fixture.store.clone(), snapshot,
        "combined-persistence-review", &member).await.unwrap().unwrap()
}

async fn persistence_effect_snapshot(fixture: &ContinuationFixture)
    -> (Vec<StoredRecord>, Vec<u8>)
{
    // The installed fixture's operator decisions, service decisions and source
    // records share this scope. The adjacent file is its actual native grant
    // owner's append journal, not a substituted grant store or lookup result.
    let records = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
    assert!(records.len() < 256, "snapshot must cover the complete fixture scope");
    let grants = std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap();
    (records, grants)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_task_reads_share_current_selection_while_setup_entry_is_held() {
    use meerkat_toolkit::host_lifetime::control::{
        HostCloseReason, HostLifetimeError, PrivateHostLifetimeControl,
    };
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let dispatch = fixture.probe.as_ref().unwrap().captured();
        tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(
            &fixture.session, dispatch.retained_work().unwrap().run_id(),
        )).await.unwrap();
        let (_, snapshot) = task.current().await.unwrap();
        let b = approve_b(&fixture, snapshot).await;
        let stored = fixture.store.get(&fixture.source.key(&b).unwrap()).await.unwrap().unwrap();
        let record = fixture.source.decode_record(&stored, &b).unwrap();
        let before = persistence_effect_snapshot(&fixture).await;
        let model_requests = fixture.model_requests();
        let (entered, observed) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::sync_channel(1);
        let owner = fixture.lifetime().clone();
        let worker = std::thread::spawn(move || owner.with_current_setup(|| {
            let _ = entered.send(());
            released.recv_timeout(WAIT * 2).map_err(|_| HostLifetimeError::Unavailable)?;
            Ok(())
        }));
        // The signal comes only from the actual entered callback. No lock
        // probe can win or disturb the synchronization being exercised.
        let entered = tokio::time::timeout(WAIT, observed).await;
        let reads = if matches!(entered, Ok(Ok(()))) {
            Some(AssertUnwindSafe(tokio::time::timeout(WAIT, async {
                (
                    task.current().await.map(|_| ()),
                    fixture.source.service_task_submission_current(&record).await,
                    fixture.source.service_input_host_current(),
                )
            })).catch_unwind().await)
        } else { None };
        // Release and join even when the real read panics or times out.
        let _ = release.send(());
        let joined = worker.join();
        assert!(matches!(entered, Ok(Ok(()))), "real setup entry was not observed");
        assert!(matches!(joined, Ok(Ok(()))), "held setup entry did not settle");
        let reads = match reads.unwrap() {
            Ok(result) => result.expect("current-selection reads must not await setup entry"),
            Err(payload) => std::panic::resume_unwind(payload),
        };
        assert_eq!(reads, (Ok(()), Ok(true), Ok(())));
        assert_eq!(persistence_effect_snapshot(&fixture).await, before);
        assert_eq!(fixture.inputs().await, Ok(1));
        assert_eq!(fixture.model_requests(), model_requests);

        let mut wrong_task = task.clone();
        wrong_task.input_digest = "0".repeat(64);
        assert!(matches!(wrong_task.current().await, Err(PrivateOriginalError::Denied)));
        fixture.lifetime().begin_close_activation(HostCloseReason::OperatorDisable).unwrap();
        assert!(matches!(task.current().await, Err(PrivateOriginalError::Denied)));
        assert_eq!(fixture.source.service_input_host_current(), Err(PrivateOriginalError::Denied));
        assert_eq!(fixture.source.service_task_submission_current(&record).await,
            Err(PrivateOriginalError::Denied));
        assert_eq!(persistence_effect_snapshot(&fixture).await, before);
        assert_eq!(fixture.inputs().await, Ok(1));
        assert_eq!(fixture.model_requests(), model_requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn combined_persistence_review_binds_scope_before_grants_and_retains_exact_approval() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let fresh = task.fresh.clone();
        let config = persistence_config();
        let defining_realm = RealmId::parse("defining-installation").unwrap();
        assert_ne!(defining_realm.as_str(), fixture.scope.realm.as_str());
        let before = persistence_effect_snapshot(&fixture).await;
        let model_requests = fixture.model_requests();
        let approvals = ApprovalService::new();
        for (field, changed) in [
            ("defining_realm", serde_json::json!("different-installation")),
            ("expected_configuration_generation", serde_json::json!(42)),
            ("configuration", serde_json::json!({"name": "different-server"})),
            ("credential_slots", serde_json::json!(["DIFFERENT_SLOT"])),
        ] {
            let review = task.clone().prepare_no_connector_persistence_review(
                &config, [id("read")].into(), None, persistence_review_request(&fixture), None,
                ServiceMcpPersistenceProposal::new(defining_realm.clone(), 41),
            ).await.unwrap();
            let pending = approvals.request(review.request().clone()).unwrap();
            let mut approved = approvals.decide(&pending.approval_id, ApprovalDecision::Approve,
                ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(), None, None).unwrap();
            let scope = approved.proposed_action.body.as_mut().unwrap()
                .get_mut("access_request").unwrap().get_mut("service_instruction").unwrap()
                .get_mut("configuration_persistence").unwrap();
            *scope.get_mut(field).unwrap() = changed;
            assert_eq!(review.approve(approved, || true).await, Err(PrivateOriginalError::Denied),
                "altered {field} must not become a grant or a fresh source");
            assert_eq!(persistence_effect_snapshot(&fixture).await, before);
        }

        let denied = task.clone().prepare_no_connector_persistence_review(
            &config, [id("read")].into(), None, persistence_review_request(&fixture), None,
            ServiceMcpPersistenceProposal::new(defining_realm.clone(), 41),
        ).await.unwrap();
        let pending = approvals.request(denied.request().clone()).unwrap();
        let record = approvals.decide(&pending.approval_id, ApprovalDecision::Deny,
            ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(), None, None).unwrap();
        assert_eq!(denied.approve(record, || true).await, Ok(false));
        assert_eq!(persistence_effect_snapshot(&fixture).await, before);

        let review = task.prepare_no_connector_persistence_review(
            &config, [id("read")].into(), None, persistence_review_request(&fixture), None,
            ServiceMcpPersistenceProposal::new(defining_realm.clone(), 41),
        ).await.unwrap();
        let body = review.request().proposed_action.body.as_ref().unwrap();
        let scope = &body["access_request"]["service_instruction"]["configuration_persistence"];
        assert_eq!(scope["defining_realm"], defining_realm.as_str());
        assert_eq!(scope["expected_configuration_generation"], 41);
        assert_eq!(scope["configuration"], serde_json::to_value(&config).unwrap());
        assert_eq!(scope["credential_slots"], serde_json::json!(["MAIL_TOKEN"]));
        assert_eq!(body["access_request"]["authentication"]["kind"], "declared_secret_slots");
        assert_eq!(body["access_request"]["notice"],
            "This review requests defining-realm configuration and credential persistence, followed by access for this task. Partial effects can remain after failure.");
        assert_eq!(persistence_effect_snapshot(&fixture).await, before,
            "preparing a combined proposal issues and publishes nothing");
        let pending = approvals.request(review.request().clone()).unwrap();
        let approved = approvals.decide(&pending.approval_id, ApprovalDecision::Approve,
            ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(), None, None).unwrap();
        // Component boundary: this invokes the real grant/source path only.
        // There is no native installation handle, secret value or attachment
        // call here. The future Console caller must first settle those effects.
        assert_eq!(review.approve(approved, || true).await, Ok(true));
        let (rows, grants) = persistence_effect_snapshot(&fixture).await;
        assert!(grants != before.1, "the exact approval must issue actual native grants");
        let records: Vec<_> = rows.iter().filter(|row| row.data.schema == SOURCE_SCHEMA)
            .map(|row| fixture.source.decode_stored(row).unwrap())
            .filter(|record| record.event.original == fresh).collect();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert!(record.final_association.is_none());
        assert!(record.service_admission.is_none());
        let approved = record.approved_task.as_ref().unwrap();
        let route = fixture.source.route(&record.event.route).unwrap();
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: route.source_authority.clone(), work: fresh,
        };
        let lineage = fixture.source.service_grant_lineage(approved.singleton_grant().unwrap(), &original).await.unwrap();
        let resolved = fixture.source.owner.grants().resolve_lineage(&lineage,
            &route.logical_executor, route.represented_subject.as_ref()).unwrap();
        let operation = crate::installed_host::application_operations::mcp_connector_operation(
            &route.grant_caller, &route.logical_executor, &route.grant_audience, &config.name,
            &meerkat_core::mcp_config::McpConnectorApproval::NoConnector);
        assert_eq!(meerkat_authorization::mcp_installation::lineage_connector_bound(
            resolved.lineage_restrictions(), resolved.restrictions(), &operation.action,
            &operation.resource_domain), meerkat_authorization::mcp_installation::McpConnectorBound::Approved);
        resolved.check_current().unwrap();
        assert_eq!(approved.original_task.origin_session_id, fixture.session);
        assert_eq!(fixture.inputs().await, Ok(1));
        assert_eq!(fixture.model_requests(), model_requests);
        fixture.source.record_task_group_cancelled(&approved.parent).await.unwrap();
        assert!(!fixture.source.service_task_submission_current(record).await.unwrap());
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn combined_persistence_review_refuses_unchecked_definition_shapes() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let before = persistence_effect_snapshot(&fixture).await;
        let model_requests = fixture.model_requests();
        let mut connector = persistence_config();
        connector.connector = reviewed_config(&fixture.scope.realm).connector;
        let http = meerkat_core::McpServerConfig::streamable_http(
            "mail", "https://not-contacted.invalid/mcp", Default::default());
        let mut missing_slots = persistence_config();
        let mut literal_env = persistence_config();
        let mut invalid_slot = persistence_config();
        for (config, case) in [(&mut missing_slots, 0), (&mut literal_env, 1), (&mut invalid_slot, 2)] {
            let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &mut config.transport else {
                unreachable!("stdio fixture");
            };
            match case {
                0 => launch.secret_env.clear(),
                1 => { launch.env.insert("INLINE_VALUE".into(), "refused-literal-fixture".into()); },
                _ => { launch.secret_env = ["INVALID-NAME".to_owned()].into(); },
            }
        }
        for (case, config) in [("connector", connector), ("HTTP", http),
            ("no slots", missing_slots), ("literal environment", literal_env), ("invalid slot", invalid_slot)]
        {
            let result = task.clone().prepare_no_connector_persistence_review(
                &config, [id("read")].into(), None, persistence_review_request(&fixture), None,
                ServiceMcpPersistenceProposal::new(RealmId::parse("defining-installation").unwrap(), 41),
            ).await;
            assert!(matches!(result, Err(PrivateOriginalError::Denied)), "{case} must refuse before review");
            assert_eq!(persistence_effect_snapshot(&fixture).await, before);
        }
        assert_eq!(fixture.inputs().await, Ok(1));
        assert_eq!(fixture.model_requests(), model_requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}
