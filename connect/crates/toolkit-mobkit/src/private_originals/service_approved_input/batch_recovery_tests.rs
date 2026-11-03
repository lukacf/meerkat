//! Actual Console admission and native batch issuance with canonical failures.
//! No MCP process is started. The held-entry test retires and joins the review;
//! it does not claim that dropping a caller settles an entered native effect.
use super::*;
use crate::private_originals::console_ingress::auth_fixture::*;
use axum::http::{HeaderMap, Uri, header};
use meerkat_mobkit::access::{AccessControlConfig, AccessController, AccessRule};
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore};
use std::sync::{
    Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{oneshot, watch};

type PersistentFixture =
    ContinuationFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;

// The fixture owns only public signing material and the login transport. Meta
// verifies the Human and the real Console binder admits the actual Original.
pub(in crate::private_originals::service_approved_input) async fn console_task(
    fixture: &mut PersistentFixture,
) -> (
    InstalledServiceTask,
    watch::Sender<bool>,
    crate::private_host::ConsoleLogins,
) {
    let selected = fixture
        .lifetime()
        .selected_policy()
        .native_host
        .as_ref()
        .unwrap();
    assert_eq!(selected.remote_operator.as_ref(), Some(&selected_human()));
    let member = selected.members[0].member.member.clone();
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["separate-configuration-owner".into()],
        rules: vec![
            AccessRule {
                id: "explicit-admin".into(),
                subjects: vec![EMAIL.into()],
                actions: vec!["access.admin".into()],
                ..Default::default()
            },
            AccessRule {
                id: "selected-member".into(),
                subjects: vec![EMAIL.into()],
                actions: vec!["agent.view".into(), "agent.send".into()],
                agents: vec![member.clone()],
                ..Default::default()
            },
        ],
        ..Default::default()
    })
    .unwrap();
    access
        .join_authorization_publication(fixture.source.authorization_publication())
        .unwrap();
    let decisions =
        meerkat_mobkit::console_auth_config::parse_console_auth_config(&serde_json::json!({
            "provider": "jwt", "shared_secret": std::str::from_utf8(KEY).unwrap(),
            "issuer": ISSUER, "audience": AUDIENCE, "email_allowlist": [EMAIL],
        }))
        .unwrap();
    fixture
        .native_for_console_mut()
        .set_access_controller(access.clone());
    let resolver = fixture
        .native_for_console_mut()
        .console_viewer_resolver(&decisions);
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let (pipe, logins) = crate::private_host::ConsoleLogins::fixture_owner();
    logins
        .fixture_enroll(
            41,
            crate::private_host::ConsoleLoginSelection {
                issuer: ISSUER.into(),
                subject: SUBJECT.into(),
                login_scope_id: SCOPE.into(),
                expires_at_ms: expires * 1000,
            },
        )
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        signed_fixture(ISSUER, SCOPE, expires).parse().unwrap(),
    );
    let viewer = resolver
        .resolve(
            &headers,
            &Uri::from_static("/host/toolkit/setup/review/decide"),
        )
        .unwrap();
    let human = viewer.human.as_ref().unwrap();
    assert_eq!(human.principal().unwrap(), selected_human());
    assert_eq!(viewer.principal.as_ref(), Some(&human.current().unwrap()));
    assert!(viewer.access_view.as_ref().unwrap().can_administer());
    let task = service_recovery_tests::verified_console_task(
        fixture,
        human,
        logins.clone(),
        access,
        &member,
    )
    .await;
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    let run = dispatch.retained_work().unwrap().run_id();
    tokio::time::timeout(
        WAIT,
        fixture.machine.wait_run_settled(&fixture.session, run),
    )
    .await
    .unwrap();
    assert_eq!(
        fixture
            .machine
            .run_turn_terminal(&fixture.session, run)
            .await,
        Some(TurnTerminalOutcome::Completed)
    );
    (task, pipe, logins)
}

fn configuration(name: &str) -> meerkat_core::McpServerConfig {
    let mut config =
        meerkat_core::McpServerConfig::stdio(name, "not-executed", vec![], Default::default());
    config.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
    config
        .tool_names
        .insert("read".into(), format!("{name}_read"));
    config
}

fn request(fixture: &PersistentFixture) -> ApprovalRequest {
    let mut request = persistence_review_request(fixture);
    request.proposed_action.summary = "Review this exact public service set".into();
    request.proposed_action.body = Some(serde_json::json!({
        "fixture": "native-batch-recovery", "configuration_persistence": "not requested",
    }));
    request
}

async fn batch(
    fixture: &PersistentFixture,
    task: InstalledServiceTask,
) -> PreparedPublicServiceBatch {
    let entries = ["batch_alpha", "batch_beta"]
        .into_iter()
        .map(|name| PublicServiceBatchEntry {
            configuration: configuration(name),
            operations: [id("read")].into(),
            request: request(fixture),
        })
        .collect();
    task.prepare_public_batch(
        entries,
        Some(30_000),
        request(fixture),
        ServiceToolPolicySelection {
            tools: ["batch_alpha_read", "batch_beta_read"]
                .into_iter()
                .map(
                    |name| crate::installed_host::tool_policy::ServiceToolInvocation {
                        name: name.into(),
                        consequence: meerkat_core::CompiledToolConsequence::R0,
                    },
                )
                .collect(),
            catalog: Some(meerkat_core::CompiledToolConsequence::R0),
        },
    )
    .await
    .unwrap()
}

fn approve(request: &ApprovalRequest) -> ApprovalRecord {
    let approvals = ApprovalService::new();
    let pending = approvals.request(request.clone()).unwrap();
    approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("fixture-protected-batch-presenter").unwrap(),
            None,
            None,
        )
        .unwrap()
}

fn journal(fixture: &PersistentFixture) -> Vec<serde_json::Value> {
    let bytes = std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap();
    std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn issued(rows: &[serde_json::Value]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            row["change"]["kind"]["issued"]["record"]["id"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

fn revoked(rows: &[serde_json::Value]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            row["change"]["kind"]["revoked"]["grant_id"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

async fn native_inputs(fixture: &PersistentFixture, store: &dyn RuntimeStore) -> serde_json::Value {
    let rows = tokio::time::timeout(
        WAIT,
        store.load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "only the actual admitted Console Original may exist"
    );
    serde_json::to_value(rows).unwrap()
}

async fn sources_for(fixture: &PersistentFixture, fresh: &EvidenceId) -> Vec<StoredRecord> {
    fixture
        .store
        .list_records(&fixture.scope, None, 256)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| {
            row.data.schema == SOURCE_SCHEMA
                && fixture.source.decode_stored(row).unwrap().event.original == *fresh
        })
        .collect()
}

async fn published_sibling(
    fixture: &PersistentFixture,
    task: &InstalledServiceTask,
) -> (EvidenceId, StoredRecord, PrivateServiceGrantRef) {
    // Reuse the exact canonical acquisition, with a distinct real service call.
    // Rebuilding its timestamped request would create a conflicting admission.
    let (_, snapshot) = task.current().await.unwrap();
    let member = snapshot
        .request()
        .host()
        .continuation
        .native_original
        .as_ref()
        .unwrap()
        .member
        .as_ref()
        .unwrap()
        .member
        .clone();
    let sibling = fixture
        .source
        .prepare_service_task(
            fixture.store.clone(),
            snapshot,
            "batch-recovery-published-sibling",
            &member,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(sibling.fresh != task.fresh);
    let fresh = sibling.fresh.clone();
    let review = sibling
        .prepare_no_connector_review(
            &configuration("batch_sibling"),
            [id("read")].into(),
            Some(30_000),
            request(fixture),
        )
        .await
        .unwrap();
    let record = approve(review.request());
    assert_eq!(review.approve(record, || true).await, Ok(true));
    let mut rows = sources_for(fixture, &fresh).await;
    assert_eq!(rows.len(), 1);
    let row = rows.remove(0);
    let decoded = fixture.source.decode_stored(&row).unwrap();
    assert!(decoded.final_association.is_none() && decoded.service_admission.is_none());
    let link = decoded.approved_task.as_ref().unwrap();
    let grants = link.grants();
    assert_eq!(grants.len(), 1);
    (fresh, row, (*grants[0]).clone())
}

async fn assert_compensated(
    fixture: &PersistentFixture,
    store: &dyn RuntimeStore,
    task: &InstalledServiceTask,
    before: &[serde_json::Value],
    inputs: &serde_json::Value,
    sibling: &(EvidenceId, StoredRecord, PrivateServiceGrantRef),
    count: usize,
) {
    let after = journal(fixture);
    assert_eq!(
        &after[..before.len()],
        before,
        "every prior native journal entry is preserved"
    );
    let added = &after[before.len()..];
    let issued = issued(added);
    assert_eq!(
        issued.len(),
        count,
        "the intended partial boundary must have been reached"
    );
    assert_eq!(
        issued
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        count
    );
    assert_eq!(
        revoked(added),
        issued,
        "each entered parent then child is revoked exactly once"
    );
    assert_eq!(
        added.len(),
        count * 2,
        "no additional native issue, revoke or owner mutation"
    );
    assert!(
        sources_for(fixture, &task.fresh).await.is_empty(),
        "failed batch published no B source"
    );
    assert_eq!(
        sources_for(fixture, &sibling.0).await,
        vec![sibling.1.clone()]
    );
    fixture
        .source
        .require_service_grant_issued(&sibling.2)
        .await
        .unwrap();
    assert_eq!(
        &native_inputs(fixture, store).await,
        inputs,
        "no B admission or changed Original input"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_returned_failure_compensates_exact_members_and_preserves_published_sibling() {
    for boundary in ["second_parent", "source"] {
        let (mut fixture, history) =
            PersistentFixture::for_console_service_batch(selected_human(), |inner| inner).await;
        let result = AssertUnwindSafe(Box::pin(async {
            let (task, _pipe, logins) = console_task(&mut fixture).await;
            let sibling = published_sibling(&fixture, &task).await;
            let review = batch(&fixture, task.clone()).await;
            let record = approve(review.request());
            let before = journal(&fixture);
            let inputs = native_inputs(&fixture, history.as_ref()).await;
            let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
            let condition = if boundary == "second_parent" {
                "json_extract(NEW.data, '$.schema') = 'toolkit.operator-application-parent' AND \
                 json_extract(NEW.data, '$.body.batch_member.kind') = 'public_services_v1' AND \
                 json_extract(NEW.data, '$.body.batch_member.index') = 1"
            } else {
                "json_extract(NEW.data, '$.schema') = 'toolkit.authenticated-source-event' AND \
                 json_extract(NEW.data, '$.body.approved_task') IS NOT NULL"
            };
            sql.execute_batch(&format!(
                "CREATE TRIGGER refuse_batch_boundary BEFORE INSERT ON toolkit_records \
                WHEN {condition} BEGIN SELECT RAISE(ABORT, 'fixture refused batch boundary'); END;"
            ))
            .unwrap();
            assert_eq!(
                review.approve(record, || true).await,
                Err(PrivateOriginalError::Unavailable)
            );
            assert_compensated(
                &fixture,
                history.as_ref(),
                &task,
                &before,
                &inputs,
                &sibling,
                if boundary == "second_parent" { 2 } else { 6 },
            )
            .await;
            // Grant compensation is not a rollback acknowledgement for the
            // separately approved policy or any other aggregate review effect.
            let retired = logins.fixture_retire(41).unwrap().await.unwrap();
            assert!(retired.fenced && retired.reservations_drained && retired.uncertain);
        }))
        .catch_unwind()
        .await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

// Hold the real second parent's protected predicate after its Pending intent.
// The first parent's actual native issue and child issue have already settled.
struct SecondParentGate {
    once: AtomicBool,
    entered: Mutex<Option<oneshot::Sender<()>>>,
    open: Mutex<bool>,
    changed: Condvar,
}

impl SecondParentGate {
    fn release(&self) {
        if let Ok(mut open) = self.open.lock() {
            *open = true;
            self.changed.notify_all();
        }
    }

    fn wait_at_boundary(&self, path: &std::path::Path) -> bool {
        if self.once.load(Ordering::SeqCst) {
            return true;
        }
        let Ok(sql) = rusqlite::Connection::open(path) else {
            return false;
        };
        let pending = sql.query_row(
            "SELECT count(*) FROM toolkit_records WHERE \
            json_extract(data, '$.schema') = 'toolkit.operator-application-parent' AND \
            json_extract(data, '$.body.batch_member.kind') = 'public_services_v1' AND \
            json_extract(data, '$.body.batch_member.index') = 1 AND \
            json_extract(data, '$.body.issued.state') = 'pending'",
            [],
            |row| row.get::<_, i64>(0),
        );
        let Ok(pending) = pending else {
            return false;
        };
        if pending == 0 || self.once.swap(true, Ordering::SeqCst) {
            return true;
        }
        if pending != 1 {
            return false;
        }
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.changed.wait(open).unwrap();
        }
        true
    }
}

struct ReleaseGate(Arc<SecondParentGate>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_retired_review_joins_held_entry_then_compensates_partial_issuance() {
    let (mut fixture, history) =
        PersistentFixture::for_console_service_batch(selected_human(), |inner| inner).await;
    let (entered, receive) = oneshot::channel();
    let gate = Arc::new(SecondParentGate {
        once: AtomicBool::new(false),
        entered: Mutex::new(Some(entered)),
        open: Mutex::new(false),
        changed: Condvar::new(),
    });
    let release = ReleaseGate(gate.clone());
    let mut worker = None;
    let result = AssertUnwindSafe(Box::pin(async {
        let (task, _pipe, logins) = console_task(&mut fixture).await;
        let sibling = published_sibling(&fixture, &task).await;
        let review = batch(&fixture, task.clone()).await;
        let record = approve(review.request());
        let before = journal(&fixture);
        let inputs = native_inputs(&fixture, history.as_ref()).await;
        let current = Arc::new(AtomicBool::new(true));
        let presented = current.clone();
        let held = gate.clone();
        let path = fixture.source_path.clone();
        worker = Some(tokio::spawn(async move {
            review
                .approve(record, move || {
                    held.wait_at_boundary(&path) && presented.load(Ordering::SeqCst)
                })
                .await
        }));
        let entered = tokio::time::timeout(WAIT, receive)
            .await
            .is_ok_and(|value| value.is_ok());
        current.store(false, Ordering::SeqCst);
        let mut retirement = logins.fixture_retire(41).unwrap();
        let retained_before_release = retirement.as_mut().now_or_never().is_none();
        let review_pending = !worker.as_ref().unwrap().is_finished();
        let held_journal = journal(&fixture);
        // Join before asserting the adverse schedule. The release guard also
        // opens on panic, and the outer cleanup joins the same retained worker.
        gate.release();
        let outcome = worker.take().unwrap().await.unwrap();
        let retired = tokio::time::timeout(WAIT, retirement)
            .await
            .unwrap()
            .unwrap();
        assert!(entered && retained_before_release && review_pending);
        assert_eq!(issued(&held_journal[before.len()..]).len(), 2);
        assert!(revoked(&held_journal[before.len()..]).is_empty());
        assert_eq!(outcome, Err(PrivateOriginalError::Unavailable));
        assert!(retired.fenced && retired.reservations_drained && retired.uncertain);
        assert_compensated(
            &fixture,
            history.as_ref(),
            &task,
            &before,
            &inputs,
            &sibling,
            2,
        )
        .await;
    }))
    .catch_unwind()
    .await;
    drop(release);
    if let Some(worker) = worker {
        let _ = worker.await;
    }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
