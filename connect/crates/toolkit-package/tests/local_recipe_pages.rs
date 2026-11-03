#![cfg(feature = "local-recipes")]

use serde_json::{Value, json};
use toolkit_package::*;
use toolkit_store::*;

const PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json");
const MCP: &[u8] = include_bytes!("../integrations/connectors/github-readonly/0.1.0/mcp.json");
const RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json");
const PACKAGE: &[u8] = include_bytes!("fixtures/python-install/package.json");
const SELECTION: &[u8] = include_bytes!("fixtures/python-install/activation.json");

fn scope(owner: &str) -> StoreScope {
    StoreScope {
        realm: "page-realm".into(),
        owner: owner.into(),
    }
}

fn evidence(name: &str) -> EvidenceReference {
    EvidenceReference {
        owner: Id::new("observations").unwrap(),
        record: Id::new(name).unwrap(),
        revision: Revision::new("1").unwrap(),
    }
}

async fn save_remote(store: &SqliteStore, owner: &str, name: &str) {
    let mut plugin: Value = serde_json::from_slice(PLUGIN).unwrap();
    plugin["extensions"]["ai.rkat.toolkit"]["identity"]["name"] = json!(name);
    LocalRecipeStore::new(store, scope(owner))
        .unwrap()
        .save(
            &serde_json::to_vec(&plugin).unwrap(),
            Some(MCP),
            RECIPES,
            evidence("decision"),
        )
        .await
        .unwrap();
}

async fn save_selection(
    store: &SqliteStore,
    name: &str,
    references: usize,
) -> RetainedLocalSelection {
    let mut package: IntegrationPackage = serde_json::from_slice(PACKAGE).unwrap();
    package.identity.name = Id::new(name).unwrap();
    let mut selection: ActivationManifest = serde_json::from_slice(SELECTION).unwrap();
    selection.package = package.identity.clone();
    selection.package_content_digest = package.content_digest().unwrap();
    let verification = (0..references)
        .map(|n| EvidenceReference {
            owner: Id::new("o".repeat(180)).unwrap(),
            record: Id::new(format!("{n:03}{}", "r".repeat(177))).unwrap(),
            revision: Revision::new("v".repeat(256)).unwrap(),
        })
        .collect();
    LocalRecipeStore::new(store, scope("alice"))
        .unwrap()
        .save_selection(
            &serde_json::to_vec(&package).unwrap(),
            &serde_json::to_vec(&selection).unwrap(),
            LocalSelectionProfile::Python,
            Id::new("python-server").unwrap(),
            evidence("decision"),
            verification,
        )
        .await
        .unwrap()
}

fn assert_page(page: &RetainedLocalRecipePage, cursor: Option<&str>, limit: u32) {
    let value = page.wire_value();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["scope"], "retained_recipe_metadata");
    assert_eq!(value["status"], "listed_page");
    assert_eq!(page.request_cursor.as_deref(), cursor);
    assert_eq!(page.limit, limit);
    assert!(page.recipes.len() + page.unverified.len() + page.oversized.len() <= limit as usize);
    assert!(page.scanned_records <= 200);
    assert!(serde_json::to_vec(&value).unwrap().len() < 256 * 1024);
    if let Some(next) = &page.next_cursor {
        assert_ne!(Some(next.as_str()), cursor);
        assert!(next.len() <= 4096 && next.len() % 2 == 0);
        assert!(
            next.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }
}

#[tokio::test]
async fn pages_reopen_exactly_and_preserve_legacy_summaries_and_owner_scope() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("recipes.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    for n in 0..7 {
        save_remote(&store, "alice", &format!("item-{n:02}")).await;
    }
    save_remote(&store, "bob", "other-owner").await;
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let expected: Vec<_> = recipes
        .list_retained()
        .await
        .unwrap()
        .verified
        .iter()
        .map(|r| r.summary().unwrap())
        .collect();
    let first = recipes.list_retained_page(None, 2).await.unwrap();
    assert_page(&first, None, 2);
    let first_again = recipes.list_retained_page(None, 2).await.unwrap();
    assert_eq!(first.wire_value(), first_again.wire_value());
    let mut actual = first.recipes;
    let mut cursor = first.next_cursor;
    drop(recipes);
    drop(store);
    while let Some(previous) = cursor {
        let store = SqliteStore::open_existing(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let page = recipes
            .list_retained_page(Some(&previous), 2)
            .await
            .unwrap();
        assert_page(&page, Some(&previous), 2);
        actual.extend(page.recipes);
        cursor = page.next_cursor;
    }
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 7);
}

#[tokio::test]
async fn invalid_foreign_noncanonical_and_oversized_cursors_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    save_remote(&store, "alice", "a").await;
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let cursor = recipes
        .list_retained_page(None, 1)
        .await
        .unwrap()
        .next_cursor
        .unwrap();
    for limit in [0, 51, u32::MAX] {
        assert!(matches!(
            recipes.list_retained_page(None, limit).await,
            Err(LocalRecipeError::InvalidPage)
        ));
    }
    for bad in [
        String::new(),
        "f".into(),
        "g0".into(),
        "00".repeat(2049),
        cursor.to_uppercase(),
    ] {
        assert!(matches!(
            recipes.list_retained_page(Some(&bad), 1).await,
            Err(LocalRecipeError::InvalidPage)
        ));
    }
    for key in ["version", "scope_digest", "after_id"] {
        let mut decoded: Value = serde_json::from_slice(&hex::decode(&cursor).unwrap()).unwrap();
        decoded[key] = match key {
            "version" => json!(2),
            "scope_digest" => json!("0".repeat(64)),
            _ => json!("\n"),
        };
        let altered = hex::encode(serde_json::to_vec(&decoded).unwrap());
        assert!(matches!(
            recipes.list_retained_page(Some(&altered), 1).await,
            Err(LocalRecipeError::InvalidPage)
        ));
    }
    let bob = LocalRecipeStore::new(&store, scope("bob")).unwrap();
    assert!(matches!(
        bob.list_retained_page(Some(&cursor), 1).await,
        Err(LocalRecipeError::InvalidPage)
    ));
    let foreign = LocalRecipeStore::new(
        &store,
        StoreScope {
            realm: "other-realm".into(),
            owner: "alice".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        foreign.list_retained_page(Some(&cursor), 1).await,
        Err(LocalRecipeError::InvalidPage)
    ));
    let decoded = hex::decode(&cursor).unwrap();
    let padded = hex::encode([b" ".as_slice(), decoded.as_slice()].concat());
    assert!(matches!(
        recipes.list_retained_page(Some(&padded), 1).await,
        Err(LocalRecipeError::InvalidPage)
    ));
}

#[tokio::test]
async fn unrelated_and_invalid_recipe_rows_advance_without_raw_content_disclosure() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    for n in 0..205 {
        store
            .admit(AdmissionRequest {
                key: StoreKey {
                    realm: "page-realm".into(),
                    owner: "alice".into(),
                    id: format!("a-other-{n:03}"),
                },
                admission_key: format!("row-{n}"),
                data: RecordData {
                    schema: "another-schema".into(),
                    version: 1,
                    body: json!({}),
                },
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
    }
    store
        .admit(AdmissionRequest {
            key: StoreKey {
                realm: "page-realm".into(),
                owner: "alice".into(),
                id: "local-recipe/PRIVATE_CORRUPT_KEY".into(),
            },
            admission_key: "bad-recipe".into(),
            data: RecordData {
                schema: LOCAL_RECIPE_SCHEMA.into(),
                version: 1,
                body: json!({"secret": "PRIVATE_CORRUPT_BODY"}),
            },
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
    save_remote(&store, "alice", "selected").await;
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let first = recipes.list_retained_page(None, 50).await.unwrap();
    assert_page(&first, None, 50);
    assert_eq!(first.scanned_records, 200);
    assert!(first.recipes.is_empty());
    assert!(first.unverified.is_empty());
    assert!(
        !serde_json::to_string(&first.wire_value())
            .unwrap()
            .contains("PRIVATE_")
    );
    let cursor = first.next_cursor.unwrap();
    let last = recipes.list_retained_page(Some(&cursor), 50).await.unwrap();
    assert_page(&last, Some(&cursor), 50);
    assert_eq!(last.recipes.len(), 1);
    assert_eq!(last.unverified.len(), 1);
    assert!(
        !serde_json::to_string(&last.wire_value())
            .unwrap()
            .contains("PRIVATE_")
    );
    assert_eq!(last.scanned_records, 7);
    assert!(last.next_cursor.is_none());
}

#[tokio::test]
async fn byte_limited_pages_do_not_lose_deferred_rows_and_oversized_summaries_do_not_stall() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    for n in 0..40 {
        save_selection(&store, &format!("item-{n:02}"), 90).await;
    }
    let oversized = save_selection(&store, "item-40", 120).await;
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let all = recipes.list_retained().await.unwrap();
    let all_summaries: Vec<_> = all.verified.iter().map(|r| r.summary().unwrap()).collect();
    assert!(serde_json::to_vec(&all_summaries).unwrap().len() > 2 * 1024 * 1024);
    let expected: Vec<_> = all_summaries
        .iter()
        .filter(|s| s["identity"]["name"] != "item-40")
        .cloned()
        .collect();
    let mut actual = vec![];
    let mut cursor = None;
    let mut oversized_count = 0;
    let mut pages = 0;
    loop {
        let page = recipes
            .list_retained_page(cursor.as_deref(), 50)
            .await
            .unwrap();
        assert_page(&page, cursor.as_deref(), 50);
        actual.extend(page.recipes);
        oversized_count += page.oversized.len();
        for row in page.oversized {
            assert_eq!(&row.identity, &oversized.package().identity);
            assert_eq!(&row.digest, oversized.digest());
        }
        pages += 1;
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(pages < 20);
    }
    assert_eq!(actual, expected);
    assert_eq!(oversized_count, 1);
    assert!(pages > 2);
    assert!(
        recipes
            .load_selection_exact(&oversized.package().identity, oversized.digest())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn malformed_storage_envelope_remains_an_error_not_a_successful_empty_page() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("recipes.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    save_remote(&store, "alice", "selected").await;
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE toolkit_records SET data='{' WHERE owner='alice'",
            [],
        )
        .unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    assert!(matches!(
        recipes.list_retained_page(None, 20).await,
        Err(LocalRecipeError::Store(_))
    ));
}

#[tokio::test]
async fn longer_skipped_record_cursor_defers_without_losing_the_preceding_page() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    for n in 0..4 {
        let mut package: IntegrationPackage = serde_json::from_slice(PACKAGE).unwrap();
        package.identity.name = Id::new(format!("sized-{n}")).unwrap();
        package.display_name = "x".into();
        let mut selection: ActivationManifest = serde_json::from_slice(SELECTION).unwrap();
        selection.package = package.identity.clone();
        selection.package_content_digest = package.content_digest().unwrap();
        let initial = RetainedLocalSelection::parse(
            &serde_json::to_vec(&package).unwrap(),
            &serde_json::to_vec(&selection).unwrap(),
            LocalSelectionProfile::Python,
            Id::new("python-server").unwrap(),
            evidence("decision"),
            vec![evidence("observed")],
        )
        .unwrap();
        let size = serde_json::to_vec(&RetainedLocalRecipe::Selection(initial).summary().unwrap())
            .unwrap()
            .len();
        package.display_name = "x".repeat(65_200 - size + 1);
        selection.package_content_digest = package.content_digest().unwrap();
        recipes
            .save_selection(
                &serde_json::to_vec(&package).unwrap(),
                &serde_json::to_vec(&selection).unwrap(),
                LocalSelectionProfile::Python,
                Id::new("python-server").unwrap(),
                evidence("decision"),
                vec![evidence("observed")],
            )
            .await
            .unwrap();
    }
    store
        .admit(AdmissionRequest {
            key: StoreKey {
                realm: "page-realm".into(),
                owner: "alice".into(),
                id: format!("z{}", "\"".repeat(511)),
            },
            admission_key: "long-skipped-key".into(),
            data: RecordData {
                schema: "unrelated".into(),
                version: 1,
                body: json!({}),
            },
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
    let page = recipes.list_retained_page(None, 50).await.unwrap();
    assert_page(&page, None, 50);
    assert_eq!(page.recipes.len(), 4);
    assert_eq!(page.scanned_records, 5);
    let cursor = page.next_cursor.unwrap();
    let next = recipes.list_retained_page(Some(&cursor), 50).await.unwrap();
    assert_page(&next, Some(&cursor), 50);
    assert!(next.recipes.is_empty());
    assert_eq!(next.scanned_records, 1);
    assert!(next.next_cursor.is_none());
}
