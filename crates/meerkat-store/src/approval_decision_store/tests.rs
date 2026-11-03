//! Rows of the SQLite decision store: the lazily created file, conditional
//! writes that never wait, the refusal of a newer domain version, the doctor
//! and migrate inventory, indexed queries, and undecodable rows.

use std::time::{Duration, Instant};

use meerkat_core::approval::ApprovalStoreError;
use meerkat_core::approval::decision::{
    ApproverId, DecisionDescriptor, DecisionHoldRequest, DecisionKind, DecisionListFilter,
    DecisionMachineState, DecisionNotice, DecisionOperationId, DecisionPhase, DecisionPrincipalKey,
    DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionRow, DecisionStage,
    DecisionStore, DecisionSubjectKey, Sha256Digest, StoredDecision,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use rusqlite::params_from_iter;
use rusqlite::types::Value;

use super::{
    APPROVAL_DECISIONS_DOMAIN, APPROVAL_DECISIONS_FILE, DUE_SQL, OWED_SQL, SqliteDecisionStore,
    count_query, list_query,
};

/// A sealed row of `subject` in `phase`, owing `notice`, due at `deadline`.
fn stored(
    subject: &str,
    phase: DecisionPhase,
    notice: DecisionNotice,
    deadline: u64,
) -> StoredDecision {
    let requester = DecisionPrincipalKey::of(
        &PrincipalRef::in_domain(
            PrincipalKind::PersonalAgent,
            "builder",
            TrustDomainId::new("example.org").expect("trust domain"),
        )
        .expect("principal"),
    )
    .expect("principal key");
    let descriptor = DecisionDescriptor::seal(
        DecisionHoldRequest::new(
            DecisionKind::new("release").expect("kind"),
            DecisionSubjectKey::new(subject).expect("subject key"),
            DecisionOperationId::new("operation-7").expect("operation id"),
            Sha256Digest::of(br#"{"build":7}"#),
            DecisionRequestBody::from_value(&serde_json::json!({"build": 7}))
                .expect("request body"),
            requester,
            ApproverId::new("team:owners").expect("approver id"),
            deadline,
            DecisionRendering::new("Release build 7.").expect("rendering"),
            DecisionRenderer::new("render.v1").expect("renderer"),
        ),
        10,
    )
    .expect("seal");
    let decision_id = descriptor.decision_id();
    let state = DecisionMachineState {
        phase,
        decision_id: decision_id.as_str().to_owned(),
        deadline_at_ms: deadline,
        subject_digest: descriptor.subject_digest.as_str().to_owned(),
        operation_id: descriptor.operation_id.as_str().to_owned(),
        request_digest: descriptor.request_digest.as_str().to_owned(),
        requester: descriptor.requester.as_str().to_owned(),
        approver_id: format!("approver:{}", descriptor.approver_id),
        reviewer: None,
        stage: DecisionStage::Human,
        first_presented_to_approver_at_ms: None,
        presented_to_reviewer: false,
        evidence_digest: None,
        decided_at_ms: None,
        ended_at_ms: None,
        permit_issued: false,
        executed_request_digest: None,
        notice,
    };
    StoredDecision::held(descriptor, state)
}

fn open_row(subject: &str) -> StoredDecision {
    stored(subject, DecisionPhase::Open, DecisionNotice::NotDue, 50)
}

fn readable(row: Option<DecisionRow>) -> StoredDecision {
    match row {
        Some(DecisionRow::Readable(row)) => *row,
        other => panic!("expected a readable row, got {other:?}"),
    }
}

#[test]
fn the_file_is_created_by_the_first_hold_not_by_open_or_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteDecisionStore::in_store_path(dir.path());
    assert_eq!(store.path(), dir.path().join(APPROVAL_DECISIONS_FILE));
    assert!(store.is_persistent());

    let row = open_row("subject-1");
    assert_eq!(store.get(&row.decision_id).expect("get"), None);
    let filter = DecisionListFilter::default();
    assert!(store.list(&filter, None, 10).expect("list").rows.is_empty());
    assert!(store.owed(None, 10).expect("owed").rows.is_empty());
    assert!(store.due(100, None, 10).expect("due").rows.is_empty());
    assert_eq!(store.count(&filter).expect("count"), 0);
    assert_eq!(
        store.replace_if_revision(1, &row),
        Err(ApprovalStoreError::Conflict),
        "no row to replace"
    );
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read the store path")
        .collect();
    assert!(
        entries.is_empty(),
        "opening and reading create nothing: {entries:?}"
    );

    store.insert_if_absent(&row).expect("the first hold");
    assert!(store.path().is_file(), "the first hold creates the file");
    assert_eq!(readable(store.get(&row.decision_id).expect("get")), row);
}

#[test]
fn writes_are_revision_conditional_and_never_wait() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteDecisionStore::in_store_path(dir.path());
    let row = open_row("subject-1");
    store.insert_if_absent(&row).expect("insert");
    assert_eq!(
        store.insert_if_absent(&row),
        Err(ApprovalStoreError::Conflict),
        "one row per decision"
    );

    let mut next = row.clone();
    next.state.phase = DecisionPhase::Expired;
    next.state.ended_at_ms = Some(60);
    next.state.notice = DecisionNotice::Owed;
    next.revision = 2;
    assert_eq!(
        store.replace_if_revision(2, &next),
        Err(ApprovalStoreError::Conflict),
        "a stale revision writes nothing"
    );
    assert_eq!(readable(store.get(&row.decision_id).expect("get")), row);
    store.replace_if_revision(1, &next).expect("replace");
    assert_eq!(readable(store.get(&row.decision_id).expect("get")), next);

    // Another writer holds the database: a write is refused at once.
    let holder = rusqlite::Connection::open(store.path()).expect("open a second writer");
    holder
        .execute_batch("BEGIN IMMEDIATE")
        .expect("hold the write lock");
    let started = Instant::now();
    assert_eq!(
        store.insert_if_absent(&open_row("subject-2")),
        Err(ApprovalStoreError::Contended)
    );
    let mut after = next;
    after.revision = 3;
    assert_eq!(
        store.replace_if_revision(2, &after),
        Err(ApprovalStoreError::Contended)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a held database is refused, never waited for: {:?}",
        started.elapsed()
    );
    holder.execute_batch("ROLLBACK").expect("release the lock");
    store
        .replace_if_revision(2, &after)
        .expect("replace once released");
}

#[test]
fn a_future_domain_version_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteDecisionStore::in_store_path(dir.path());
    let row = open_row("subject-1");
    store.insert_if_absent(&row).expect("insert");
    let newer = rusqlite::Connection::open(store.path()).expect("open");
    let changed = newer
        .execute(
            "UPDATE meerkat_schema SET version = 2 WHERE domain = ?1",
            [APPROVAL_DECISIONS_DOMAIN.name],
        )
        .expect("stamp a newer version");
    assert_eq!(changed, 1);
    drop(newer);

    assert!(matches!(
        store.get(&row.decision_id),
        Err(ApprovalStoreError::NotCommitted(_))
    ));
    assert!(matches!(
        store.insert_if_absent(&open_row("subject-2")),
        Err(ApprovalStoreError::NotCommitted(_))
    ));
}

#[test]
fn doctor_and_migrate_know_the_decision_file_and_domain() {
    let files = [
        APPROVAL_DECISIONS_FILE,
        "sessions_jsonl/approval_decisions.sqlite3",
    ];
    for file in files {
        assert!(
            crate::migrate::REALM_SQLITE_FILES.contains(&file),
            "the maintenance fence and migrate cover {file}"
        );
        assert!(
            crate::doctor::REALM_DATABASE_FILES
                .iter()
                .any(|(relative, domains)| *relative == file
                    && **domains == [APPROVAL_DECISIONS_DOMAIN.name]),
            "the doctor probes {file} for its domain"
        );
    }
    assert!(crate::doctor::KNOWN_LEDGER_DOMAINS.contains(&APPROVAL_DECISIONS_DOMAIN.name));
    assert_eq!(
        crate::migrate::supported_domain_version(APPROVAL_DECISIONS_DOMAIN.name),
        Some(1)
    );
    assert_eq!(APPROVAL_DECISIONS_DOMAIN.name, "approval-decisions");
}

#[test]
fn owed_due_and_count_queries_use_their_indexes() {
    // The queries spell the phase and notice tokens.
    assert_eq!(DecisionPhase::Approved.as_str(), "approved");
    assert_eq!(DecisionPhase::Open.as_str(), "open");
    assert_eq!(DecisionNotice::Owed.as_str(), "owed");

    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteDecisionStore::in_store_path(dir.path());
    store
        .insert_if_absent(&open_row("subject-1"))
        .expect("create the file");
    let conn = rusqlite::Connection::open(store.path()).expect("open");
    let plan = |sql: &str, values: &[Value]| -> Vec<String> {
        let mut statement = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("explain");
        statement
            .query_map(params_from_iter(values.iter()), |row| {
                row.get::<_, String>(3)
            })
            .expect("plan")
            .collect::<Result<Vec<_>, _>>()
            .expect("plan rows")
    };
    let assert_indexed = |label: &str, details: &[String], indexes: &[&str]| {
        assert!(
            !details.iter().any(|detail| detail.starts_with("SCAN")),
            "{label} scans: {details:?}"
        );
        for index in indexes {
            assert!(
                details.iter().any(|detail| detail.contains(index)),
                "{label} does not use {index}: {details:?}"
            );
        }
    };

    let owed = plan(OWED_SQL, &[Value::Text(String::new()), Value::Integer(11)]);
    assert_indexed(
        "owed",
        &owed,
        &[
            "approval_decisions_phase_deadline_idx",
            "approval_decisions_notice_idx",
        ],
    );
    let due = plan(
        DUE_SQL,
        &[
            Value::Integer(100),
            Value::Text(String::new()),
            Value::Integer(11),
        ],
    );
    assert_indexed("due", &due, &["approval_decisions_phase_deadline_idx"]);

    for (label, filter, index) in [
        (
            "count by phase",
            DecisionListFilter::default().with_phase(DecisionPhase::Open),
            "approval_decisions_phase_deadline_idx",
        ),
        (
            "count by approver",
            DecisionListFilter::default()
                .with_approver_id(ApproverId::new("team:owners").expect("approver id")),
            "approval_decisions_approver_phase_idx",
        ),
        (
            "count by kind",
            DecisionListFilter::default()
                .with_kind(DecisionKind::new("release").expect("kind"))
                .with_phase(DecisionPhase::Open),
            "approval_decisions_kind_phase_idx",
        ),
    ] {
        let (sql, values) = count_query(&filter);
        assert_indexed(label, &plan(&sql, &values), &[index]);
    }
    let requester = open_row("subject-1").descriptor.requester;
    let (sql, values) = list_query(
        &DecisionListFilter::default().with_requester(requester),
        None,
        10,
    );
    assert_indexed(
        "list by requester",
        &plan(&sql, &values),
        &["approval_decisions_requester_phase_idx"],
    );
}

#[test]
fn queries_page_in_decision_id_order_and_keep_an_undecodable_row_unreadable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteDecisionStore::in_store_path(dir.path());
    let open = open_row("subject-open");
    let approved = stored(
        "subject-approved",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        500,
    );
    let owed = stored(
        "subject-owed",
        DecisionPhase::Rejected,
        DecisionNotice::Owed,
        50,
    );
    let mut held = stored(
        "subject-held",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        50,
    );
    held.state.permit_issued = true;
    for row in [&open, &approved, &owed, &held] {
        store.insert_if_absent(row).expect("insert");
    }

    let ids = |rows: &[DecisionRow]| -> Vec<String> {
        rows.iter()
            .map(|row| row.decision_id().to_owned())
            .collect()
    };
    let mut owed_ids = vec![
        approved.decision_id.as_str().to_owned(),
        owed.decision_id.as_str().to_owned(),
        held.decision_id.as_str().to_owned(),
    ];
    owed_ids.sort();
    assert_eq!(ids(&store.owed(None, 10).expect("owed").rows), owed_ids);
    assert_eq!(
        ids(&store.due(100, None, 10).expect("due").rows),
        vec![open.decision_id.as_str().to_owned()],
        "the held decision waits for its permit's settlement"
    );
    let first = store
        .list(&DecisionListFilter::default(), None, 2)
        .expect("first page");
    assert_eq!(first.rows.len(), 2);
    let after = first.next_after.expect("another page");
    let second = store
        .list(&DecisionListFilter::default(), Some(&after), 2)
        .expect("second page");
    assert_eq!(second.rows.len(), 2);
    assert_eq!(second.next_after, None);
    let mut all = ids(&first.rows);
    all.extend(ids(&second.rows));
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(all, sorted, "pages follow decision id order");
    assert_eq!(
        store
            .count(&DecisionListFilter::default().with_phase(DecisionPhase::Approved))
            .expect("count"),
        2
    );

    let conn = rusqlite::Connection::open(store.path()).expect("open");
    conn.execute(
        "UPDATE approval_decisions SET row_json = ?1 WHERE decision_id = ?2",
        rusqlite::params![b"{not json".to_vec(), open.decision_id.as_str()],
    )
    .expect("tear the row");
    drop(conn);
    match store.get(&open.decision_id).expect("get") {
        Some(DecisionRow::Unreadable(row)) => {
            assert_eq!(row.decision_id, open.decision_id.as_str());
        }
        other => panic!("an undecodable row is unreadable, got {other:?}"),
    }
    let listed = store
        .list(&DecisionListFilter::default(), None, 10)
        .expect("list");
    assert_eq!(listed.rows.len(), 4, "the other rows still read");
    assert_eq!(
        listed
            .rows
            .iter()
            .filter(|row| matches!(row, DecisionRow::Unreadable(_)))
            .count(),
        1
    );
}

#[test]
fn an_approved_decision_with_no_permit_past_its_deadline_is_due() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteDecisionStore::in_store_path(dir.path());
    let lapsed = stored(
        "subject-lapsed",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        50,
    );
    let mut held = stored(
        "subject-held",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        50,
    );
    held.state.permit_issued = true;
    let fresh = stored(
        "subject-fresh",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        500,
    );
    for row in [&lapsed, &held, &fresh] {
        store.insert_if_absent(row).expect("insert");
    }
    let due = |now_ms: u64| -> Vec<String> {
        store
            .due(now_ms, None, 10)
            .expect("due")
            .rows
            .iter()
            .map(|row| row.decision_id().to_owned())
            .collect()
    };
    let lapsed_only = vec![lapsed.decision_id.as_str().to_owned()];
    assert!(due(49).is_empty(), "before its deadline nothing is due");
    assert_eq!(due(50), lapsed_only, "at its deadline it is due");
    assert_eq!(
        due(100),
        lapsed_only,
        "past it, while the held one waits for its permit's settlement"
    );
}
