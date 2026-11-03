#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! End-to-end test for the mob/run label sidecar.
//!
//! Drives `RuntimeMetadataTable` (the in-memory store that backs
//! `UnifiedRuntime::set_mob_labels` / `set_run_labels`) directly. Bootstrapping
//! a full `UnifiedRuntime` requires LLM clients, mob definitions, and storage
//! adapters — none of which the metadata side car depends on, so we exercise
//! the table at its boundary.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use meerkat_meta::unified_runtime::mob_events::MobEventsStore;
use meerkat_meta::{MetadataScope, RuntimeMetadataTable};
use meerkat_mob::event::{MobEvent, MobEventKind};
use meerkat_mob::ids::{FlowId, MobId, RunId};

fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[tokio::test]
async fn mob_and_run_labels_set_get_delete_round_trip() {
    let table = RuntimeMetadataTable::new();
    let mob = "mob-fixture".to_string();
    let mob_scope = MetadataScope::Mob(mob.clone());
    let run_a = MetadataScope::Run(mob.clone(), "run-a".to_string());
    let run_b = MetadataScope::Run(mob.clone(), "run-b".to_string());

    table
        .set_labels(
            mob_scope.clone(),
            labels(&[("repo", "agents"), ("env", "dev")]),
        )
        .await;
    table
        .set_labels(run_a.clone(), labels(&[("trace_id", "alpha")]))
        .await;
    table
        .set_labels(run_b.clone(), labels(&[("trace_id", "beta")]))
        .await;

    // Read everything back.
    let got_mob = table.get_labels(&mob_scope).await;
    assert_eq!(got_mob.get("repo").map(String::as_str), Some("agents"));
    assert_eq!(got_mob.get("env").map(String::as_str), Some("dev"));
    let got_a = table.get_labels(&run_a).await;
    assert_eq!(got_a.get("trace_id").map(String::as_str), Some("alpha"));
    let got_b = table.get_labels(&run_b).await;
    assert_eq!(got_b.get("trace_id").map(String::as_str), Some("beta"));

    // Listing returns mob + both runs.
    let listed = table.list_labels_for_mob(&mob).await;
    assert_eq!(listed.len(), 3);
    let scopes: Vec<&MetadataScope> = listed.iter().map(|(s, _)| s).collect();
    assert!(scopes.contains(&&mob_scope));
    assert!(scopes.contains(&&run_a));
    assert!(scopes.contains(&&run_b));

    // Replacement is wholesale (not a merge).
    table
        .set_labels(mob_scope.clone(), labels(&[("repo", "platform")]))
        .await;
    let after_replace = table.get_labels(&mob_scope).await;
    assert_eq!(after_replace.len(), 1);
    assert_eq!(
        after_replace.get("repo").map(String::as_str),
        Some("platform")
    );
    assert!(!after_replace.contains_key("env"));

    // Delete one run; the others remain.
    let prev = table.delete_labels(&run_a).await;
    assert_eq!(
        prev.unwrap().get("trace_id").map(String::as_str),
        Some("alpha")
    );
    assert!(table.get_labels(&run_a).await.is_empty());
    assert_eq!(
        table
            .get_labels(&run_b)
            .await
            .get("trace_id")
            .map(String::as_str),
        Some("beta"),
    );
    assert!(!table.get_labels(&mob_scope).await.is_empty());
}

#[tokio::test]
async fn run_scope_isolated_per_mob() {
    let table = RuntimeMetadataTable::new();
    let scope_a = MetadataScope::Run("mob-a".to_string(), "run-1".to_string());
    let scope_b = MetadataScope::Run("mob-b".to_string(), "run-1".to_string());
    table
        .set_labels(scope_a.clone(), labels(&[("k", "value-a")]))
        .await;
    table
        .set_labels(scope_b.clone(), labels(&[("k", "value-b")]))
        .await;
    assert_eq!(
        table
            .get_labels(&scope_a)
            .await
            .get("k")
            .map(String::as_str),
        Some("value-a")
    );
    assert_eq!(
        table
            .get_labels(&scope_b)
            .await
            .get("k")
            .map(String::as_str),
        Some("value-b")
    );

    let listed_a = table.list_labels_for_mob("mob-a").await;
    assert_eq!(listed_a.len(), 1);
    let listed_b = table.list_labels_for_mob("mob-b").await;
    assert_eq!(listed_b.len(), 1);
}

#[tokio::test]
async fn empty_label_set_clears_entry() {
    let table = RuntimeMetadataTable::new();
    let scope = MetadataScope::Mob("mob-x".to_string());
    table.set_labels(scope.clone(), labels(&[("a", "1")])).await;
    table.set_labels(scope.clone(), BTreeMap::new()).await;
    assert!(table.get_labels(&scope).await.is_empty());
}

#[tokio::test]
async fn delete_missing_returns_none() {
    let table = RuntimeMetadataTable::new();
    let scope = MetadataScope::Mob("never-set".to_string());
    assert!(table.delete_labels(&scope).await.is_none());
}

/// When the structural events store is wired to the metadata table,
/// every projected envelope picks up the matching mob/run labels at
/// projection time. Closes the loop between Unit 5 (labels) and Unit 1
/// (events) — the deferred join from the original split landings.
#[tokio::test]
async fn structural_event_envelope_carries_mob_and_run_labels() {
    let table = Arc::new(RuntimeMetadataTable::new());
    let mob_id = MobId::from("mob-events-with-labels");
    let run_id = RunId::new();

    table
        .set_labels(
            MetadataScope::Mob(mob_id.as_str().to_string()),
            labels(&[("repo", "agents"), ("env", "prod")]),
        )
        .await;
    table
        .set_labels(
            MetadataScope::Run(mob_id.as_str().to_string(), run_id.to_string()),
            labels(&[("trace_id", "abc-123")]),
        )
        .await;

    let store = MobEventsStore::new().with_metadata_table(table.clone());

    let event = MobEvent {
        cursor: 0,
        timestamp: Utc::now(),
        mob_id: mob_id.clone(),
        kind: MobEventKind::FlowStarted {
            run_id: run_id.clone(),
            flow_id: FlowId::from("demo"),
            params: serde_json::Value::Null,
        },
    };
    let envelope = store.project_mob_event(&event).await;

    assert_eq!(
        envelope.run_id.as_deref(),
        Some(run_id.to_string().as_str())
    );
    assert_eq!(
        envelope.mob_labels.get("repo").map(String::as_str),
        Some("agents")
    );
    assert_eq!(
        envelope.mob_labels.get("env").map(String::as_str),
        Some("prod")
    );
    assert_eq!(
        envelope.run_labels.get("trace_id").map(String::as_str),
        Some("abc-123")
    );
}

/// Events without a `run_id` (e.g. mob-level lifecycle) still carry mob
/// labels but have an empty `run_labels` set.
#[tokio::test]
async fn mob_level_events_have_empty_run_labels() {
    let table = Arc::new(RuntimeMetadataTable::new());
    let mob_id = MobId::from("mob-level-only");
    table
        .set_labels(
            MetadataScope::Mob(mob_id.as_str().to_string()),
            labels(&[("env", "stage")]),
        )
        .await;

    let store = MobEventsStore::new().with_metadata_table(table);
    let event = MobEvent {
        cursor: 0,
        timestamp: Utc::now(),
        mob_id: mob_id.clone(),
        kind: MobEventKind::MobReset,
    };
    let envelope = store.project_mob_event(&event).await;
    assert!(envelope.run_id.is_none());
    assert_eq!(
        envelope.mob_labels.get("env").map(String::as_str),
        Some("stage")
    );
    assert!(envelope.run_labels.is_empty());
}
