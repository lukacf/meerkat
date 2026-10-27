#![cfg(feature = "local-recipes")]

use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::Path;
use toolkit_package::*;
use toolkit_store::*;

const NODE_PACKAGE: &[u8] = include_bytes!("fixtures/node-install/package.json");
const NODE_SELECTION: &[u8] = include_bytes!("fixtures/node-install/activation.json");
const REMOTE_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json");
const REMOTE_MCP: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/mcp.json");
const REMOTE_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json");
const CONFIG: &[u8] = b"{ \"toolsets\": \"navigation\" }\n";

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn scope(owner: &str) -> StoreScope {
    StoreScope {
        realm: "household".into(),
        owner: owner.into(),
    }
}
fn reference(record: &str) -> EvidenceReference {
    EvidenceReference {
        owner: id("historical"),
        record: id(record),
        revision: Revision::new("1").unwrap(),
    }
}
fn verification() -> Vec<EvidenceReference> {
    vec![reference("install"), reference("mcp")]
}
fn selection(config: &[u8]) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(NODE_SELECTION).unwrap();
    value["configuration"]["content_digest"] = json!(ContentDigest::of_bytes(config));
    value["components"][0]["source"]["dependencies"] = json!({"kind":"self_contained"});
    value["components"][0]["source"]["entrypoint"] =
        json!({"executable":"server.mjs","arguments":["--toolsets=navigation"]});
    serde_json::to_vec_pretty(&value).unwrap()
}
fn parse(config: &[u8]) -> Result<RetainedLocalSelection, LocalRecipeError> {
    RetainedLocalSelection::parse_mcpb(
        NODE_PACKAGE,
        &selection(config),
        config,
        id("node-server"),
        reference("decision"),
        verification(),
    )
}
fn change_record(path: &Path, change: impl FnOnce(&mut RecordData)) {
    let db = rusqlite::Connection::open(path).unwrap();
    let raw: String = db
        .query_row("SELECT data FROM toolkit_records", [], |r| r.get(0))
        .unwrap();
    let mut data: RecordData = serde_json::from_str(&raw).unwrap();
    change(&mut data);
    db.execute(
        "UPDATE toolkit_records SET data=?1,digest=?2",
        rusqlite::params![
            serde_json::to_string(&data).unwrap(),
            data.digest().unwrap()
        ],
    )
    .unwrap();
}

#[tokio::test]
async fn mcpb_exact_documents_reopen_replay_and_scope_without_archive_access() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("recipes.sqlite");
    let selected = selection(CONFIG);
    let expected = parse(CONFIG).unwrap();
    {
        let store = SqliteStore::open(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let saved = recipes
            .save_mcpb_selection(
                NODE_PACKAGE,
                &selected,
                CONFIG,
                id("node-server"),
                reference("decision"),
                verification(),
            )
            .await
            .unwrap();
        assert_eq!(saved.digest(), expected.digest());
        let rows = store.list_records(&scope("alice"), None, 10).await.unwrap();
        assert_eq!(rows[0].data.version, 3);
        assert_eq!(
            rows[0].data.body["configuration_json"],
            json!(String::from_utf8_lossy(CONFIG))
        );
        assert_eq!(saved.profile(), LocalSelectionProfile::Mcpb);
        assert_eq!(
            saved.configuration_digest(),
            Some(&ContentDigest::of_bytes(CONFIG))
        );
        assert!(
            store
                .read_events(&rows[0].key, 0, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .list_effects(&rows[0].key, None, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
    let store = SqliteStore::open_existing(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let saved = recipes
        .load_selection_exact(&expected.package().identity, expected.digest())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.package_json(), NODE_PACKAGE);
    assert_eq!(saved.selection_json(), selected);
    assert_eq!(saved.configuration_json(), Some(CONFIG));
    assert_eq!(saved.verification(), verification());
    assert_eq!(saved.decision(), &reference("decision"));
    let before = store.list_records(&scope("alice"), None, 10).await.unwrap();
    recipes
        .save_mcpb_selection(
            NODE_PACKAGE,
            &selected,
            CONFIG,
            id("node-server"),
            reference("decision"),
            verification(),
        )
        .await
        .unwrap();
    assert_eq!(
        before,
        store.list_records(&scope("alice"), None, 10).await.unwrap()
    );
    assert!(matches!(
        recipes
            .load_retained_exact(&expected.package().identity, expected.digest())
            .await
            .unwrap(),
        Some(RetainedLocalRecipe::Selection(_))
    ));
    assert_eq!(recipes.list_retained().await.unwrap().verified.len(), 1);
    assert_eq!(recipes.list().await.unwrap().unverified.len(), 1);
    assert!(
        LocalRecipeStore::new(&store, scope("bob"))
            .unwrap()
            .load_retained_exact(&expected.package().identity, expected.digest())
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn mcpb_configuration_grammar_and_digest_are_inert_preflight() {
    for config in [
        b"[]".as_slice(),
        b"null",
        b"{\"a\":1}",
        b"{\"a\":{}}",
        b"{\"a\":\"x\",\"a\":\"y\"}",
        b"{\"a\":\"$unsafe\"}",
        b"{\"a\":\"line\\n\"}",
        b"{\"bad-key\":\"x\"}",
        b"\xff",
    ] {
        assert!(parse(config).is_err(), "{config:?}");
    }
    let many = (0..65)
        .map(|i| (format!("f{i}"), json!("x")))
        .collect::<serde_json::Map<_, _>>();
    assert!(parse(&serde_json::to_vec(&many).unwrap()).is_err());
    assert!(parse(&serde_json::to_vec(&json!({"a":"x".repeat(4097)})).unwrap()).is_err());
    assert!(parse(&serde_json::to_vec(&json!({"a":"x".repeat(4096)})).unwrap()).is_ok());
    assert!(parse(b"{}").is_ok()); // Field completeness belongs to the actual archive owner.
    let mut too_large = CONFIG.to_vec();
    too_large.resize(1024 * 1024 + 1, b' ');
    assert!(parse(&too_large).is_err());
    assert!(
        RetainedLocalSelection::parse_mcpb(
            NODE_PACKAGE,
            &selection(CONFIG),
            b"{}",
            id("node-server"),
            reference("decision"),
            verification()
        )
        .is_err()
    );
    assert!(
        RetainedLocalSelection::parse(
            NODE_PACKAGE,
            &selection(CONFIG),
            LocalSelectionProfile::Mcpb,
            id("node-server"),
            reference("decision"),
            verification()
        )
        .is_err()
    );
}

#[tokio::test]
async fn configuration_and_evidence_changes_conflict_at_same_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let original = recipes
        .save_mcpb_selection(
            NODE_PACKAGE,
            &selection(CONFIG),
            CONFIG,
            id("node-server"),
            reference("decision"),
            verification(),
        )
        .await
        .unwrap();
    for change in ["format", "value", "decision", "evidence"] {
        let config = match change {
            "format" => b"{\"toolsets\":\"navigation\"}".to_vec(),
            "value" => b"{\"toolsets\":\"other\"}".to_vec(),
            _ => CONFIG.to_vec(),
        };
        let decision = reference(if change == "decision" {
            "another"
        } else {
            "decision"
        });
        let mut observations = verification();
        if change == "evidence" {
            observations.reverse();
        }
        let selected = RetainedLocalSelection::parse_mcpb(
            NODE_PACKAGE,
            &selection(&config),
            &config,
            id("node-server"),
            decision.clone(),
            observations.clone(),
        )
        .unwrap();
        assert_ne!(selected.digest(), original.digest());
        assert!(
            matches!(
                recipes
                    .save_mcpb_selection(
                        NODE_PACKAGE,
                        &selection(&config),
                        &config,
                        id("node-server"),
                        decision,
                        observations
                    )
                    .await,
                Err(LocalRecipeError::ImmutableConflict)
            ),
            "{change}"
        );
    }
}

#[tokio::test]
async fn old_profiles_keep_exact_version_two_body_and_retention_digest() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let saved = recipes
        .save_selection(
            NODE_PACKAGE,
            NODE_SELECTION,
            LocalSelectionProfile::Node,
            id("node-server"),
            reference("decision"),
            verification(),
        )
        .await
        .unwrap();
    let package: IntegrationPackage = serde_json::from_slice(NODE_PACKAGE).unwrap();
    let validated = serde_json::from_slice::<ActivationManifest>(NODE_SELECTION)
        .unwrap()
        .validate(&package)
        .unwrap();
    let body = json!({"identity":package.identity,"package_content_digest":package.content_digest().unwrap(),"manifest_digest":validated.digest(),"package_json":String::from_utf8_lossy(NODE_PACKAGE),"selection_json":String::from_utf8_lossy(NODE_SELECTION),"profile":"node","component":"node-server","decision":reference("decision"),"verification":verification()});
    let rows = store.list_records(&scope("alice"), None, 10).await.unwrap();
    assert_eq!(
        rows[0].data,
        RecordData {
            schema: LOCAL_RECIPE_SCHEMA.into(),
            version: 2,
            body: body.clone()
        }
    );
    assert_eq!(
        saved.digest(),
        &ContentDigest::of_bytes(&canonical_json(&(LOCAL_RECIPE_SCHEMA, 2_u32, &body)).unwrap())
    );
    assert_eq!(saved.configuration_json(), None);
    assert_eq!(saved.configuration_digest(), None);
    assert!(matches!(
        recipes
            .save_mcpb_selection(
                NODE_PACKAGE,
                &selection(CONFIG),
                CONFIG,
                id("node-server"),
                reference("decision"),
                verification()
            )
            .await,
        Err(LocalRecipeError::ImmutableConflict)
    ));
}

#[tokio::test]
async fn mcpb_cannot_replace_v1_or_v2_and_neither_can_replace_mcpb() {
    let remote = IntegrationBundle::parse(REMOTE_PLUGIN, Some(REMOTE_MCP), REMOTE_RECIPES).unwrap();
    for old_version in [1, 2] {
        for new_first in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
            let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
            let mut package: IntegrationPackage = serde_json::from_slice(NODE_PACKAGE).unwrap();
            if old_version == 1 {
                package.identity = remote.package().identity.clone();
            }
            let p = serde_json::to_vec(&package).unwrap();
            let mut old: ActivationManifest = serde_json::from_slice(NODE_SELECTION).unwrap();
            old.package = package.identity.clone();
            old.package_content_digest = package.content_digest().unwrap();
            let old = serde_json::to_vec(&old).unwrap();
            let mut new: ActivationManifest = serde_json::from_slice(&selection(CONFIG)).unwrap();
            new.package = package.identity.clone();
            new.package_content_digest = package.content_digest().unwrap();
            let new = serde_json::to_vec(&new).unwrap();
            if new_first {
                recipes
                    .save_mcpb_selection(
                        &p,
                        &new,
                        CONFIG,
                        id("node-server"),
                        reference("decision"),
                        verification(),
                    )
                    .await
                    .unwrap();
            }
            let old_result = if old_version == 1 {
                recipes
                    .save(
                        REMOTE_PLUGIN,
                        Some(REMOTE_MCP),
                        REMOTE_RECIPES,
                        reference("decision"),
                    )
                    .await
                    .map(|_| ())
            } else {
                recipes
                    .save_selection(
                        &p,
                        &old,
                        LocalSelectionProfile::Node,
                        id("node-server"),
                        reference("decision"),
                        verification(),
                    )
                    .await
                    .map(|_| ())
            };
            if new_first {
                assert!(matches!(
                    old_result,
                    Err(LocalRecipeError::ImmutableConflict)
                ));
            } else {
                old_result.unwrap();
                assert!(matches!(
                    recipes
                        .save_mcpb_selection(
                            &p,
                            &new,
                            CONFIG,
                            id("node-server"),
                            reference("decision"),
                            verification()
                        )
                        .await,
                    Err(LocalRecipeError::ImmutableConflict)
                ));
            }
        }
    }
}

#[tokio::test]
async fn crossed_versions_configuration_tamper_and_self_consistent_replacement_refuse() {
    for change in [
        "version",
        "profile",
        "missing",
        "null",
        "config-digest",
        "record-digest",
        "consistent-replacement",
        "extra",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recipes.sqlite");
        let selected = parse(CONFIG).unwrap();
        {
            let store = SqliteStore::open(&path).unwrap();
            LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .save_mcpb_selection(
                    NODE_PACKAGE,
                    &selection(CONFIG),
                    CONFIG,
                    id("node-server"),
                    reference("decision"),
                    verification(),
                )
                .await
                .unwrap();
        }
        change_record(&path, |record| match change {
            "version" => record.version = 2,
            "profile" => record.body["profile"] = json!("node"),
            "missing" => {
                record
                    .body
                    .as_object_mut()
                    .unwrap()
                    .remove("configuration_json");
            }
            "null" => record.body["configuration_json"] = Value::Null,
            "config-digest" => record.body["configuration_json"] = json!("{}"),
            "record-digest" => record.body["manifest_digest"] = json!("a".repeat(64)),
            "consistent-replacement" => {
                record.body["configuration_json"] = json!("{}");
                let new = selection(b"{}");
                let p: IntegrationPackage = serde_json::from_slice(NODE_PACKAGE).unwrap();
                let m: ActivationManifest = serde_json::from_slice(&new).unwrap();
                record.body["manifest_digest"] = json!(m.validate(&p).unwrap().digest());
                record.body["selection_json"] = json!(String::from_utf8(new).unwrap());
            }
            "extra" => record.body["prepared_evidence"] = json!({"claims":"ready"}),
            _ => unreachable!(),
        });
        let store = SqliteStore::open_existing(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        assert!(
            recipes
                .load_retained_exact(&selected.package().identity, selected.digest())
                .await
                .is_err(),
            "{change}"
        );
        let list = recipes.list_retained().await.unwrap();
        assert_eq!(
            list.verified.len(),
            usize::from(change == "consistent-replacement"),
            "{change}"
        );
    }
    for change in ["v3", "null"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recipes.sqlite");
        let selected;
        {
            let store = SqliteStore::open(&path).unwrap();
            selected = LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .save_selection(
                    NODE_PACKAGE,
                    NODE_SELECTION,
                    LocalSelectionProfile::Node,
                    id("node-server"),
                    reference("decision"),
                    verification(),
                )
                .await
                .unwrap();
        }
        change_record(&path, |r| {
            if change == "v3" {
                r.version = 3;
            } else {
                r.body["configuration_json"] = Value::Null;
            }
        });
        let store = SqliteStore::open_existing(&path).unwrap();
        assert!(
            LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .load_selection_exact(&selected.package().identity, selected.digest())
                .await
                .is_err()
        );
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    AfterCommit,
    ForeignNamespace,
    ChangedResponse,
}

struct FaultyStore {
    inner: SqliteStore,
    fault: Fault,
}
impl FaultyStore {
    fn alter(&self, record: &mut StoredRecord) {
        match self.fault {
            Fault::ForeignNamespace => record.key.owner = "bob".into(),
            Fault::ChangedResponse => {
                record.data.body["decision"]["record"] = json!("changed-after-admission");
                record.content_digest = record.data.digest().unwrap();
            }
            Fault::AfterCommit => (),
        }
    }
}
#[async_trait]
impl ToolkitStore for FaultyStore {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        let outcome = self.inner.admit(request).await?;
        if self.fault == Fault::AfterCommit {
            return Err(StoreError::Synchronization);
        }
        let (created, mut record) = match outcome {
            AdmissionOutcome::Created(record) => (true, record),
            AdmissionOutcome::Existing(record) => (false, record),
        };
        self.alter(&mut record);
        Ok(if created {
            AdmissionOutcome::Created(record)
        } else {
            AdmissionOutcome::Existing(record)
        })
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        let mut record = self.inner.get(key).await?;
        if let Some(record) = &mut record {
            self.alter(record);
        }
        Ok(record)
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after_id: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        let mut records = self.inner.list_records(scope, after_id, limit).await?;
        for record in &mut records {
            self.alter(record);
        }
        Ok(records)
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
async fn mcpb_lost_acknowledgement_and_false_backend_responses_keep_exact_recovery() {
    for fault in [
        Fault::AfterCommit,
        Fault::ForeignNamespace,
        Fault::ChangedResponse,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recipes.sqlite");
        let expected = parse(CONFIG).unwrap();
        {
            let store = FaultyStore {
                inner: SqliteStore::open(&path).unwrap(),
                fault,
            };
            let result = LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .save_mcpb_selection(
                    NODE_PACKAGE,
                    &selection(CONFIG),
                    CONFIG,
                    id("node-server"),
                    reference("decision"),
                    verification(),
                )
                .await;
            if fault == Fault::AfterCommit {
                assert!(matches!(result, Err(LocalRecipeError::Unconfirmed(_))));
            } else {
                assert!(matches!(result, Err(LocalRecipeError::Unverified(_))));
            }
        }
        let store = SqliteStore::open_existing(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let recovered = recipes
            .load_selection_exact(&expected.package().identity, expected.digest())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.configuration_json(), Some(CONFIG));
        let before = store.list_records(&scope("alice"), None, 10).await.unwrap();
        recipes
            .save_mcpb_selection(
                NODE_PACKAGE,
                &selection(CONFIG),
                CONFIG,
                id("node-server"),
                reference("decision"),
                verification(),
            )
            .await
            .unwrap();
        assert_eq!(
            before,
            store.list_records(&scope("alice"), None, 10).await.unwrap()
        );
        assert!(
            store
                .list_effects(&before[0].key, None, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn mcpb_unsupported_selection_and_unbounded_record_refuse_before_admission() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    for change in [
        "runtime",
        "dependencies",
        "component",
        "empty-evidence",
        "duplicate-evidence",
        "package-duplicate",
        "selection-duplicate",
        "encoded-limit",
    ] {
        let mut p = NODE_PACKAGE.to_vec();
        let mut s = selection(CONFIG);
        let mut c = CONFIG.to_vec();
        let mut component = id("node-server");
        let mut observations = verification();
        match change {
            "runtime" | "dependencies" => {
                let mut v: Value = serde_json::from_slice(&s).unwrap();
                if change == "runtime" {
                    v["components"][0]["runtime"] = json!("python");
                } else {
                    v["components"][0]["source"]["dependencies"] =
                        json!({"kind":"pinned_closure","artifacts":{"x":"a".repeat(64)}});
                }
                s = serde_json::to_vec(&v).unwrap();
            }
            "component" => component = id("other"),
            "empty-evidence" => observations.clear(),
            "duplicate-evidence" => observations.push(observations[0].clone()),
            "package-duplicate" => {
                p.pop();
                let i = p.iter().rposition(|b| *b == b'}').unwrap();
                p.splice(i..i, b",\"schema_version\":1".to_vec());
            }
            "selection-duplicate" => {
                let i = s.iter().rposition(|b| *b == b'}').unwrap();
                s.splice(i..i, b",\"schema_version\":1".to_vec());
            }
            "encoded-limit" => {
                p.resize(1024 * 1024, b' ');
                c.resize(1024 * 1024, b' ');
                s = selection(&c);
            }
            _ => unreachable!(),
        }
        assert!(
            recipes
                .save_mcpb_selection(&p, &s, &c, component, reference("decision"), observations)
                .await
                .is_err(),
            "{change}"
        );
    }
    assert!(
        store
            .list_records(&scope("alice"), None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
#[test]
fn retained_metadata_never_substitutes_for_actual_bundled_configuration_validation() {
    use std::collections::BTreeMap;
    use std::io::{Cursor, Write};
    use toolkit_package::local_artifacts::*;
    use zip::write::SimpleFileOptions;
    // This archive deliberately declares a sensitive field. Pure retention has
    // no bundled manifest; only the existing full plan can reject that fact.
    let manifest = json!({"manifest_version":"0.3","name":"node-server","version":"0.0.0-fixture",
        "description":"Inert archive validation boundary fixture","author":{"name":"Fixture"},
        "server":{"type":"node","entry_point":"server.mjs","mcp_config":{"command":"node","args":["${__dirname}/server.mjs","--toolsets=${user_config.toolsets}"]}},
        "compatibility":{"platforms":["darwin","linux"],"runtimes":{"node":">=22.0.0"}},
        "user_config":{"toolsets":{"type":"string","title":"Toolsets","sensitive":true}}});
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in [
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("server.mjs", b"// inert, never executed".to_vec()),
    ] {
        zip.start_file(name, SimpleFileOptions::default().unix_permissions(0o644))
            .unwrap();
        zip.write_all(&data).unwrap();
    }
    let bytes = zip.finish().unwrap().into_inner();
    let mut m: ActivationManifest = serde_json::from_slice(&selection(CONFIG)).unwrap();
    let ComponentSource::Artifact { content_digest, .. } = &mut m.components[0].source else {
        unreachable!()
    };
    *content_digest = ContentDigest::of_bytes(&bytes);
    let m = serde_json::to_vec(&m).unwrap();
    let retained = RetainedLocalSelection::parse_mcpb(
        NODE_PACKAGE,
        &m,
        CONFIG,
        id("node-server"),
        reference("decision"),
        verification(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("input.mcpb");
    std::fs::write(&archive, &bytes).unwrap();
    let work = temp.path().join("attempt");
    std::fs::create_dir(&work).unwrap();
    let owner = LocalArtifactPreparer::new(
        cap_std::fs::Dir::open_ambient_dir(&work, cap_std::ambient_authority()).unwrap(),
    );
    let prepared = owner
        .prepare(
            retained.validated_selection(),
            retained.component(),
            LocalArtifactLayout::Zip,
            LocalArtifactInputs {
                artifact: std::fs::File::open(archive).unwrap(),
                pinned_dependencies: BTreeMap::new(),
            },
            &PreparationLimits::default(),
        )
        .unwrap();
    assert!(
        McpbInstallPlan::new(
            retained.package(),
            retained.validated_selection(),
            retained.component(),
            &prepared,
            retained.configuration_json().unwrap()
        )
        .is_err()
    );
}
