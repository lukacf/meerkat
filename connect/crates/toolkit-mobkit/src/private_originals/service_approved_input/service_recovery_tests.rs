//! Actual canonical SQLite and native file-grant ownership controls. The cold
//! case drops and restores installed/native owners; no provider is contacted.
//! The first four cases exercise canonical owners without Console auth. The
//! fifth verifies a real Meta Console carrier and protected binding, but no
//! deployed HTTP endpoint or interactive login flow is exercised.
use super::*;
use crate::installed_host::operator_setup::ServiceMcpParentOutcome;
use crate::private_originals::service_recovery::ServiceRecoveryRead;
use meerkat_authorization_contracts::grant::GrantLineageRef;

async fn reviewed<S>(
    fixture: &ContinuationFixture<S>,
    task: InstalledServiceTask,
) -> PreparedServiceInstruction {
    let mut config = persistence_config();
    let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &mut config.transport else {
        unreachable!()
    };
    launch.secret_env.clear();
    task.prepare_no_connector_review(
        &config,
        [id("read")].into(),
        None,
        persistence_review_request(fixture),
    )
    .await
    .unwrap()
}
fn approve_record(review: &PreparedServiceInstruction) -> ApprovalRecord {
    let approvals = ApprovalService::new();
    let pending = approvals.request(review.request().clone()).unwrap();
    approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(),
            None,
            None,
        )
        .unwrap()
}
async fn issue_before_publication<S>(
    fixture: &ContinuationFixture<S>,
    task: InstalledServiceTask,
) -> (
    PrivateServiceGrantRef,
    GrantLineageRef,
    PrivateSourceEvidence,
) {
    let review = reviewed(fixture, task).await;
    let record = approve_record(&review);
    let evidence = approval_evidence(&record).unwrap();
    let grant = PrivateOriginals::service_mcp_reference(
        review.review.decision(),
        review.review.selection(),
    )
    .unwrap();
    let observation = review
        .operator
        .service_mcp_parent_compensation(&review.review, &record)
        .unwrap()
        .unwrap();
    assert!(review.tool_policy.is_none());
    // Exercise the actual two native owners, stopping at the precise crash
    // boundary before retain_approved. No source or grant row is fabricated.
    let ServiceMcpParentOutcome::Approved(parent) = review
        .operator
        .apply_service_mcp_parent_decision(review.review, record)
        .await
        .unwrap()
    else {
        panic!("parent issued")
    };
    assert_eq!(
        fixture
            .source
            .apply_mcp_service_access_decision_current(parent, Arc::new(|| true))
            .await
            .unwrap()
            .state,
        PrivateServiceGrantState::GrantIssued
    );
    (
        grant,
        observation.reference_observation().await.unwrap().unwrap(),
        evidence,
    )
}
fn journal<S>(fixture: &ContinuationFixture<S>) -> Vec<serde_json::Value> {
    let bytes = std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap();
    std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter_map(|line| line.get("change").cloned())
        .collect()
}
fn revoked(changes: &[serde_json::Value]) -> Vec<String> {
    changes
        .iter()
        .filter_map(|change| {
            change["kind"]["revoked"]["grant_id"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}
fn issued(changes: &[serde_json::Value]) -> Vec<String> {
    changes
        .iter()
        .filter_map(|change| {
            change["kind"]["issued"]["record"]["id"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}
async fn inventory<S>(fixture: &ContinuationFixture<S>) -> ServiceRecoveryRead {
    let source = &fixture.source;
    let human = source.routes().unwrap()[0].requester.clone();
    let member = fixture
        .lifetime()
        .selected_policy()
        .native_host
        .as_ref()
        .unwrap()
        .members[0]
        .member
        .clone();
    let page = source.source_status_page_with_recovery(None).await.unwrap();
    assert!(
        page.next_after.is_none(),
        "small real fixture must fit in a bounded page"
    );
    source
        .read_service_recovery_page(&human, &[member], &page.rows)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_recovery_restores_file_owner_and_cleans_unpublished_intent_without_admission() {
    let (first, history, retained, continuation) =
        ContinuationFixture::for_durable_continuation().await;
    let created = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&first).await;
        let dispatch = first.probe.as_ref().unwrap().captured();
        tokio::time::timeout(
            WAIT,
            first
                .machine
                .wait_run_settled(&first.session, dispatch.retained_work().unwrap().run_id()),
        )
        .await
        .unwrap();
        let (grant, parent, _) = issue_before_publication(&first, task.clone()).await;
        let (_, child) = first
            .source
            .service_compensation_child(&grant, &parent)
            .await
            .unwrap()
            .unwrap();
        let read = inventory(&first).await;
        assert_eq!(read.rows.len(), 1);
        assert!(read.rows[0].can_recover);
        assert_eq!(read.rows[0].projection["state"], "publication_unresolved");
        (
            parent,
            child,
            task.parent.clone(),
            task.parent_association.clone(),
        )
    }))
    .catch_unwind()
    .await;
    let closed = first.close_durable_owners().await;
    drop(continuation);
    drop(history);
    drop(first);
    assert!(closed.is_ok(), "all pre-restore owners join: {closed:?}");
    let (parent, child, prior, association) = match created {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    let (fixture, history, continuation) =
        ContinuationFixture::reopen_durable_continuation(retained).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let source = &fixture.source;
        assert_eq!(
            source
                .owner
                .grants()
                .issued_reference(&parent.grant_id)
                .unwrap(),
            Some(parent.clone()),
            "same persisted authority incarnation"
        );
        assert_eq!(
            source
                .owner
                .grants()
                .issued_reference(&child.grant_id)
                .unwrap(),
            Some(child.clone())
        );
        assert!(
            source
                .owner
                .historical_task_custody_for_test(&association)
                .is_none(),
            "recovery does not restore Original admission"
        );
        let before = fixture
            .store
            .list_records(&fixture.scope, None, 256)
            .await
            .unwrap();
        let native_before = journal(&fixture);
        let inputs = fixture.inputs().await.unwrap();
        let requests = fixture.model_requests();
        let read = inventory(&fixture).await;
        assert_eq!(read.rows.len(), 1);
        let row = &read.rows[0];
        assert!(row.can_recover);
        assert_eq!(row.projection["parent"], "issued");
        assert_eq!(row.projection["child"], "issued");
        assert_eq!(row.projection["policy"], "not_requested");
        assert!(read.check_visible(|_| true).is_ok());
        assert!(read.check_visible(|_| false).is_err());
        let page = source.source_status_page_with_recovery(None).await.unwrap();
        assert!(
            source
                .read_service_recovery_page(&read.human, &[], &page.rows)
                .await
                .unwrap()
                .rows
                .is_empty()
        );
        let other = PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::Human,
            "someone-else",
            meerkat_core::TrustDomainId::new("other-console").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            source
                .read_service_recovery_page(&other, &[row.member.clone()], &page.rows)
                .await,
            Err(PrivateOriginalError::Denied)
        ));
        let held_writer = source.owner.try_source_writer().unwrap();
        assert!(source.prepare_service_recovery(&read, row).await.is_err());
        drop(held_writer);
        let prepared = source.prepare_service_recovery(&read, row).await.unwrap();
        let held_custody = fixture.machine.try_controller_grant_mutation().unwrap();
        assert!(matches!(
            prepared.revoke(),
            Err(PrivateOriginalError::Unavailable)
        ));
        drop(held_custody);
        assert_eq!(
            journal(&fixture),
            native_before,
            "busy custody performs no native mutation"
        );
        // Establish the exact owner state left by a prior attempt that revoked
        // the parent but did not finish the child. This is a real native revoke,
        // not an injected failure or a fabricated journal entry.
        let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
        source
            .owner
            .grants()
            .revoke(&read.human, &parent, &mut custody)
            .unwrap();
        drop(custody);
        assert_eq!(
            revoked(&journal(&fixture)[native_before.len()..]),
            vec![parent.grant_id.as_str().to_owned()]
        );
        assert!(
            source.prepare_service_recovery(&read, row).await.is_err(),
            "old offer stamp does not authorize retry"
        );
        let retry = inventory(&fixture).await;
        assert!(
            retry.rows[0].can_recover,
            "retained revoked identity is not absent-parent nonentry"
        );
        let outcome = source
            .prepare_service_recovery(&retry, &retry.rows[0])
            .await
            .unwrap()
            .revoke()
            .unwrap();
        assert_eq!(outcome.parent, "revoked");
        assert_eq!(outcome.child, "revoked");
        assert!(!outcome.uncertain);
        let after = journal(&fixture);
        let changes = &after[native_before.len()..];
        assert!(issued(changes).is_empty());
        assert_eq!(
            revoked(changes),
            vec![parent.grant_id.as_str().to_owned(), child.grant_id.as_str().to_owned()]
        );
        assert_eq!(
            fixture
                .store
                .list_records(&fixture.scope, None, 256)
                .await
                .unwrap(),
            before,
            "recovery adds no second ledger and edits no source/policy"
        );
        assert!(
            source
                .owner
                .historical_task_custody_for_test(&association)
                .is_none()
        );
        assert_eq!(fixture.inputs().await.unwrap(), inputs);
        assert_eq!(fixture.model_requests(), requests);
        let original = fixture
            .store
            .get(&source.key(&prior).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert!(
            source
                .decode_record(&original, &prior)
                .unwrap()
                .final_association
                .as_ref()
                == Some(&association)
        );
    }))
    .catch_unwind()
    .await;
    let closed = fixture.close_durable_owners().await;
    drop(continuation);
    drop(history);
    drop(fixture);
    assert!(closed.is_ok(), "restored owners join: {closed:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_recovery_preserves_new_publication_and_other_approval_child() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let (grant, parent, evidence) = issue_before_publication(&fixture, task.clone()).await;
        let first = inventory(&fixture).await;
        assert_eq!(first.rows.len(), 1);
        assert!(first.rows[0].can_recover);
        let second = reviewed(&fixture, task.clone()).await;
        let record = approve_record(&second);
        let cleanup = second
            .operator
            .service_mcp_parent_compensation(&second.review, &record)
            .unwrap()
            .unwrap();
        let ServiceMcpParentOutcome::Approved(approved) = second
            .operator
            .apply_service_mcp_parent_decision(second.review, record)
            .await
            .unwrap()
        else {
            panic!("actual second parent")
        };
        assert!(
            fixture
                .source
                .apply_mcp_service_access_decision_current(approved, Arc::new(|| true))
                .await
                .is_err(),
            "deterministic child belongs to first parent"
        );
        let other_parent = cleanup.reference_observation().await.unwrap().unwrap();
        let read = inventory(&fixture).await;
        let second_row = read
            .rows
            .iter()
            .find(|row| row.parent.as_ref() == Some(&other_parent))
            .unwrap();
        assert_eq!(second_row.projection["child"], "another_approval");
        let before = journal(&fixture);
        let outcome = fixture
            .source
            .prepare_service_recovery(&read, second_row)
            .await
            .unwrap()
            .revoke()
            .unwrap();
        assert_eq!(outcome.parent, "revoked");
        assert_eq!(outcome.child, "another_approval_preserved");
        assert_eq!(
            revoked(&journal(&fixture)[before.len()..]),
            vec![other_parent.grant_id.as_str().to_owned()]
        );
        fixture
            .source
            .require_service_grant_issued(&grant)
            .await
            .unwrap();
        let ready = inventory(&fixture).await;
        let first_row = ready
            .rows
            .iter()
            .find(|row| row.parent.as_ref() == Some(&parent))
            .unwrap();
        task.clone()
            .retain_approved(grant.clone(), evidence, &|| Ok(()))
            .await
            .unwrap();
        let published_before = journal(&fixture);
        assert!(
            fixture
                .source
                .prepare_service_recovery(&ready, first_row)
                .await
                .is_err(),
            "publication appearing after offer invalidates cleanup before native entry"
        );
        let published = inventory(&fixture).await;
        assert!(
            published
                .rows
                .iter()
                .all(|row| !row.can_recover && row.projection["state"] == "publication_present")
        );
        assert!(
            fixture
                .source
                .prepare_service_recovery(&published, &published.rows[0])
                .await
                .is_err()
        );
        assert_eq!(journal(&fixture), published_before);
        fixture
            .source
            .require_service_grant_issued(&grant)
            .await
            .unwrap();
        assert_eq!(fixture.inputs().await, Ok(1));
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_inventory_uses_the_selected_shared_page_without_changing_legacy_cursor() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        issue_before_publication(&fixture, task).await;
        let source = &fixture.source;
        let records = fixture
            .store
            .list_records(&fixture.scope, None, 256)
            .await
            .unwrap();
        let expected = records
            .iter()
            .filter(|row| row.data.schema == SOURCE_SCHEMA)
            .cloned()
            .collect::<Vec<_>>();
        let legacy = source.source_status_page(None).await.unwrap();
        assert_eq!(legacy.rows, expected);
        assert!(legacy.next_after.is_none());
        let selected = source.source_status_page_with_recovery(None).await.unwrap();
        assert_eq!(selected.rows.len(), legacy.rows.len() + 1);
        assert_eq!(
            selected
                .rows
                .iter()
                .filter(|row| {
                    crate::installed_host::operator_setup::service_recovery::is_parent_record(row)
                })
                .count(),
            1
        );
        // Padding is only mechanical history, never grant/approval evidence.
        for index in 0..1024 {
            let key = format!("!recovery-padding-{index:04}");
            fixture
                .store
                .admit(toolkit_store::AdmissionRequest {
                    key: StoreKey {
                        realm: fixture.scope.realm.clone(),
                        owner: fixture.scope.owner.clone(),
                        id: key.clone(),
                    },
                    admission_key: key,
                    data: toolkit_store::RecordData {
                        schema: "fixture.history".into(),
                        version: 1,
                        body: serde_json::json!({}),
                    },
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
        }
        let legacy = source.source_status_page(None).await.unwrap();
        let selected = source.source_status_page_with_recovery(None).await.unwrap();
        assert!(legacy.rows.is_empty() && selected.rows.is_empty());
        assert_eq!(legacy.next_after, selected.next_after);
        assert_eq!(legacy.next_after.as_deref(), Some("!recovery-padding-1023"));
        let tail = source
            .source_status_page(legacy.next_after.as_deref())
            .await
            .unwrap();
        assert_eq!(tail.rows, expected);
        assert_eq!(fixture.inputs().await, Ok(1));
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_recovery_keeps_absent_parent_unresolved_and_allows_cancelled_source_cleanup() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let absent = reviewed(&fixture, task.clone()).await;
        let record = approve_record(&absent);
        let before = journal(&fixture);
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(
            "CREATE TRIGGER lose_native_refusal BEFORE UPDATE OF data ON toolkit_records
            WHEN json_extract(NEW.data, '$.schema') = 'toolkit.operator-application-parent'
            AND json_extract(NEW.data, '$.body.issued.state') = 'native_refused'
            BEGIN SELECT RAISE(ABORT, 'fixture lost refusal acknowledgement'); END;",
        )
        .unwrap();
        assert!(matches!(
            absent
                .operator
                .apply_service_mcp_parent_decision_current(
                    absent.review,
                    record,
                    Arc::new(|| false)
                )
                .await
                .unwrap(),
            ServiceMcpParentOutcome::PendingReconciliation
        ));
        sql.execute_batch("DROP TRIGGER lose_native_refusal;")
            .unwrap();
        assert_eq!(journal(&fixture), before);
        let read = inventory(&fixture).await;
        assert_eq!(read.rows.len(), 1);
        let row = &read.rows[0];
        assert_eq!(row.projection["parent"], "not_observed");
        assert!(!row.can_recover);
        assert!(
            fixture
                .source
                .prepare_service_recovery(&read, row)
                .await
                .is_err()
        );
        assert_eq!(
            journal(&fixture),
            before,
            "absence cannot be advertised as cancellation or completed cleanup"
        );
        let (_grant, parent, _) = issue_before_publication(&fixture, task.clone()).await;
        fixture
            .source
            .record_task_group_cancelled(&task.parent)
            .await
            .unwrap();
        assert_eq!(
            task.check_setup_current().await,
            Err(PrivateOriginalError::Denied)
        );
        let read = inventory(&fixture).await;
        let row = read
            .rows
            .iter()
            .find(|row| row.parent.as_ref() == Some(&parent))
            .unwrap();
        let outcome = fixture
            .source
            .prepare_service_recovery(&read, row)
            .await
            .unwrap()
            .revoke()
            .unwrap();
        assert_eq!(outcome.parent, "revoked");
        assert_eq!(outcome.child, "revoked");
        assert_eq!(fixture.inputs().await, Ok(1));
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

// Admit the test's first Original through the actual installed Console binder.
// A non-Console submission cannot supply login custody for a qualified Human.
pub(super) async fn verified_console_task<S>(
    fixture: &ContinuationFixture<S>,
    human: &meerkat_mobkit::runtime::AuthenticatedConsoleHuman,
    logins: crate::private_host::ConsoleLogins,
    access: meerkat_mobkit::access::AccessController,
    member: &str,
) -> InstalledServiceTask {
    use meerkat_mobkit::identity_first::{TurnOutcome, TurnOutput, TurnTracking};
    use meerkat_runtime::{Input, PromptInput, identifiers::IdempotencyKey};
    let login = logins
        .resolve(
            human.issuer(),
            human.subject(),
            human.login_scope().unwrap(),
        )
        .unwrap();
    let ingress = fixture
        .source
        .console_ingress(logins, Some(access))
        .await
        .unwrap();
    let runtime = fixture.source.bound_identity_runtime().unwrap();
    let identity = runtime
        .owned_identity_for_member_alias(member)
        .await
        .unwrap();
    let binder = ingress.binder_for(&identity, human).unwrap();
    let mut prompt = PromptInput::new("Answer this retained Console input", None);
    prompt.header.idempotency_key = Some(IdempotencyKey::new(
        "console:00000000-0000-4000-8000-000000000011:00000000-0000-4000-8000-000000000012",
    ));
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let admitted = tokio::time::timeout(
        WAIT,
        runtime.send_governed_with_turn_ticket(&identity, Some(member), input, binder),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(admitted.admission.session_id, fixture.session);
    assert_eq!(admitted.admission.input_id, input_id);
    assert!(!admitted.admission.deduplicated);
    assert!(admitted.settlement.is_ok());
    let TurnTracking::Tracked(ticket) = admitted.ticketed.turn else {
        panic!("native ticket missing")
    };
    let completed = tokio::time::timeout(WAIT, runtime.wait_for_turn(&identity, ticket, WAIT))
        .await
        .unwrap();
    assert!(matches!(
        completed,
        TurnOutcome::Completed {
            output: TurnOutput::Text { text, .. }
        } if text == "The original request completed"
    ));
    // ProbeProvider transforms the first authorized response into the captured
    // ContextProbe call; the second authorized response completes this turn.
    assert_eq!(fixture.model_requests(), 2);
    // Read the existing owner's retained identity for this exact native input.
    // No record, grant, authenticated Human or login association is fabricated.
    let original = {
        let retained = fixture.source.retained.lock().unwrap();
        let rows: Vec<_> = retained
            .iter()
            .filter(|held| held.reference.original.as_str() == input_id.to_string())
            .collect();
        assert_eq!(rows.len(), 1);
        rows[0].reference.clone()
    };
    let association = fixture
        .source
        .binding_evidence(&original)
        .await
        .unwrap()
        .unwrap();
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    let snapshot = acquisition_for_association(fixture, &dispatch, association).await;
    let task = fixture
        .source
        .prepare_service_task(
            fixture.store.clone(),
            snapshot,
            "combined-persistence-review",
            member,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        task.parent == original,
        "setup must retain this exact Console source"
    );
    assert!(task.console_login().unwrap().same_owner(&login));
    task
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operator_recovery_requires_actual_verified_human_admin_member_and_same_login_at_entry() {
    use crate::host_gateway::{ConsoleAccessPolicy, ConsoleRecoveryPolicy, ConsoleSetupBinding};
    use crate::private_originals::console_ingress::auth_fixture::*;
    use axum::http::{HeaderMap, Uri, header};
    use meerkat_mobkit::access::{AccessControlConfig, AccessController, AccessRule};
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    let mut fixture = ContinuationFixture::for_console_operator_input(selected_human()).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let selected = fixture.lifetime().selected_policy().native_host.as_ref().unwrap();
        assert_eq!(selected.remote_operator.as_ref(), Some(&selected_human()));
        assert_eq!(fixture.source.route(&id("console")).unwrap().requester, selected_human());
        let member = selected.members[0].member.member.clone();
        let config = |admin: bool, visible: bool| AccessControlConfig { enabled:true,
            admins: vec!["separate-configuration-owner".into()], rules: [
                admin.then(|| AccessRule { id:"explicit-admin".into(), subjects:vec![EMAIL.into()],
                    actions:vec!["access.admin".into()], ..Default::default() }),
                visible.then(|| AccessRule { id:"selected-member".into(), subjects:vec![EMAIL.into()],
                    actions:vec!["agent.view".into(), "agent.send".into()], agents:vec![member.clone()], ..Default::default() }),
            ].into_iter().flatten().collect(), ..Default::default() };
        let controller = AccessController::new(config(true, true)).unwrap();
        controller.join_authorization_publication(fixture.source.authorization_publication()).unwrap();
        let decisions = meerkat_mobkit::console_auth_config::parse_console_auth_config(&serde_json::json!({
            "provider":"jwt", "shared_secret":std::str::from_utf8(KEY).unwrap(), "issuer":ISSUER,
            "audience":AUDIENCE, "email_allowlist":[EMAIL],
        })).unwrap();
        let runtime = fixture.native_for_console_mut(); runtime.set_access_controller(controller.clone());
        let resolver = runtime.console_viewer_resolver(&decisions);
        let expires = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 600;
        let (pipe, logins) = crate::private_host::ConsoleLogins::fixture_owner();
        logins.fixture_enroll(41, crate::private_host::ConsoleLoginSelection { issuer:ISSUER.into(), subject:SUBJECT.into(),
            login_scope_id:SCOPE.into(), expires_at_ms:expires * 1000 }).unwrap();
        logins.fixture_enroll(42, crate::private_host::ConsoleLoginSelection { issuer:ISSUER.into(), subject:SUBJECT.into(),
            login_scope_id:"another-login".into(), expires_at_ms:expires * 1000 }).unwrap();
        let mut headers = HeaderMap::new(); headers.insert(header::AUTHORIZATION, signed_fixture(ISSUER, SCOPE, expires).parse().unwrap());
        let mut other_headers = HeaderMap::new(); other_headers.insert(header::AUTHORIZATION, signed_fixture(ISSUER, "another-login", expires).parse().unwrap());
        let uri = Uri::from_static("/host/toolkit/setup/recovery/revoke");
        let current = || resolver.resolve(&headers, &uri);
        let viewer = current().unwrap(); let human = viewer.human.as_ref().unwrap();
        assert_eq!(human.principal().unwrap(), selected_human()); assert_eq!(human.current().unwrap().as_str(), EMAIL);
        assert_eq!(viewer.principal.as_ref(), Some(&human.current().unwrap()));
        assert!(viewer.access_view.as_ref().unwrap().can_administer());
        let task = verified_console_task(&fixture, human, logins.clone(), controller.clone(), &member).await;
        assert_eq!(fixture.inputs().await, Ok(1));
        let (_grant, parent, _) = issue_before_publication(&fixture, task).await;
        let binding = ConsoleSetupBinding::fixture(fixture.lifetime().clone(), fixture.source.clone(), logins.clone(),
            crate::private_host::PrivateHostClosed(pipe.subscribe()));
        let requester_only = binding.clone().with_access_policy(Some(ConsoleAccessPolicy::RequesterIssuedLeaves));
        let old_page = requester_only.status_page(&current, None).await.unwrap();
        assert!(old_page.recovery().is_none() && old_page.recovery_projection().is_none());
        let binding = binding.with_recovery_policy(Some(ConsoleRecoveryPolicy::OperatorUnpublishedServiceApprovals));
        let page = binding.status_page(&current, None).await.unwrap();
        let offer = page.recovery().unwrap(); let row_id = offer.projection()["rows"][0]["id"].as_str().unwrap().to_owned();
        assert!(old_page.access().is_some());
        // Both actual Console visibility wrappers retain Unavailable when an
        // owner publication invalidates the read. Refresh rebuilds from owners.
        controller.replace_config(config(true, true)).unwrap();
        assert!(matches!(requester_only.check_status_page(&old_page, &current),
            Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable)));
        assert!(matches!(binding.check_status_page(&page, &current),
            Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable)));
        let refreshed = binding.status_page(&current, None).await.unwrap();
        assert!(binding.check_status_page(&refreshed, &current).is_ok());
        let page = refreshed;
        let offer = page.recovery().unwrap();
        let before = journal(&fixture);
        assert!(requester_only.recover_service_approval(offer, &row_id, &current).await.is_err());
        assert!(binding.recover_service_approval(offer, &row_id, &|| resolver.resolve(&other_headers, &uri)).await.is_err());
        for (admin, visible) in [(false, true), (true, false)] {
            controller.replace_config(config(admin, visible)).unwrap();
            assert!(binding.recover_service_approval(offer, &row_id, &current).await.is_err());
            assert_eq!(journal(&fixture), before);
            controller.replace_config(config(true, true)).unwrap();
        }
        // Change administration in the actual final synchronous resolver call,
        // after awaited canonical reads and reservation, before native revoke.
        let page = binding.status_page(&current, None).await.unwrap(); let offer = page.recovery().unwrap();
        let row_id = offer.projection()["rows"][0]["id"].as_str().unwrap().to_owned();
        let calls = AtomicUsize::new(0);
        let changes_at_entry = || {
            if calls.fetch_add(1, Ordering::SeqCst) == 2 { controller.replace_config(config(false, true)).unwrap(); }
            resolver.resolve(&headers, &uri)
        };
        assert!(binding.recover_service_approval(offer, &row_id, &changes_at_entry).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3); assert_eq!(journal(&fixture), before);
        controller.replace_config(config(true, true)).unwrap();
        let page = binding.status_page(&current, None).await.unwrap(); let offer = page.recovery().unwrap();
        let row_id = offer.projection()["rows"][0]["id"].as_str().unwrap().to_owned();
        let outcome = binding.recover_service_approval(offer, &row_id, &current).await.unwrap();
        assert_eq!(outcome["parent"], "revoked"); assert_eq!(outcome["child"], "revoked"); assert_eq!(outcome["uncertain"], false);
        assert_eq!(outcome["policy_observed_at_offer"], "not_requested"); assert_eq!(outcome["policy_changed"], false);
        let after = journal(&fixture); assert_eq!(revoked(&after[before.len()..]).first(), Some(&parent.grant_id.as_str().to_owned()));
        assert_eq!(revoked(&after[before.len()..]).len(), 2); assert!(issued(&after[before.len()..]).is_empty());
        assert!(binding.recover_service_approval(offer, &row_id, &current).await.is_err()); assert_eq!(journal(&fixture), after);
        let retired = logins.fixture_retire(41).unwrap().await.unwrap();
        assert!(retired.reservations_drained && !retired.uncertain);
        assert!(binding.status_page(&current, None).await.is_err());
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[path = "local_recovery_tests.rs"]
mod local_recovery_tests;
