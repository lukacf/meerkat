#![cfg(feature = "local-recipes")]

use async_trait::async_trait;
use std::path::Path;
use toolkit_package::*;
use toolkit_store::*;

const REMOTE_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json");
const REMOTE_MCP: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/mcp.json");
const REMOTE_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json");
const GUIDANCE_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/google-drive-api-setup/0.1.0/plugin.json");
const GUIDANCE_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/google-drive-api-setup/0.1.0/recipes.json");

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn scope(owner: &str) -> StoreScope {
    StoreScope {
        realm: "host-realm".into(),
        owner: owner.into(),
    }
}

fn decision(record: &str) -> EvidenceReference {
    EvidenceReference {
        owner: id("setup-agent-decisions"),
        record: id(record),
        revision: Revision::new("1").unwrap(),
    }
}

/// An agent-investigated package outside the curated catalog: the retained
/// documents under a local publisher identity.
fn out_of_catalog(plugin: &[u8], name: &str) -> Vec<u8> {
    out_of_catalog_as(plugin, "local-research", name)
}

fn out_of_catalog_as(plugin: &[u8], publisher: &str, name: &str) -> Vec<u8> {
    let mut value: serde_json::Value = serde_json::from_slice(plugin).unwrap();
    let identity = &mut value["extensions"]["ai.rkat.toolkit"]["identity"];
    identity["publisher"] = serde_json::json!(publisher);
    identity["name"] = serde_json::json!(name);
    serde_json::to_vec_pretty(&value).unwrap()
}

fn identity_of(
    plugin: &[u8],
    mcp: Option<&[u8]>,
    recipes: &[u8],
) -> (PackageIdentity, ContentDigest) {
    let bundle = IntegrationBundle::parse(plugin, mcp, recipes).unwrap();
    (bundle.package().identity.clone(), bundle.digest().clone())
}

#[tokio::test]
async fn an_out_of_catalog_recipe_survives_reopen_with_exact_bytes_and_local_origin() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("toolkit.sqlite");
    let plugin = out_of_catalog(REMOTE_PLUGIN, "acme-tracker");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    let curated = bundled_integration_library().unwrap();
    assert!(curated.entries().all(|b| b.package().identity != identity));
    {
        let store = SqliteStore::open(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let saved = recipes
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice"),
            )
            .await
            .unwrap();
        assert_eq!(saved.bundle().digest(), &digest);
    }
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let loaded = recipes
        .load_exact(&identity, &digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.plugin_json(), plugin.as_slice());
    assert_eq!(loaded.mcp_json(), Some(REMOTE_MCP));
    assert_eq!(loaded.recipes_json(), REMOTE_RECIPES);
    let fresh = IntegrationBundle::parse(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES).unwrap();
    assert_eq!(loaded.bundle().digest(), fresh.digest());
    assert_eq!(loaded.bundle().package(), fresh.package());
    assert_eq!(loaded.bundle().recipes(), fresh.recipes());
    assert_eq!(loaded.decision(), &decision("acme-choice"));
    assert_eq!(
        loaded.bundle().local_decision(),
        Some(&decision("acme-choice"))
    );
    assert_eq!(
        loaded.origin(),
        CatalogOrigin::LocalInvestigation {
            decision: decision("acme-choice")
        }
    );
    // Reuse is not curated support.
    assert!(
        loaded
            .bundle()
            .package()
            .assessments
            .iter()
            .all(|a| a.status == SupportStatus::Experimental)
    );
    let listing = recipes.list().await.unwrap();
    assert_eq!(listing.verified.len(), 1);
    assert!(listing.unverified.is_empty());
}

#[tokio::test]
async fn replay_is_idempotent_and_other_content_under_the_same_identity_refuses() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("toolkit.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let plugin = out_of_catalog(REMOTE_PLUGIN, "acme-tracker");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    for _ in 0..2 {
        recipes
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice"),
            )
            .await
            .unwrap();
    }
    let key = StoreKey {
        realm: "host-realm".into(),
        owner: "alice".into(),
        id: format!(
            "local-recipe/v1/{}:{}{}:{}{}:{}",
            identity.publisher.as_str().len(),
            identity.publisher,
            identity.name.as_str().len(),
            identity.name,
            identity.version.as_str().len(),
            identity.version
        ),
    };
    assert_eq!(store.get(&key).await.unwrap().unwrap().revision, 1);
    let mut changed: serde_json::Value = serde_json::from_slice(REMOTE_RECIPES).unwrap();
    changed["recipes"][0]["prerequisites"][0]["instruction"] =
        serde_json::json!("A different account is required");
    let changed = serde_json::to_vec(&changed).unwrap();
    assert!(matches!(
        recipes
            .save(&plugin, Some(REMOTE_MCP), &changed, decision("acme-choice"))
            .await,
        Err(LocalRecipeError::ImmutableConflict)
    ));
    assert!(matches!(
        recipes
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("another-choice")
            )
            .await,
        Err(LocalRecipeError::ImmutableConflict)
    ));
    let loaded = recipes
        .load_exact(&identity, &digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.recipes_json(), REMOTE_RECIPES);
    assert_eq!(loaded.decision(), &decision("acme-choice"));
}

fn tamper(path: &Path, edit: impl FnOnce(&mut serde_json::Value), recompute_record_digest: bool) {
    let connection = rusqlite::Connection::open(path).unwrap();
    let (id, data): (String, String) = connection
        .query_row("SELECT id,data FROM toolkit_records", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let mut record: serde_json::Value = serde_json::from_str(&data).unwrap();
    edit(&mut record["body"]);
    let digest: String = if recompute_record_digest {
        serde_json::from_value::<RecordData>(record.clone())
            .unwrap()
            .digest()
            .unwrap()
    } else {
        connection
            .query_row(
                "SELECT digest FROM toolkit_records WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap()
    };
    connection
        .execute(
            "UPDATE toolkit_records SET data=?1, digest=?2 WHERE id=?3",
            rusqlite::params![record.to_string(), digest, id],
        )
        .unwrap();
}

#[tokio::test]
async fn original_save_replay_refuses_a_self_consistent_changed_decision() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("toolkit.sqlite");
    let plugin = out_of_catalog(REMOTE_PLUGIN, "acme-tracker");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    {
        let store = SqliteStore::open(&path).unwrap();
        LocalRecipeStore::new(&store, scope("alice"))
            .unwrap()
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice"),
            )
            .await
            .unwrap();
    }
    tamper(
        &path,
        |body| body["decision"] = serde_json::to_value(decision("substituted-choice")).unwrap(),
        true,
    );
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    // The bundle digest is unchanged. Only equality with the submitted save
    // can detect that its decision evidence was replaced in the current row.
    let before = store.list_records(&scope("alice"), None, 10).await.unwrap();
    assert_eq!(
        recipes
            .load_exact(&identity, &digest)
            .await
            .unwrap()
            .unwrap()
            .decision(),
        &decision("substituted-choice")
    );
    assert!(matches!(
        recipes
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice"),
            )
            .await,
        Err(LocalRecipeError::Unverified(_))
    ));
    assert_eq!(
        store.list_records(&scope("alice"), None, 10).await.unwrap(),
        before
    );
}

#[tokio::test]
async fn original_save_replay_refuses_a_self_consistent_changed_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("toolkit.sqlite");
    let plugin = out_of_catalog(REMOTE_PLUGIN, "acme-tracker");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    {
        let store = SqliteStore::open(&path).unwrap();
        LocalRecipeStore::new(&store, scope("alice"))
            .unwrap()
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice"),
            )
            .await
            .unwrap();
    }
    let mut changed: serde_json::Value = serde_json::from_slice(REMOTE_RECIPES).unwrap();
    changed["recipes"][0]["prerequisites"][0]["instruction"] =
        serde_json::json!("A substituted account prerequisite");
    let changed = serde_json::to_string(&changed).unwrap();
    let (changed_identity, changed_digest) =
        identity_of(&plugin, Some(REMOTE_MCP), changed.as_bytes());
    assert_eq!(changed_identity, identity);
    assert_ne!(changed_digest, digest);
    tamper(
        &path,
        |body| {
            body["recipes_json"] = changed.clone().into();
            body["bundle_digest"] = serde_json::to_value(&changed_digest).unwrap();
        },
        true,
    );
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let before = store.list_records(&scope("alice"), None, 10).await.unwrap();
    assert_eq!(
        recipes
            .load_exact(&identity, &changed_digest)
            .await
            .unwrap()
            .unwrap()
            .recipes_json(),
        changed.as_bytes()
    );
    assert!(matches!(
        recipes
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice"),
            )
            .await,
        Err(LocalRecipeError::Unverified(_))
    ));
    assert_eq!(
        store.list_records(&scope("alice"), None, 10).await.unwrap(),
        before
    );
}

#[tokio::test]
async fn duplicate_library_bundle_cannot_discard_or_replace_local_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("toolkit.sqlite")).unwrap();
    let plugin = out_of_catalog(REMOTE_PLUGIN, "acme-tracker");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    let alice = LocalRecipeStore::new(&store, scope("alice"))
        .unwrap()
        .save(
            &plugin,
            Some(REMOTE_MCP),
            REMOTE_RECIPES,
            decision("alice-choice"),
        )
        .await
        .unwrap();
    let bob = LocalRecipeStore::new(&store, scope("bob"))
        .unwrap()
        .save(
            &plugin,
            Some(REMOTE_MCP),
            REMOTE_RECIPES,
            decision("bob-choice"),
        )
        .await
        .unwrap();
    assert_eq!(alice.bundle().digest(), bob.bundle().digest());

    let generic_origin = CatalogOrigin::LocalInvestigation {
        decision: decision("generic-library-choice"),
    };
    let mut raw_first = IntegrationLibrary::new(generic_origin.clone());
    raw_first
        .insert(IntegrationBundle::parse(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES).unwrap())
        .unwrap();
    let raw_catalog_before = raw_first.catalog().get(&identity).unwrap().clone();
    let raw_bundle_before =
        serde_json::to_value(raw_first.get(&identity, &digest).unwrap()).unwrap();
    assert!(matches!(
        raw_first.insert(alice.bundle().clone()),
        Err(IntegrationLibraryError::ProvenanceConflict)
    ));
    assert_eq!(
        raw_first.catalog().get(&identity),
        Some(&raw_catalog_before)
    );
    assert_eq!(
        serde_json::to_value(raw_first.get(&identity, &digest).unwrap()).unwrap(),
        raw_bundle_before
    );
    assert!(
        raw_first
            .get(&identity, &digest)
            .unwrap()
            .local_decision()
            .is_none()
    );

    let mut retained_first = IntegrationLibrary::new(generic_origin);
    retained_first.insert(alice.bundle().clone()).unwrap();
    // An exact replay remains valid; the conflicting decision is the difference.
    retained_first.insert(alice.bundle().clone()).unwrap();
    let retained_catalog_before = retained_first.catalog().get(&identity).unwrap().clone();
    let retained_bundle_before =
        serde_json::to_value(retained_first.get(&identity, &digest).unwrap()).unwrap();
    assert!(matches!(
        retained_first.insert(bob.bundle().clone()),
        Err(IntegrationLibraryError::ProvenanceConflict)
    ));
    assert_eq!(
        retained_first.catalog().get(&identity),
        Some(&retained_catalog_before)
    );
    assert_eq!(
        serde_json::to_value(retained_first.get(&identity, &digest).unwrap()).unwrap(),
        retained_bundle_before
    );
    assert_eq!(
        retained_first
            .get(&identity, &digest)
            .unwrap()
            .local_decision(),
        Some(&decision("alice-choice"))
    );
}

#[tokio::test]
async fn malformed_or_edited_content_never_returns_a_verified_entry() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("toolkit.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let plugin = out_of_catalog(REMOTE_PLUGIN, "acme-tracker");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    let skipped_entry = serde_json::to_vec(&serde_json::json!({
        "$schema": AGENT_PLUGIN_MCP_SCHEMA,
        "mcpServers": { "bad": { "type": "stdio", "command": "node index.js" } }
    }))
    .unwrap();
    for (mcp, recipes_json) in [
        (Some(REMOTE_MCP), b"{ not json".as_slice()),
        (Some(skipped_entry.as_slice()), REMOTE_RECIPES),
        (None, REMOTE_RECIPES),
    ] {
        assert!(matches!(
            recipes
                .save(&plugin, mcp, recipes_json, decision("acme-choice"))
                .await,
            Err(LocalRecipeError::Invalid(_))
        ));
    }
    assert!(recipes.list().await.unwrap().verified.is_empty());
    assert!(recipes.load(&identity).await.unwrap().is_none());
    recipes
        .save(
            &plugin,
            Some(REMOTE_MCP),
            REMOTE_RECIPES,
            decision("acme-choice"),
        )
        .await
        .unwrap();
    drop(recipes);
    drop(store);

    let edited_recipes = {
        let mut value: serde_json::Value = serde_json::from_slice(REMOTE_RECIPES).unwrap();
        value["recipes"][0]["prerequisites"][0]["instruction"] =
            serde_json::json!("Paste a personal access token here");
        serde_json::to_string(&value).unwrap()
    };
    // Edited bytes behind the store's record digest.
    let text = edited_recipes.clone();
    tamper(&path, move |body| body["recipes_json"] = text.into(), false);
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    assert!(matches!(
        recipes.load(&identity).await,
        Err(LocalRecipeError::Unverified(
            "record digest does not match its content"
        ))
    ));
    let listing = recipes.list().await.unwrap();
    assert!(listing.verified.is_empty());
    assert_eq!(listing.unverified.len(), 1);
    drop(recipes);
    drop(store);

    // A consistent record digest still leaves the recorded bundle digest.
    let text = edited_recipes.clone();
    tamper(&path, move |body| body["recipes_json"] = text.into(), true);
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    assert!(matches!(
        recipes.load(&identity).await,
        Err(LocalRecipeError::Unverified(
            "saved documents do not match their recorded identity or digest"
        ))
    ));
    drop(recipes);
    drop(store);

    // Fully self-consistent forgery parses as a different bundle; the exact
    // selection by digest refuses it.
    let forged = IntegrationBundle::parse(&plugin, Some(REMOTE_MCP), edited_recipes.as_bytes())
        .unwrap()
        .digest()
        .clone();
    tamper(
        &path,
        move |body| body["bundle_digest"] = serde_json::to_value(&forged).unwrap(),
        true,
    );
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    assert!(matches!(
        recipes.load_exact(&identity, &digest).await,
        Err(LocalRecipeError::Unverified(
            "stored recipe differs from the selected digest"
        ))
    ));
}

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    BeforeCommit,
    AfterCommit,
    Volatile,
}

/// Delegates to SQLite and injects one admission or capability fault.
struct FaultyStore {
    inner: SqliteStore,
    fault: Fault,
}

#[async_trait]
impl ToolkitStore for FaultyStore {
    fn capabilities(&self) -> StoreCapabilities {
        let mut capabilities = self.inner.capabilities();
        if self.fault == Fault::Volatile {
            capabilities.durability = Durability::Volatile;
        }
        capabilities
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        match self.fault {
            Fault::BeforeCommit => Err(StoreError::Synchronization),
            Fault::AfterCommit => {
                self.inner.admit(request).await?;
                Err(StoreError::Synchronization)
            }
            Fault::Volatile => self.inner.admit(request).await,
        }
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        self.inner.get(key).await
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after_id: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after_id, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        self.inner.compare_and_swap(transition).await
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        expected_revision: u64,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<RecordLease> {
        self.inner
            .claim_record(key, expected_revision, worker, ttl_ms)
            .await
    }
    async fn renew_record_claim(
        &self,
        lease: &RecordLease,
        ttl_ms: u64,
    ) -> StoreResult<RecordLease> {
        self.inner.renew_record_claim(lease, ttl_ms).await
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(prior).await
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        self.inner.release_record_claim(lease).await
    }
    async fn read_events(
        &self,
        key: &StoreKey,
        after_sequence: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after_sequence, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(
        &self,
        key: &StoreKey,
        after_id: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        self.inner.list_effects(key, after_id, limit).await
    }
    async fn claim_effect(
        &self,
        key: &EffectKey,
        expected_record_revision: u64,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        self.inner
            .claim_effect(key, expected_record_revision, worker, ttl_ms)
            .await
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        self.inner.acknowledge_effect(lease, outcome).await
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl_ms: u64,
    ) -> StoreResult<StoredEffect> {
        self.inner.claim_reconciliation(key, worker, ttl_ms).await
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        self.inner.reconcile_effect(lease, resolution).await
    }
}

#[tokio::test]
async fn interrupted_or_unconfirmed_saves_are_never_reported_as_saved() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("toolkit.sqlite");
    let plugin = out_of_catalog(REMOTE_PLUGIN, "acme-tracker");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    let volatile = FaultyStore {
        inner: SqliteStore::open(&path).unwrap(),
        fault: Fault::Volatile,
    };
    assert!(matches!(
        LocalRecipeStore::new(&volatile, scope("alice")),
        Err(LocalRecipeError::StoreNotDurable)
    ));
    drop(volatile);

    let interrupted = FaultyStore {
        inner: SqliteStore::open(&path).unwrap(),
        fault: Fault::BeforeCommit,
    };
    let recipes = LocalRecipeStore::new(&interrupted, scope("alice")).unwrap();
    assert!(matches!(
        recipes
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice")
            )
            .await,
        Err(LocalRecipeError::Unconfirmed(_))
    ));
    assert!(recipes.load(&identity).await.unwrap().is_none());
    drop(recipes);
    drop(interrupted);

    let unconfirmed = FaultyStore {
        inner: SqliteStore::open(&path).unwrap(),
        fault: Fault::AfterCommit,
    };
    let recipes = LocalRecipeStore::new(&unconfirmed, scope("alice")).unwrap();
    assert!(matches!(
        recipes
            .save(
                &plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice")
            )
            .await,
        Err(LocalRecipeError::Unconfirmed(_))
    ));
    drop(recipes);
    drop(unconfirmed);
    // Whatever committed is only reused after the same full verification.
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let loaded = recipes
        .load_exact(&identity, &digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.recipes_json(), REMOTE_RECIPES);
}

#[tokio::test]
async fn namespaces_are_isolated_and_local_bundles_are_not_promoted() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("toolkit.sqlite")).unwrap();
    let alice = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let bob = LocalRecipeStore::new(&store, scope("bob")).unwrap();
    let plugin = out_of_catalog(REMOTE_PLUGIN, "acme-tracker");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    let saved = alice
        .save(
            &plugin,
            Some(REMOTE_MCP),
            REMOTE_RECIPES,
            decision("acme-choice"),
        )
        .await
        .unwrap();
    assert!(bob.load(&identity).await.unwrap().is_none());
    assert!(bob.load_exact(&identity, &digest).await.unwrap().is_none());
    assert!(bob.list().await.unwrap().verified.is_empty());
    // Another namespace makes its own choice under the same identity.
    let mut changed: serde_json::Value = serde_json::from_slice(REMOTE_RECIPES).unwrap();
    changed["recipes"][0]["prerequisites"][0]["instruction"] =
        serde_json::json!("Bob's organization requires an administrator");
    bob.save(
        &plugin,
        Some(REMOTE_MCP),
        &serde_json::to_vec(&changed).unwrap(),
        decision("bob-choice"),
    )
    .await
    .unwrap();
    assert_eq!(
        alice
            .load(&identity)
            .await
            .unwrap()
            .unwrap()
            .bundle()
            .digest(),
        &digest
    );

    let mut curated = bundled_integration_library().unwrap();
    assert!(matches!(
        curated.insert(saved.bundle().clone()),
        Err(IntegrationLibraryError::LocalPromotion)
    ));
    let mut imported = IntegrationLibrary::new(CatalogOrigin::Imported {
        format: id("agent-plugins"),
        source_digest: digest.clone(),
    });
    assert!(matches!(
        imported.insert(saved.bundle().clone()),
        Err(IntegrationLibraryError::LocalPromotion)
    ));
    let mut local = IntegrationLibrary::new(CatalogOrigin::LocalInvestigation {
        decision: decision("host-local-library"),
    });
    local.insert(saved.bundle().clone()).unwrap();
    assert_eq!(
        local.catalog().get(&identity).unwrap().origin,
        saved.origin()
    );
    assert!(local.get(&identity, &digest).is_some());

    // Only the selected documents and decision are stored: no credentials,
    // environment, readiness or setup journal.
    let record = store
        .list_records(&scope("alice"), None, 10)
        .await
        .unwrap()
        .remove(0);
    let mut fields: Vec<_> = record
        .data
        .body
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "bundle_digest",
            "decision",
            "identity",
            "mcp_json",
            "plugin_json",
            "recipes_json"
        ]
    );
    assert!(
        store
            .read_events(&record.key, 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_effects(&record.key, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn reused_guidance_keeps_unresolved_prerequisites_and_never_activates() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("toolkit.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let plugin = out_of_catalog(GUIDANCE_PLUGIN, "acme-drive-guidance");
    let (identity, digest) = identity_of(&plugin, None, GUIDANCE_RECIPES);
    recipes
        .save(&plugin, None, GUIDANCE_RECIPES, decision("drive-choice"))
        .await
        .unwrap();
    let loaded = recipes
        .load_exact(&identity, &digest)
        .await
        .unwrap()
        .unwrap();
    let fresh = IntegrationBundle::parse(&plugin, None, GUIDANCE_RECIPES).unwrap();
    assert!(loaded.mcp_json().is_none());
    assert!(loaded.bundle().servers().is_empty());
    assert_eq!(loaded.bundle().recipes(), fresh.recipes());
    let strategy = &loaded.bundle().package().strategies[0];
    assert!(!strategy.unresolved_details.is_empty());
    assert_eq!(strategy, &fresh.package().strategies[0]);
    for recipe in &loaded.bundle().recipes().recipes {
        assert!(
            matches!(&recipe.plan, RecipePlan::GuidanceOnly { blockers } if !blockers.is_empty())
        );
        assert!(!recipe.prerequisites.is_empty());
    }
    // Any concrete manifest is still refused as guidance; reuse does not
    // install, activate or claim service access.
    let remote = IntegrationBundle::parse(REMOTE_PLUGIN, Some(REMOTE_MCP), REMOTE_RECIPES).unwrap();
    let RecipePlan::RemoteMcp {
        server, publisher, ..
    } = &remote.recipes().recipes[0].plan
    else {
        panic!("remote fixture")
    };
    let PortableMcpServer::StreamableHttp { url, .. } = &remote.servers()[server] else {
        panic!("remote fixture")
    };
    let manifest: ActivationManifest = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "package": identity,
        "package_content_digest": loaded.bundle().package().content_digest().unwrap(),
        "strategy": strategy.id,
        "configuration": { "owner": "config", "reference": "selected", "revision": "1",
            "content_digest": ContentDigest::of_bytes(b"config") },
        "policy": { "owner": "policy", "revision": "1", "content_digest": ContentDigest::of_bytes(b"policy") },
        "components": [{
            "id": "remote", "version": "1",
            "source": { "kind": "remote_service", "endpoint": url, "publisher": publisher,
                "configuration_revision": "1", "observed_contract": ContentDigest::of_bytes(b"tools"),
                "transport": "mcp_streamable_http" },
            "runtime": "remote", "placement": "remote", "lifetime": "externally_managed",
            "activation_scope": { "kind": "independent" },
            "readiness": { "owner": "owner", "checks": ["observe"] },
            "shutdown": { "owner": "owner", "operation": "disconnect", "maximum_wait_ms": 1000,
                "timeout": "preserve_uncertainty" },
            "rollback": { "kind": "owner_reconciliation", "owner": "owner", "operation": "reconcile" }
        }],
        "exports": []
    }))
    .unwrap();
    assert!(matches!(
        loaded.bundle().validate_selection(
            loaded.bundle().digest(),
            manifest,
            &HostCapabilities::default()
        ),
        Err(LibrarySelectionError::GuidanceOnly)
    ));
}

#[tokio::test]
async fn identities_that_join_to_the_same_path_stay_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("toolkit.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    // Joined with '/', both read local-research/acme/tracker/0.1.0.
    let first = out_of_catalog_as(REMOTE_PLUGIN, "local-research/acme", "tracker");
    let second = out_of_catalog_as(REMOTE_PLUGIN, "local-research", "acme/tracker");
    let (first_identity, first_digest) = identity_of(&first, Some(REMOTE_MCP), REMOTE_RECIPES);
    let (second_identity, second_digest) = identity_of(&second, Some(REMOTE_MCP), REMOTE_RECIPES);
    assert_ne!(first_identity, second_identity);
    for plugin in [&first, &second, &first] {
        recipes
            .save(
                plugin,
                Some(REMOTE_MCP),
                REMOTE_RECIPES,
                decision("acme-choice"),
            )
            .await
            .unwrap();
    }
    for (identity, digest, plugin) in [
        (&first_identity, &first_digest, &first),
        (&second_identity, &second_digest, &second),
    ] {
        let loaded = recipes.load_exact(identity, digest).await.unwrap().unwrap();
        assert_eq!(&loaded.bundle().package().identity, identity);
        assert_eq!(loaded.plugin_json(), plugin.as_slice());
    }
    let mut ids: Vec<_> = store
        .list_records(&scope("alice"), None, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|record| (record.key.id, record.revision))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            (
                "local-recipe/v1/14:local-research12:acme/tracker5:0.1.0".to_owned(),
                1
            ),
            (
                "local-recipe/v1/19:local-research/acme7:tracker5:0.1.0".to_owned(),
                1
            ),
        ]
    );
    assert_eq!(recipes.list().await.unwrap().verified.len(), 2);
    drop(recipes);
    drop(store);

    // A record carrying another identity under the requested key is refused.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE toolkit_records SET (data, digest) = \
             (SELECT data, digest FROM toolkit_records WHERE id=?1) WHERE id=?2",
            rusqlite::params![
                "local-recipe/v1/19:local-research/acme7:tracker5:0.1.0",
                "local-recipe/v1/14:local-research12:acme/tracker5:0.1.0"
            ],
        )
        .unwrap();
    drop(connection);
    let store = SqliteStore::open(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    for result in [
        recipes.load(&second_identity).await,
        recipes.load_exact(&second_identity, &second_digest).await,
    ] {
        match result {
            Err(LocalRecipeError::IdentityMismatch { requested, stored }) => {
                assert_eq!(*requested, second_identity);
                assert_eq!(*stored, first_identity);
            }
            other => panic!("cross-identity load was not refused: {other:?}"),
        }
    }
    assert!(
        recipes
            .load_exact(&first_identity, &first_digest)
            .await
            .unwrap()
            .is_some()
    );
    let listing = recipes.list().await.unwrap();
    assert_eq!(listing.verified.len(), 1);
    assert_eq!(listing.unverified.len(), 1);
}

#[tokio::test]
async fn current_submission_is_durable_exact_and_scoped_without_an_external_decision() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("toolkit.sqlite");
    let plugin = out_of_catalog(REMOTE_PLUGIN, "present-choice");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    let reference = {
        let store = SqliteStore::open(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let saved = recipes.save_current_submission(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES)
            .await.unwrap();
        let rows = store.list_records(&scope("alice"), None, 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].data.body["current_submission"], "v1");
        assert!(rows[0].key.id.starts_with("local-recipe/v1/"));
        assert_ne!(saved.decision().record.as_str(), rows[0].key.id);
        saved.decision().clone()
    };
    let store = SqliteStore::open_existing(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let loaded = recipes.load_exact(&identity, &digest).await.unwrap().unwrap();
    assert_eq!(loaded.decision(), &reference);
    assert_eq!(loaded.plugin_json(), plugin);
    assert_eq!(loaded.mcp_json(), Some(REMOTE_MCP));
    assert_eq!(loaded.recipes_json(), REMOTE_RECIPES);
    assert_eq!(recipes.save_current_submission(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES)
        .await.unwrap().decision(), &reference);
    let rows = store.list_records(&scope("alice"), None, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].revision, 1);
    for other in [scope("bob"), StoreScope { realm: "other-realm".into(), owner: "alice".into() },
                  StoreScope { realm: "realm with spaces".into(), owner: "x".repeat(256) }] {
        let recipes = LocalRecipeStore::new(&store, other).unwrap();
        assert!(recipes.load(&identity).await.unwrap().is_none());
        let saved = recipes.save_current_submission(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES)
            .await.unwrap();
        assert_ne!(saved.decision().record, reference.record);
        assert_eq!(saved.decision().revision, reference.revision);
        assert!(saved.decision().record.as_str().len() <= 192);
    }
    // Exact bytes, not only parsed package meaning, bind the submission.
    let mut changed = plugin.clone();
    changed.push(b'\n');
    assert!(matches!(recipes.save_current_submission(&changed, Some(REMOTE_MCP), REMOTE_RECIPES)
        .await, Err(LocalRecipeError::ImmutableConflict)));
    let independent = SqliteStore::open(temp.path().join("independent.sqlite")).unwrap();
    let changed = LocalRecipeStore::new(&independent, scope("alice")).unwrap()
        .save_current_submission(&changed, Some(REMOTE_MCP), REMOTE_RECIPES).await.unwrap();
    assert_eq!(changed.decision().record, reference.record);
    assert_ne!(changed.decision().revision, reference.revision);
}

#[tokio::test]
async fn current_submission_reopen_checks_marked_reference_and_exact_documents() {
    for edit in ["owner", "record", "revision", "documents", "marker_version", "marker_shape", "marker_null"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("toolkit.sqlite");
        let plugin = out_of_catalog(REMOTE_PLUGIN, "present-choice");
        let (identity, _) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
        {
            let store = SqliteStore::open(&path).unwrap();
            LocalRecipeStore::new(&store, scope("alice")).unwrap()
                .save_current_submission(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES).await.unwrap();
        }
        tamper(&path, |body| match edit {
            "marker_version" => body["current_submission"] = serde_json::json!("v2"),
            "marker_shape" => body["current_submission"] = serde_json::json!({"v1": null}),
            "marker_null" => body["current_submission"] = serde_json::Value::Null,
            "documents" => {
                let changed = format!("{}\n", body["plugin_json"].as_str().unwrap());
                let (_, changed_digest) = identity_of(changed.as_bytes(), Some(REMOTE_MCP), REMOTE_RECIPES);
                body["plugin_json"] = serde_json::json!(changed);
                body["bundle_digest"] = serde_json::json!(changed_digest);
            }
            field => body["decision"][field] = serde_json::json!("substituted"),
        }, true);
        let store = SqliteStore::open_existing(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let before = store.list_records(&scope("alice"), None, 10).await.unwrap();
        assert!(matches!(recipes.load(&identity).await, Err(LocalRecipeError::Unverified(_))), "{edit}");
        let listing = recipes.list().await.unwrap();
        assert!(listing.verified.is_empty());
        assert_eq!(listing.unverified.len(), 1);
        assert!(matches!(recipes.save_current_submission(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES)
            .await, Err(LocalRecipeError::Unverified(_)) | Err(LocalRecipeError::ImmutableConflict)), "{edit}");
        assert_eq!(store.list_records(&scope("alice"), None, 10).await.unwrap(), before);
    }
}

#[tokio::test]
async fn copied_current_submission_cannot_change_its_store_scope() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("toolkit.sqlite")).unwrap();
    let plugin = out_of_catalog(REMOTE_PLUGIN, "present-choice");
    let (identity, _) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    LocalRecipeStore::new(&store, scope("alice")).unwrap()
        .save_current_submission(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES).await.unwrap();
    let original = store.list_records(&scope("alice"), None, 10).await.unwrap().remove(0);
    for destination in [scope("bob"), StoreScope { realm: "other-realm".into(), owner: "alice".into() }] {
        let key = StoreKey { realm: destination.realm.clone(), owner: destination.owner.clone(), id: original.key.id.clone() };
        store.admit(AdmissionRequest { admission_key: key.id.clone(), key,
            data: original.data.clone(), events: vec![], effects: vec![] }).await.unwrap();
        assert!(matches!(LocalRecipeStore::new(&store, destination).unwrap().load(&identity).await,
            Err(LocalRecipeError::Unverified(_))));
    }
}

#[tokio::test]
async fn external_reference_resembling_submission_owner_remains_an_unmarked_legacy_claim() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("toolkit.sqlite");
    let plugin = out_of_catalog(REMOTE_PLUGIN, "historical-choice");
    let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
    let external = EvidenceReference { owner: id("toolkit.local-recipes.current-submission"),
        record: id("submission/external-history"), revision: Revision::new("old external revision").unwrap() };
    {
        let store = SqliteStore::open(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        recipes.save(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES, external.clone()).await.unwrap();
        let rows = store.list_records(&scope("alice"), None, 10).await.unwrap();
        assert_eq!(rows[0].data.version, 1);
        assert!(rows[0].data.body.get("current_submission").is_none());
        assert_eq!(rows[0].data.body.as_object().unwrap().len(), 6);
    }
    let store = SqliteStore::open_existing(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    assert_eq!(recipes.load_exact(&identity, &digest).await.unwrap().unwrap().decision(), &external);
    assert_eq!(recipes.save(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES, external.clone())
        .await.unwrap().decision(), &external);
    assert!(matches!(recipes.save_current_submission(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES)
        .await, Err(LocalRecipeError::ImmutableConflict)));
}

#[tokio::test]
async fn unconfirmed_current_submission_is_observed_without_retrying_admission() {
    for fault in [Fault::BeforeCommit, Fault::AfterCommit] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("toolkit.sqlite");
        let plugin = out_of_catalog(REMOTE_PLUGIN, "present-choice");
        let (identity, digest) = identity_of(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES);
        {
            let faulty = FaultyStore { inner: SqliteStore::open(&path).unwrap(), fault };
            let recipes = LocalRecipeStore::new(&faulty, scope("alice")).unwrap();
            assert!(matches!(recipes.save_current_submission(&plugin, Some(REMOTE_MCP), REMOTE_RECIPES)
                .await, Err(LocalRecipeError::Unconfirmed(_))));
        }
        let store = SqliteStore::open_existing(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let observed = recipes.load_exact(&identity, &digest).await.unwrap();
        assert_eq!(observed.is_some(), fault == Fault::AfterCommit);
        let rows = store.list_records(&scope("alice"), None, 10).await.unwrap();
        assert_eq!(rows.len(), usize::from(fault == Fault::AfterCommit));
        if let Some(recipe) = observed {
            assert_eq!(recipe.plugin_json(), plugin);
            assert_eq!(rows[0].revision, 1);
        }
    }
}
