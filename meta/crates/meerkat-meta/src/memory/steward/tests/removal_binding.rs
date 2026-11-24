//! What an operator's removal of a shared record binds the steward to, and
//! what it does not.

use super::*;
use crate::memory::distiller::{DistillerProfile, TombstoneSource, render_prompt};

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

/// A record an operator removed from the mob's shared memory is named in
/// the lists that tell a model what not to write again. The list is per
/// scope: the mob scope's holds the record's kind and title, the one of an
/// identity's own scope does not. A distiller's prompt rendered from the
/// mob scope's list shows the title, and so does a dream that covers the
/// mob's scope, under "Recent tombstones".
#[tokio::test]
async fn a_removed_shared_record_is_named_in_the_recent_tombstones_of_its_scope() {
    let own = identity_scope("identity:worker");
    let fixture = build_fixture(vec![empty_gather(), empty_consolidate()], vec![]);
    seed_active(
        &fixture.store,
        "mem-m",
        &mob_scope(),
        "Deploy window",
        "Deploys happen on Friday afternoons.",
    )
    .await;
    seed_active(
        &fixture.store,
        "mem-own",
        &own,
        "Standup time",
        "Standup is at ten.",
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

    let shared = fixture
        .store
        .recent_tombstones(&mob_scope(), 0, 32)
        .await
        .expect("the mob scope's recent tombstones");
    assert_eq!(
        shared
            .iter()
            .map(|tombstone| (tombstone.kind, tombstone.title.as_str()))
            .collect::<Vec<_>>(),
        vec![(MemoryKind::Fact, "Deploy window")]
    );
    let of_the_identity = fixture
        .store
        .recent_tombstones(&own, 0, 32)
        .await
        .expect("the identity scope's recent tombstones");
    assert!(of_the_identity.is_empty(), "{of_the_identity:?}");

    let distiller = render_prompt(
        &DistillerProfile::embedded_default(),
        &[],
        &shared,
        "(empty)",
    );
    assert!(
        distiller.contains("- [fact] Deploy window"),
        "the distiller's prompt over the mob's scope: {distiller}"
    );

    completed_dream(&fixture.engine).await;
    let prompts = fixture.llm.prompts();
    assert!(
        prompts.iter().any(|prompt| {
            prompt.contains("Recent tombstones (never re-create these):")
                && prompt.contains("- [fact] Deploy window")
        }),
        "the dream's prompts: {prompts:#?}"
    );
}

/// A purged record gives a model nothing to read. Neither a distiller's
/// prompt over the record's scope nor any prompt of a dream that covers it
/// holds the record's title, its description or its body; the scope's live
/// record is still shown in both.
#[tokio::test]
async fn a_purged_record_shows_no_title_or_content_in_a_distiller_or_a_dream_prompt() {
    const TITLE: &str = "Home address of the operator";
    const BODY: &str = "12 Example Lane, Springfield";
    let leaks = |text: &str| {
        text.contains(TITLE)
            || text.contains(BODY)
            || text.contains("Home address")
            || text.contains("Example Lane")
    };
    let own = identity_scope("identity:worker");
    let fixture = build_fixture(vec![empty_gather(), empty_consolidate()], vec![]);
    seed_active(&fixture.store, "mem-gone", &own, TITLE, BODY).await;
    seed_active(
        &fixture.store,
        "mem-own",
        &own,
        "Standup time",
        "Standup is at ten.",
    )
    .await;
    let removed = fixture
        .store
        .forget_authored(&own, "mem-gone", MemoryAuthor::Operator { principal: None })
        .await
        .expect("an operator retires the record");
    assert!(removed.deleted);
    let purged = fixture
        .store
        .purge_authored(&own, "mem-gone", MemoryAuthor::Operator { principal: None })
        .await
        .expect("an operator purges the retired record");
    assert!(purged.purged, "{purged:?}");

    let tombstones = fixture
        .store
        .recent_tombstones(&own, 0, 32)
        .await
        .expect("the scope's recent tombstones");
    assert!(
        tombstones.is_empty(),
        "the purged record is the scope's only retired one: {tombstones:?}"
    );
    let manifest = fixture
        .store
        .manifest(std::slice::from_ref(&own), ManifestTier::Full)
        .await
        .expect("the scope's manifest");
    let distiller = render_prompt(
        &DistillerProfile::embedded_default(),
        &manifest,
        &tombstones,
        "(empty)",
    );
    assert!(!leaks(&distiller), "the distiller's prompt: {distiller}");
    assert!(
        distiller.contains("Standup time"),
        "the scope's live record is still shown: {distiller}"
    );

    completed_dream(&fixture.engine).await;
    let prompts = fixture.llm.prompts();
    assert!(
        !prompts.iter().any(|prompt| leaks(prompt)),
        "no prompt of the dream shows the purged record: {prompts:#?}"
    );
    assert!(
        prompts.iter().any(|prompt| prompt.contains("Standup time")),
        "the dream still sees the scope's live record: {prompts:#?}"
    );
}

/// Whether a prompt holds a tombstone line with a kind and no title.
fn has_untitled_tombstone(prompt: &str) -> bool {
    prompt.lines().any(|line| line.trim_end() == "- [fact]")
}

/// A purged record is not in its scope's list of recent tombstones: the
/// list names a record by its title, and the purged row has none. The
/// retired record beside it that still holds its text stays listed, and
/// neither a distiller's prompt nor a dream's carries a tombstone line
/// with a kind and no title.
#[tokio::test]
async fn a_purged_record_is_left_out_of_the_recent_tombstones_of_its_scope() {
    let own = identity_scope("identity:worker");
    let fixture = build_fixture(vec![empty_gather(), empty_consolidate()], vec![]);
    for (id, title, body) in [
        (
            "mem-kept",
            "Old deploy window",
            "Deploys used to happen on Mondays.",
        ),
        (
            "mem-gone",
            "Home address of the operator",
            "12 Example Lane, Springfield",
        ),
    ] {
        seed_active(&fixture.store, id, &own, title, body).await;
        let removed = fixture
            .store
            .forget_authored(&own, id, MemoryAuthor::Operator { principal: None })
            .await
            .expect("an operator retires the record");
        assert!(removed.deleted, "{id}");
    }
    seed_active(
        &fixture.store,
        "mem-own",
        &own,
        "Standup time",
        "Standup is at ten.",
    )
    .await;
    let before = fixture
        .store
        .recent_tombstones(&own, 0, 32)
        .await
        .expect("the scope's recent tombstones");
    assert_eq!(
        before.len(),
        2,
        "both retired records are listed before the purge: {before:?}"
    );
    let purged = fixture
        .store
        .purge_authored(&own, "mem-gone", MemoryAuthor::Operator { principal: None })
        .await
        .expect("an operator purges the retired record");
    assert!(purged.purged, "{purged:?}");

    let listed = fixture
        .store
        .recent_tombstones(&own, 0, 32)
        .await
        .expect("the scope's recent tombstones");
    assert_eq!(
        listed
            .iter()
            .map(|tombstone| tombstone.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Old deploy window"]
    );

    let distiller = render_prompt(
        &DistillerProfile::embedded_default(),
        &[],
        &listed,
        "(empty)",
    );
    assert!(
        distiller.contains("- [fact] Old deploy window"),
        "the distiller's prompt: {distiller}"
    );
    assert!(
        !has_untitled_tombstone(&distiller),
        "the distiller's prompt: {distiller}"
    );

    completed_dream(&fixture.engine).await;
    let prompts = fixture.llm.prompts();
    assert!(
        prompts
            .iter()
            .any(|prompt| prompt.contains("- [fact] Old deploy window")),
        "the dream's prompts: {prompts:#?}"
    );
    assert!(
        !prompts.iter().any(|prompt| has_untitled_tombstone(prompt)),
        "the dream's prompts: {prompts:#?}"
    );
}

/// The list's limit counts the retired records that still hold their text.
/// With as many of those as the limit, and as many purged records retired
/// after them, every title is still listed: a purged row takes no place.
#[tokio::test]
async fn purged_records_take_no_place_in_the_limit_of_the_recent_tombstones() {
    const LIMIT: usize = 3;
    let own = identity_scope("identity:worker");
    let fixture = build_fixture(vec![], vec![]);
    let mut titles = Vec::new();
    for index in 0..LIMIT {
        let id = format!("mem-kept-{index}");
        let title = format!("Kept title {index}");
        seed_active(
            &fixture.store,
            &id,
            &own,
            &title,
            &format!("kept body {index}"),
        )
        .await;
        let removed = fixture
            .store
            .forget_authored(&own, &id, MemoryAuthor::Operator { principal: None })
            .await
            .expect("an operator retires the record");
        assert!(removed.deleted, "{id}");
        titles.push(title);
    }
    // Retired later than every record above: newest first, they come before.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    for index in 0..LIMIT {
        let id = format!("mem-gone-{index}");
        seed_active(
            &fixture.store,
            &id,
            &own,
            &format!("Purged title {index}"),
            &format!("purged body {index}"),
        )
        .await;
        let removed = fixture
            .store
            .forget_authored(&own, &id, MemoryAuthor::Operator { principal: None })
            .await
            .expect("an operator retires the record");
        assert!(removed.deleted, "{id}");
        let purged = fixture
            .store
            .purge_authored(&own, &id, MemoryAuthor::Operator { principal: None })
            .await
            .expect("an operator purges the retired record");
        assert!(purged.purged, "{id}: {purged:?}");
    }

    let mut listed: Vec<String> = fixture
        .store
        .recent_tombstones(&own, 0, LIMIT)
        .await
        .expect("the scope's recent tombstones")
        .into_iter()
        .map(|tombstone| tombstone.title)
        .collect();
    listed.sort();
    assert_eq!(listed, titles);
}
