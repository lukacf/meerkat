//! More rows for the steward's write limit (`write_scopes`): the review of
//! a quarantined shared record, a realm-scoped record, and an op that names
//! a create the limit dropped.

use super::*;

fn realm_scope() -> MemoryScope {
    MemoryScope::Realm {
        realm: REALM.to_string(),
    }
}

/// A quarantined record in a shared `scope`. An agent cannot write there
/// (its shared writes are proposals), so the record is one an earlier dream
/// committed while the write gate fired for it: a steward's create whose
/// evidence cites the tainted session.
async fn seed_quarantined_in(
    store: &SqliteAgentMemoryStore,
    id: &str,
    scope: &MemoryScope,
    title: &str,
    body: &str,
) -> String {
    let mut record = new_record(title, body);
    record.evidence = vec![EvidenceRef {
        session_id: "tainted-sess".to_string(),
        generation: 0,
        revision: None,
        range: None,
    }];
    let batch = StagedMutationBatch {
        kind: StagedBatchKind::FreshWrite,
        realm: REALM.to_string(),
        author: MemoryAuthor::Steward {
            run_id: "dream-earlier".to_string(),
        },
        ops: vec![StagedOp::Create {
            id: Some(id.to_string()),
            scope: scope.clone(),
            record,
            trust: TrustTier::AgentObserved,
            derived_from: Vec::new(),
            rationale: None,
            created_at_ms: None,
            updated_at_ms: None,
        }],
    };
    let token = store.stage(batch).await.expect("stage");
    store.commit(token).await.expect("commit");
    let seeded = store
        .records_by_ids(REALM, &[id.to_string()])
        .await
        .expect("read")
        .remove(0);
    assert!(
        matches!(seeded.status, RecordStatus::Quarantined { .. }),
        "seed must land quarantined: {:?}",
        seeded.status
    );
    seeded.id
}

/// A release and a tombstone of a quarantined record both change the
/// record's scope in place. Under the limit the dream decides neither for a
/// record in the mob's scope (an operator reviews it), and still decides a
/// record in an identity's scope.
#[tokio::test]
async fn write_scopes_identity_skips_the_review_of_a_quarantined_mob_record() {
    let fixture = build_fixture_limited(
        vec![empty_gather(), "PLACEHOLDER-CONSOLIDATE".to_string()],
        vec![],
        StewardWriteScopes::Identity,
    );
    let released = seed_quarantined_in(
        &fixture.store,
        "mem-q1",
        &mob_scope(),
        "Shared claim one",
        "body of the first shared claim",
    )
    .await;
    let tombstoned = seed_quarantined_in(
        &fixture.store,
        "mem-q2",
        &mob_scope(),
        "Shared claim two",
        "body of the second shared claim",
    )
    .await;
    let own = seed_quarantined(
        &fixture.store,
        "identity:worker",
        "Worker claim",
        "body of the worker's claim",
    )
    .await;
    script_consolidate(
        &fixture,
        consolidate_with(
            serde_json::json!([]),
            serde_json::json!([]),
            serde_json::json!([
                {"record_id": released, "verdict": "release", "rationale": "looks fine"},
                {"record_id": tombstoned, "verdict": "tombstone", "rationale": "looks wrong"},
                {"record_id": own, "verdict": "tombstone", "rationale": "looks wrong"}
            ]),
            serde_json::json!([]),
        ),
    );
    let run = completed_dream(&fixture.engine).await;

    for id in [&released, &tombstoned] {
        assert!(
            matches!(
                record_of(&fixture, id).await.status,
                RecordStatus::Quarantined { .. }
            ),
            "{:?}",
            run.skips
        );
    }
    assert!(mob_titles(&fixture).await.is_empty(), "{:?}", run.skips);
    assert_eq!(
        record_of(&fixture, &own).await.status,
        RecordStatus::Tombstoned,
        "{:?}",
        run.skips
    );
    assert_eq!(run.verdicts.quarantine_released, 0, "{:?}", run.skips);
    assert_eq!(run.verdicts.quarantine_tombstoned, 1, "{:?}", run.skips);
    assert_eq!(limit_skips(&run), 2, "{:?}", run.skips);
    assert!(
        run.skips
            .iter()
            .any(|skip| skip.contains("quarantine release") && skip.contains("write_scopes")),
        "{:?}",
        run.skips
    );
}

/// The realm's own scope is a shared scope too: under the limit a record
/// there is not retired by a dream.
#[tokio::test]
async fn write_scopes_identity_drops_the_tombstone_of_a_realm_record() {
    let fixture = build_fixture_limited(
        vec![
            empty_gather(),
            consolidate_with(
                serde_json::json!([
                    {"op": "tombstone", "id": "mem-realm", "rationale": "stale"},
                    {"op": "tombstone", "id": "mem-w1", "rationale": "stale"}
                ]),
                serde_json::json!([]),
                serde_json::json!([]),
                serde_json::json!([]),
            ),
        ],
        vec![],
        StewardWriteScopes::Identity,
    );
    seed_active(
        &fixture.store,
        "mem-realm",
        &realm_scope(),
        "Realm note",
        "realm body",
    )
    .await;
    seed_active(
        &fixture.store,
        "mem-w1",
        &identity_scope("identity:worker"),
        "Worker note one",
        "worker body",
    )
    .await;
    let run = completed_dream(&fixture.engine).await;

    assert_eq!(
        record_of(&fixture, "mem-realm").await.status,
        RecordStatus::Active,
        "{:?}",
        run.skips
    );
    assert_eq!(
        record_of(&fixture, "mem-w1").await.status,
        RecordStatus::Tombstoned,
        "{:?}",
        run.skips
    );
    assert_eq!(limit_skips(&run), 1, "{:?}", run.skips);
}

/// An op that names the id a dropped create declared is dropped on its
/// own: the rest of the group still commits, where the dangling id would
/// have failed the whole group at validation.
#[tokio::test]
async fn write_scopes_identity_drops_an_op_that_names_a_dropped_create_alone() {
    let fixture = build_fixture_limited(
        vec![
            empty_gather(),
            consolidate_with(
                serde_json::json!([
                    {"op": "create", "id": "c1", "kind": "fact",
                     "scope": {"kind": "mob", "key": "mob:main"},
                     "title": "Shared insight", "body": "everyone should know this"},
                    {"op": "create", "kind": "fact",
                     "scope": {"kind": "identity", "key": "identity:worker"},
                     "title": "Derived insight", "body": "built on the shared one",
                     "derived_from": ["c1"]},
                    {"op": "create", "kind": "fact",
                     "scope": {"kind": "identity", "key": "identity:worker"},
                     "title": "Worker-only insight", "body": "the worker should know this"}
                ]),
                serde_json::json!([]),
                serde_json::json!([]),
                serde_json::json!([]),
            ),
        ],
        vec![],
        StewardWriteScopes::Identity,
    );
    let run = completed_dream(&fixture.engine).await;

    assert!(mob_titles(&fixture).await.is_empty(), "{:?}", run.skips);
    assert_eq!(
        titles_in(&fixture, &identity_scope("identity:worker")).await,
        vec!["Worker-only insight"],
        "{:?}",
        run.skips
    );
    assert_eq!(run.ops_committed, 1, "{:?}", run.skips);
    assert_eq!(limit_skips(&run), 1, "{:?}", run.skips);
    assert!(
        run.skips
            .iter()
            .any(|skip| skip.contains("derives from unknown 'c1'")),
        "{:?}",
        run.skips
    );
    assert!(
        !run.skips
            .iter()
            .any(|skip| skip.contains("failed validation")),
        "{:?}",
        run.skips
    );
}
