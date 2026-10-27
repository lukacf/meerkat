//! Real native issuance/revocation and canonical SQLite failure controls.
//! These tests do not install an MCP, authenticate a provider or restore V39.
//! They qualify no Console login, HTTP, abrupt-drop or process-crash recovery.
use super::*;

async fn prepared(fixture: &ContinuationFixture, task: InstalledServiceTask)
    -> PreparedServiceInstruction
{
    let mut config = persistence_config();
    let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &mut config.transport else {
        unreachable!()
    };
    launch.secret_env.clear();
    task.prepare_no_connector_review(&config, [id("read")].into(), None,
        persistence_review_request(fixture)).await.unwrap()
}

fn approved(review: &PreparedServiceInstruction) -> ApprovalRecord {
    let approvals = ApprovalService::new();
    let pending = approvals.request(review.request().clone()).unwrap();
    approvals.decide(&pending.approval_id, ApprovalDecision::Approve,
        ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(), None, None).unwrap()
}

fn changes(fixture: &ContinuationFixture) -> Vec<serde_json::Value> {
    let bytes = std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap();
    std::str::from_utf8(&bytes).unwrap().lines().map(|line|
        serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter_map(|line| line.get("change").cloned()).collect()
}

fn issued_ids(changes: &[serde_json::Value]) -> Vec<String> {
    changes.iter().filter_map(|change| change["kind"]["issued"]["record"]["id"]
        .as_str().map(str::to_owned)).collect()
}
fn revoked_ids(changes: &[serde_json::Value]) -> Vec<String> {
    changes.iter().filter_map(|change| change["kind"]["revoked"]["grant_id"]
        .as_str().map(str::to_owned)).collect()
}

fn fail_source_insert(fixture: &ContinuationFixture) -> rusqlite::Connection {
    let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
    sql.execute_batch("CREATE TRIGGER fail_approved_source BEFORE INSERT ON toolkit_records
        WHEN json_extract(NEW.data, '$.schema') = 'toolkit.authenticated-source-event'
        AND json_extract(NEW.data, '$.body.approved_task') IS NOT NULL
        BEGIN SELECT RAISE(ABORT, 'fixture refused approved source'); END;").unwrap();
    sql
}

async fn fresh_sources(fixture: &ContinuationFixture, task: &InstalledServiceTask) -> Vec<StoredRecord> {
    fixture.store.list_records(&fixture.scope, None, 256).await.unwrap().into_iter()
        .filter(|row| row.data.schema == SOURCE_SCHEMA
            && fixture.source.decode_stored(row).unwrap().event.original == task.fresh).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_approval_compensates_native_issuance_after_canonical_write_failures() {
    for boundary in ["source", "parent_ack", "child_ack"] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let task = persistence_task(&fixture).await;
            let review = prepared(&fixture, task.clone()).await;
            let record = approved(&review);
            let evidence = approval_evidence(&record).unwrap();
            let grant = PrivateOriginals::service_mcp_reference(review.review.decision(), review.review.selection()).unwrap();
            let before = changes(&fixture);
            let sql = if boundary == "source" { fail_source_insert(&fixture) } else {
                let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
                let (schema, field) = if boundary == "parent_ack" {
                    ("toolkit.operator-application-parent", "issued")
                } else { ("toolkit.authenticated-service-access-decision", "issuance") };
                sql.execute_batch(&format!("CREATE TRIGGER fail_issued_ack BEFORE UPDATE OF data ON toolkit_records
                    WHEN json_extract(NEW.data, '$.schema') = '{schema}'
                    AND json_extract(NEW.data, '$.body.{field}.state') = 'issued'
                    BEGIN SELECT RAISE(ABORT, 'fixture refused issuance acknowledgement'); END;")).unwrap();
                sql
            };
            assert_eq!(review.approve(record, || true).await, Err(PrivateOriginalError::Unavailable));
            let after = changes(&fixture);
            assert_eq!(&after[..before.len()], before.as_slice());
            let added = &after[before.len()..];
            let issued = issued_ids(added);
            assert_eq!(issued.len(), if boundary == "parent_ack" { 1 } else { 2 });
            assert_eq!(revoked_ids(added), issued,
                "native must acknowledge revocation of every issued grant, parent then child");
            assert!(fresh_sources(&fixture, &task).await.is_empty());
            if boundary == "source" {
                sql.execute_batch("DROP TRIGGER fail_approved_source;").unwrap();
                assert!(task.clone().retain_approved(grant, evidence, &|| Ok(())).await.is_err(),
                    "a publisher that previously observed issuance must recheck inside the source writer");
                assert!(fresh_sources(&fixture, &task).await.is_empty());
                assert_eq!(changes(&fixture), after, "late publication cannot issue or revive grants");
            }
            assert_eq!(fixture.inputs().await, Ok(1));
            drop(sql);
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(payload) = result { std::panic::resume_unwind(payload); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_approval_compensation_preserves_published_replay_and_other_approval_child() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let first = prepared(&fixture, task.clone()).await;
        let replay = prepared(&fixture, task.clone()).await;
        let replacement = prepared(&fixture, task.clone()).await;
        let record = approved(&first);
        let first_cleanup = first.operator.service_mcp_parent_compensation(&first.review, &record).unwrap().unwrap();
        let grant = PrivateOriginals::service_mcp_reference(first.review.decision(), first.review.selection()).unwrap();
        assert_eq!(first.approve(record.clone(), || true).await, Ok(true));
        let parent = first_cleanup.issued().await.unwrap().unwrap();
        let (_, child) = fixture.source.service_compensation_child(&grant, &parent).await.unwrap().unwrap();
        let source_before = fresh_sources(&fixture, &task).await;
        assert_eq!(source_before.len(), 1);
        let before_replay = changes(&fixture);
        assert_eq!(replay.approve(record, || false).await, Err(PrivateOriginalError::Denied));
        assert_eq!(changes(&fixture), before_replay, "an exact published approval cannot be revoked by a refused replay");
        let other_record = approved(&replacement);
        assert!(replacement.approve(other_record, || true).await.is_err());
        let after = changes(&fixture);
        let added = &after[before_replay.len()..];
        let new_parents = issued_ids(added);
        assert_eq!(new_parents.len(), 1);
        assert_eq!(revoked_ids(added), new_parents, "only this different approval's new parent belongs to cleanup");
        assert_eq!(fresh_sources(&fixture, &task).await, source_before);
        fixture.source.require_service_grant_issued(&grant).await.unwrap();
        assert_eq!(fixture.source.owner.grants().issued_reference(&child.grant_id).unwrap(), Some(child));
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_approval_compensation_cancellation_and_busy_native_custody_remain_distinct() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let review = prepared(&fixture, task.clone()).await;
        let record = approved(&review);
        let cleanup = review.operator.service_mcp_parent_compensation(&review.review, &record).unwrap().unwrap();
        let grant = PrivateOriginals::service_mcp_reference(review.review.decision(), review.review.selection()).unwrap();
        let before = changes(&fixture);
        let _sql = fail_source_insert(&fixture);
        // Actual controller custody is busy. The joined error must retain
        // native issuance as unresolved, never equate an attempt with revoke.
        let held = fixture.machine.try_controller_grant_mutation().unwrap();
        assert_eq!(review.approve(record, || true).await, Err(PrivateOriginalError::Unavailable));
        let failed = changes(&fixture);
        let issued = issued_ids(&failed[before.len()..]);
        assert_eq!(issued.len(), 2);
        assert!(revoked_ids(&failed[before.len()..]).is_empty());
        drop(held);
        fixture.source.record_task_group_cancelled(&task.parent).await.unwrap();
        assert_eq!(task.check_setup_current().await, Err(PrivateOriginalError::Denied));
        task.compensate_unpublished_approval(cleanup, grant).await.unwrap();
        let after = changes(&fixture);
        assert_eq!(issued_ids(&after[before.len()..]), issued, "cleanup cannot issue replacement grants");
        assert_eq!(revoked_ids(&after[before.len()..]), issued,
            "parent cancellation must not prevent joined exact native cleanup");
        assert!(fresh_sources(&fixture, &task).await.is_empty());
        assert_eq!(fixture.inputs().await, Ok(1));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}
