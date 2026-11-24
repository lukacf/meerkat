//! What an operator's removal of a shared record binds the steward to, and
//! what it does not.

use super::*;

fn mob_create(title: &str, body: &str) -> String {
    consolidate_with(
        serde_json::json!([
            {"op": "create", "kind": "fact",
             "scope": {"kind": "mob", "key": "mob:main"},
             "title": title, "body": body}
        ]),
        serde_json::json!([]),
        serde_json::json!([]),
        serde_json::json!([]),
    )
}

/// A removal binds the steward by its guard against re-creating a retired
/// record only: the exact title and body, for seven days. The steward still
/// reads what the shared record was made from, so the same knowledge in
/// other words is committed again by a later dream. The setting that keeps
/// it out is the write limit: with `write_scopes = "identity"` nothing
/// returns to the mob's scope.
#[tokio::test]
async fn steward_can_write_a_reworded_copy_after_a_removal_unless_write_scopes_excludes_mob() {
    let fixture = build_fixture(
        vec![
            empty_gather(),
            mob_create("Deploy window", "Deploys happen on Friday afternoons."),
            empty_gather(),
            mob_create("Deploy day", "The team deploys on Fridays, after lunch."),
            empty_gather(),
            mob_create("When we deploy", "Friday afternoon is deploy time."),
        ],
        vec![],
    );
    seed_active(
        &fixture.store,
        "mem-m",
        &mob_scope(),
        "Deploy window",
        "Deploys happen on Friday afternoons.",
    )
    .await;
    let removed = fixture
        .store
        .forget_authored(
            &mob_scope(),
            "mem-m",
            MemoryAuthor::Operator { principal: None },
        )
        .await
        .expect("an operator's removal");
    assert!(removed.deleted);
    assert!(mob_titles(&fixture).await.is_empty());

    // The same text again: refused by the existing guard.
    let exact = completed_dream(&fixture.engine).await;
    assert!(mob_titles(&fixture).await.is_empty(), "{:?}", exact.skips);
    assert!(
        exact
            .skips
            .iter()
            .any(|skip| skip.contains("failed validation")),
        "{:?}",
        exact.skips
    );

    // The same knowledge in other words: committed.
    let reworded = completed_dream(&fixture.engine).await;
    assert_eq!(
        mob_titles(&fixture).await,
        vec!["Deploy day"],
        "{:?}",
        reworded.skips
    );

    // Under the write limit nothing more reaches the mob's scope.
    let limited = engine_over(
        &fixture,
        limited_config(StewardWriteScopes::Identity),
        Arc::new(SingleMobSource),
    );
    let stopped = completed_dream(&limited).await;
    assert_eq!(
        mob_titles(&fixture).await,
        vec!["Deploy day"],
        "{:?}",
        stopped.skips
    );
    assert_eq!(limit_skips(&stopped), 1, "{:?}", stopped.skips);
}
