#![cfg(feature = "local-recipes")]

use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::Path;
use toolkit_package::*;
use toolkit_store::*;

const PYTHON_PACKAGE: &[u8] = include_bytes!("fixtures/python-install/package.json");
const PYTHON_SELECTION: &[u8] = include_bytes!("fixtures/python-install/activation.json");
const NODE_PACKAGE: &[u8] = include_bytes!("fixtures/node-install/package.json");
const NODE_SELECTION: &[u8] = include_bytes!("fixtures/node-install/activation.json");
const REMOTE_PLUGIN: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json");
const REMOTE_MCP: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/mcp.json");
const REMOTE_RECIPES: &[u8] =
    include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json");

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn scope(owner: &str) -> StoreScope {
    StoreScope {
        realm: "household".into(),
        owner: owner.into(),
    }
}
fn evidence(record: &str) -> EvidenceReference {
    EvidenceReference {
        owner: id("historical-observations"),
        record: id(record),
        revision: Revision::new("1").unwrap(),
    }
}
fn observations() -> Vec<EvidenceReference> {
    vec![evidence("installation"), evidence("mcp-check")]
}
fn parse(package: &[u8], selection: &[u8]) -> RetainedLocalSelection {
    RetainedLocalSelection::parse(
        package,
        selection,
        LocalSelectionProfile::Python,
        id("python-server"),
        evidence("decision"),
        observations(),
    )
    .unwrap()
}
fn python_with_identity(identity: PackageIdentity) -> (Vec<u8>, Vec<u8>) {
    let mut package: IntegrationPackage = serde_json::from_slice(PYTHON_PACKAGE).unwrap();
    package.identity = identity.clone();
    // A source commit is valid canonical package provenance. The remote recipe
    // format's documentation-hash restriction must not rewrite it.
    package.provenance.source_revision =
        Revision::new("599dafc1054550a6eeb87a6545c1e1b03b3ca8270").unwrap();
    let mut selection: ActivationManifest = serde_json::from_slice(PYTHON_SELECTION).unwrap();
    selection.package = identity;
    selection.package_content_digest = package.content_digest().unwrap();
    (
        serde_json::to_vec_pretty(&package).unwrap(),
        serde_json::to_vec_pretty(&selection).unwrap(),
    )
}

#[tokio::test]
async fn canonical_local_selections_survive_reopen_without_rewriting_source_or_host_intent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("recipes.sqlite");
    let original: IntegrationPackage = serde_json::from_slice(PYTHON_PACKAGE).unwrap();
    let (python_package, python_selection) = python_with_identity(original.identity);
    let mut expected = Vec::new();
    {
        let store = SqliteStore::open(&path).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        for (package, selection, profile, component) in [
            (
                python_package.as_slice(),
                python_selection.as_slice(),
                LocalSelectionProfile::Python,
                "python-server",
            ),
            (
                NODE_PACKAGE,
                NODE_SELECTION,
                LocalSelectionProfile::Node,
                "node-server",
            ),
        ] {
            let saved = recipes
                .save_selection(
                    package,
                    selection,
                    profile,
                    id(component),
                    evidence("decision"),
                    observations(),
                )
                .await
                .unwrap();
            assert_eq!(saved.package_json(), package);
            assert_eq!(saved.selection_json(), selection);
            assert_eq!(saved.profile(), profile);
            assert_eq!(saved.component(), &id(component));
            assert_eq!(saved.decision(), &evidence("decision"));
            assert_eq!(saved.verification(), observations());
            assert_eq!(
                saved.origin(),
                CatalogOrigin::LocalInvestigation {
                    decision: evidence("decision")
                }
            );
            assert_ne!(saved.digest(), saved.validated_selection().digest());
            assert_ne!(saved.digest(), &saved.package().content_digest().unwrap());
            expected.push((
                saved.package().identity.clone(),
                saved.digest().clone(),
                package.to_vec(),
                selection.to_vec(),
            ));
        }
    }
    let store = SqliteStore::open_existing(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    for (identity, digest, package, selection) in expected {
        let saved = recipes
            .load_selection_exact(&identity, &digest)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.package_json(), package);
        assert_eq!(saved.selection_json(), selection);
        assert_eq!(
            saved.validated_selection().manifest(),
            &serde_json::from_slice::<ActivationManifest>(&selection).unwrap()
        );
        assert!(matches!(
            recipes
                .load_retained_exact(&identity, &digest)
                .await
                .unwrap(),
            Some(RetainedLocalRecipe::Selection(_))
        ));
        assert!(
            LocalRecipeStore::new(&store, scope("bob"))
                .unwrap()
                .load_retained_exact(&identity, &digest)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            recipes
                .load_selection_exact(&identity, &ContentDigest::of_bytes(b"stale"))
                .await
                .is_err()
        );
    }
    assert_eq!(recipes.list_retained().await.unwrap().verified.len(), 2);
    assert!(recipes.list_retained().await.unwrap().unverified.is_empty());
}

#[tokio::test]
async fn exact_documents_decision_and_ordered_verification_are_immutable() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let original = recipes
        .save_selection(
            PYTHON_PACKAGE,
            PYTHON_SELECTION,
            LocalSelectionProfile::Python,
            id("python-server"),
            evidence("decision"),
            observations(),
        )
        .await
        .unwrap();
    let again = recipes
        .save_selection(
            PYTHON_PACKAGE,
            PYTHON_SELECTION,
            LocalSelectionProfile::Python,
            id("python-server"),
            evidence("decision"),
            observations(),
        )
        .await
        .unwrap();
    assert_eq!(original.digest(), again.digest());
    for change in [
        "package-format",
        "selection-format",
        "decision",
        "verification",
        "verification-order",
        "policy",
    ] {
        let mut package = PYTHON_PACKAGE.to_vec();
        let mut selection = PYTHON_SELECTION.to_vec();
        let mut decision = evidence("decision");
        let mut verification = observations();
        match change {
            "package-format" => package.push(b'\n'),
            "selection-format" => selection.push(b'\n'),
            "decision" => decision = evidence("different-decision"),
            "verification" => verification[0] = evidence("different-observation"),
            "verification-order" => verification.reverse(),
            "policy" => {
                let mut value: Value = serde_json::from_slice(&selection).unwrap();
                value["policy"]["revision"] = json!("2");
                selection = serde_json::to_vec(&value).unwrap();
            }
            _ => unreachable!(),
        }
        let changed = RetainedLocalSelection::parse(
            &package,
            &selection,
            LocalSelectionProfile::Python,
            id("python-server"),
            decision.clone(),
            verification.clone(),
        )
        .unwrap();
        assert_ne!(changed.digest(), original.digest(), "{change}");
        assert!(
            matches!(
                recipes
                    .save_selection(
                        &package,
                        &selection,
                        LocalSelectionProfile::Python,
                        id("python-server"),
                        decision,
                        verification
                    )
                    .await,
                Err(LocalRecipeError::ImmutableConflict)
            ),
            "{change}"
        );
    }
    assert_eq!(
        recipes
            .load_selection_exact(&original.package().identity, original.digest())
            .await
            .unwrap()
            .unwrap()
            .selection_json(),
        PYTHON_SELECTION
    );
}

#[tokio::test]
async fn both_forms_share_identity_and_mixed_read_preserves_v1_digests() {
    let remote = IntegrationBundle::parse(REMOTE_PLUGIN, Some(REMOTE_MCP), REMOTE_RECIPES).unwrap();
    let (package, selection) = python_with_identity(remote.package().identity.clone());
    for selection_first in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        if selection_first {
            let saved = recipes
                .save_selection(
                    &package,
                    &selection,
                    LocalSelectionProfile::Python,
                    id("python-server"),
                    evidence("decision"),
                    observations(),
                )
                .await
                .unwrap();
            assert!(matches!(
                recipes
                    .save(
                        REMOTE_PLUGIN,
                        Some(REMOTE_MCP),
                        REMOTE_RECIPES,
                        evidence("decision")
                    )
                    .await,
                Err(LocalRecipeError::ImmutableConflict)
            ));
            assert!(recipes.load(&saved.package().identity).await.is_err());
            let old_listing = recipes.list().await.unwrap();
            assert!(old_listing.verified.is_empty());
            assert_eq!(old_listing.unverified.len(), 1);
        } else {
            let saved = recipes
                .save(
                    REMOTE_PLUGIN,
                    Some(REMOTE_MCP),
                    REMOTE_RECIPES,
                    evidence("decision"),
                )
                .await
                .unwrap();
            assert!(matches!(
                recipes
                    .load_retained_exact(&remote.package().identity, saved.bundle().digest())
                    .await
                    .unwrap(),
                Some(RetainedLocalRecipe::Recipe(_))
            ));
            assert!(matches!(
                recipes
                    .save_selection(
                        &package,
                        &selection,
                        LocalSelectionProfile::Python,
                        id("python-server"),
                        evidence("decision"),
                        observations()
                    )
                    .await,
                Err(LocalRecipeError::ImmutableConflict)
            ));
            assert!(
                recipes
                    .load_selection_exact(&remote.package().identity, saved.bundle().digest())
                    .await
                    .is_err()
            );
            assert_eq!(
                recipes
                    .load_exact(&remote.package().identity, saved.bundle().digest())
                    .await
                    .unwrap()
                    .unwrap()
                    .plugin_json(),
                REMOTE_PLUGIN
            );
        }
        // A different identity adds the other form without hiding either entry.
        if selection_first {
            let mut plugin: Value = serde_json::from_slice(REMOTE_PLUGIN).unwrap();
            plugin["extensions"]["ai.rkat.toolkit"]["identity"]["name"] = json!("another-remote");
            recipes
                .save(
                    &serde_json::to_vec(&plugin).unwrap(),
                    Some(REMOTE_MCP),
                    REMOTE_RECIPES,
                    evidence("decision"),
                )
                .await
                .unwrap();
        } else {
            recipes
                .save_selection(
                    NODE_PACKAGE,
                    NODE_SELECTION,
                    LocalSelectionProfile::Node,
                    id("node-server"),
                    evidence("decision"),
                    observations(),
                )
                .await
                .unwrap();
        }
        let mixed = recipes.list_retained().await.unwrap();
        assert_eq!(mixed.verified.len(), 2);
        assert!(mixed.unverified.is_empty());
    }
}

#[tokio::test]
async fn unsupported_profiles_and_incomplete_evidence_are_rejected_before_admission() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    for change in [
        "package",
        "wrong-profile",
        "component",
        "self-contained",
        "entrypoint",
        "state",
        "required-contract",
        "extra-component",
        "absent-evidence",
        "duplicate-evidence",
        "too-many-evidence",
    ] {
        let mut package: Value = serde_json::from_slice(PYTHON_PACKAGE).unwrap();
        let mut selection: Value = serde_json::from_slice(PYTHON_SELECTION).unwrap();
        let mut profile = LocalSelectionProfile::Python;
        let mut component = id("python-server");
        let mut verification = observations();
        match change {
            "package" => package["description"] = json!("Changed without rebinding manifest"),
            "wrong-profile" => profile = LocalSelectionProfile::Node,
            "component" => component = id("different"),
            "self-contained" => {
                selection["components"][0]["source"]["dependencies"] =
                    json!({"kind":"self_contained"})
            }
            "entrypoint" => {
                selection["components"][0]["source"]["entrypoint"]["executable"] =
                    json!("unsafe.sh")
            }
            "state" => {
                selection["components"][0]["state"] =
                    json!([{"namespace":"state","owner":"owner","format":"1","migrations":[]}])
            }
            "required-contract" => {
                selection["required_contracts"] =
                    json!([{"name":"host-only","version":{"major":1,"minimum_minor":0}}])
            }
            "extra-component" => {
                let extra = selection["components"][0].clone();
                selection["components"].as_array_mut().unwrap().push(extra);
            }
            "absent-evidence" => verification.clear(),
            "duplicate-evidence" => verification.push(verification[0].clone()),
            "too-many-evidence" => {
                verification = (0..257)
                    .map(|n| evidence(&format!("observation-{n}")))
                    .collect()
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                recipes
                    .save_selection(
                        &serde_json::to_vec(&package).unwrap(),
                        &serde_json::to_vec(&selection).unwrap(),
                        profile,
                        component,
                        evidence("decision"),
                        verification
                    )
                    .await,
                Err(LocalRecipeError::Invalid(_))
            ),
            "{change}"
        );
    }
    assert!(serde_json::from_value::<LocalSelectionProfile>(json!("native")).is_err());
    assert!(recipes.list_retained().await.unwrap().verified.is_empty());
}

#[test]
fn malformed_documents_and_full_encoded_store_limits_are_preflighted() {
    for (package, selection) in [
        (&b"not json"[..], PYTHON_SELECTION),
        (PYTHON_PACKAGE, &b"\xff"[..]),
    ] {
        assert!(
            RetainedLocalSelection::parse(
                package,
                selection,
                LocalSelectionProfile::Python,
                id("python-server"),
                evidence("decision"),
                observations()
            )
            .is_err()
        );
    }
    let mut package = PYTHON_PACKAGE.to_vec();
    package.resize(1024 * 1024 + 1, b' ');
    assert!(
        RetainedLocalSelection::parse(
            &package,
            PYTHON_SELECTION,
            LocalSelectionProfile::Python,
            id("python-server"),
            evidence("decision"),
            observations()
        )
        .is_err()
    );
    package.pop();
    let mut selection = PYTHON_SELECTION.to_vec();
    selection.resize(1024 * 1024, b' ');
    // Both documents individually fit, but the full versioned envelope does not.
    assert!(
        RetainedLocalSelection::parse(
            &package,
            &selection,
            LocalSelectionProfile::Python,
            id("python-server"),
            evidence("decision"),
            observations()
        )
        .is_err()
    );
}

fn tamper(path: &Path, change: impl FnOnce(&mut RecordData), repair_digest: bool) {
    let db = rusqlite::Connection::open(path).unwrap();
    let raw: String = db
        .query_row("SELECT data FROM toolkit_records", [], |row| row.get(0))
        .unwrap();
    let mut data: RecordData = serde_json::from_str(&raw).unwrap();
    change(&mut data);
    let encoded = serde_json::to_string(&data).unwrap();
    if repair_digest {
        db.execute(
            "UPDATE toolkit_records SET data=?1,digest=?2",
            rusqlite::params![encoded, data.digest().unwrap()],
        )
        .unwrap();
    } else {
        db.execute("UPDATE toolkit_records SET data=?1", [encoded])
            .unwrap();
    }
}

#[tokio::test]
async fn every_read_rechecks_envelope_canonical_selection_and_pinned_retention_digest() {
    for change in [
        "outer-digest",
        "schema",
        "version",
        "identity",
        "package-digest",
        "manifest-digest",
        "unknown-field",
        "bad-profile",
        "bad-component",
        "duplicate-evidence",
        "self-consistent-replacement",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recipes.sqlite");
        let selected = parse(PYTHON_PACKAGE, PYTHON_SELECTION);
        {
            let store = SqliteStore::open(&path).unwrap();
            LocalRecipeStore::new(&store, scope("alice"))
                .unwrap()
                .save_selection(
                    PYTHON_PACKAGE,
                    PYTHON_SELECTION,
                    LocalSelectionProfile::Python,
                    id("python-server"),
                    evidence("decision"),
                    observations(),
                )
                .await
                .unwrap();
        }
        tamper(
            &path,
            |data| match change {
                "outer-digest" | "self-consistent-replacement" => {
                    data.body["decision"]["record"] = json!("new-decision")
                }
                "schema" => data.schema = "another-schema".into(),
                "version" => data.version = 3,
                "identity" => data.body["identity"]["name"] = json!("another"),
                "package-digest" => data.body["package_content_digest"] = json!("a".repeat(64)),
                "manifest-digest" => data.body["manifest_digest"] = json!("a".repeat(64)),
                "unknown-field" => data.body["extra"] = json!(true),
                "bad-profile" => data.body["profile"] = json!("mcpb"),
                "bad-component" => data.body["component"] = json!("another"),
                "duplicate-evidence" => {
                    let duplicate = data.body["verification"][0].clone();
                    data.body["verification"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                _ => unreachable!(),
            },
            change != "outer-digest",
        );
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
        if change == "self-consistent-replacement" {
            assert_eq!(list.verified.len(), 1);
            assert_ne!(list.verified[0].digest(), selected.digest());
        } else {
            assert!(list.verified.is_empty(), "{change}");
            assert_eq!(list.unverified.len(), 1, "{change}");
        }
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
async fn lost_commit_response_requires_exact_readback_and_does_not_repeat_effects() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("recipes.sqlite");
    let selected = parse(PYTHON_PACKAGE, PYTHON_SELECTION);
    {
        let store = FaultyStore {
            inner: SqliteStore::open(&path).unwrap(),
            fault: Fault::AfterCommit,
        };
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        assert!(matches!(
            recipes
                .save_selection(
                    PYTHON_PACKAGE,
                    PYTHON_SELECTION,
                    LocalSelectionProfile::Python,
                    id("python-server"),
                    evidence("decision"),
                    observations()
                )
                .await,
            Err(LocalRecipeError::Unconfirmed(_))
        ));
    }
    let store = SqliteStore::open_existing(&path).unwrap();
    let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
    let recovered = recipes
        .load_selection_exact(&selected.package().identity, selected.digest())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.selection_json(), PYTHON_SELECTION);
    let before = store.list_records(&scope("alice"), None, 10).await.unwrap();
    let replay = recipes
        .save_selection(
            PYTHON_PACKAGE,
            PYTHON_SELECTION,
            LocalSelectionProfile::Python,
            id("python-server"),
            evidence("decision"),
            observations(),
        )
        .await
        .unwrap();
    assert_eq!(replay.digest(), selected.digest());
    assert_eq!(
        store.list_records(&scope("alice"), None, 10).await.unwrap(),
        before
    );
    assert!(
        store
            .list_effects(&before[0].key, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .read_events(&before[0].key, 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn foreign_or_changed_backend_responses_never_acknowledge_the_requested_selection() {
    for fault in [Fault::ForeignNamespace, Fault::ChangedResponse] {
        let temp = tempfile::tempdir().unwrap();
        let store = FaultyStore {
            inner: SqliteStore::open(temp.path().join("recipes.sqlite")).unwrap(),
            fault,
        };
        let recipes = LocalRecipeStore::new(&store, scope("alice")).unwrap();
        let selected = parse(PYTHON_PACKAGE, PYTHON_SELECTION);
        assert!(matches!(
            recipes
                .save_selection(
                    PYTHON_PACKAGE,
                    PYTHON_SELECTION,
                    LocalSelectionProfile::Python,
                    id("python-server"),
                    evidence("decision"),
                    observations()
                )
                .await,
            Err(LocalRecipeError::Unverified(_))
        ));
        assert!(
            recipes
                .load_retained_exact(&selected.package().identity, selected.digest())
                .await
                .is_err()
        );
        if fault == Fault::ForeignNamespace {
            let listing = recipes.list_retained().await.unwrap();
            assert!(listing.verified.is_empty());
            assert_eq!(listing.unverified.len(), 1);
        }
        // Backend response corruption did not alter the actual admitted bytes.
        assert!(
            LocalRecipeStore::new(&store.inner, scope("alice"))
                .unwrap()
                .load_selection_exact(&selected.package().identity, selected.digest())
                .await
                .unwrap()
                .is_some()
        );
    }
}

// Explicit software declaration fixture, independent of ActivationManifest.
fn software_documents(profile: LocalSelectionProfile) -> (&'static [u8], Vec<u8>, &'static str) {
    let (bytes, runtime, component, strategy, location, executable, artifacts) = match profile {
        LocalSelectionProfile::Python => (PYTHON_PACKAGE, "python", "python-server", "python-local",
            "https://example.invalid/fixture_server-1.0-py3-none-any.whl", "fixture_server/__main__.py",
            json!({"requirements":"2".repeat(64),"dependency_one-1.0-py3-none-any.whl":"3".repeat(64)})),
        LocalSelectionProfile::Node => (NODE_PACKAGE, "node", "node-server", "node-local",
            "https://example.invalid/node-server.tgz", "dist/index.js",
            json!({"package-json":"2".repeat(64),"package-lock":"3".repeat(64),"dependency-archive":"4".repeat(64)})),
        LocalSelectionProfile::Mcpb => panic!("full MCPB path only"),
    };
    let package: IntegrationPackage = serde_json::from_slice(bytes).unwrap();
    let value = json!({"schema_version":1,"document_kind":"local_software_declaration",
        "package":package.identity,"package_content_digest":package.content_digest().unwrap(),"strategy":strategy,
        "components":[{"id":component,"version":"0.0.0-fixture","runtime":runtime,
            "source":{"kind":"artifact","location":location,"content_digest":"1".repeat(64),
                "dependencies":{"kind":"pinned_closure","artifacts":artifacts},
                "entrypoint":{"executable":executable,"arguments":[]}}}],
        "exports":[{"role":"service_connector","id":"service","components":[component],
            "authentication":{"kind":"none"},"operations":[{"id":"fixture-read","schema_digest":"5".repeat(64),
                "declared_effect":"read","required_authority":["read"],"resources":{"kind":"connection_wide"},
                "result_limits":{"maximum_bytes":4096,"maximum_items":10}}]}],
        "portable_mcp_entries":[{"name":"software-server","component":component,
            "operation_tools":{"fixture-read":"actual_read"}}]});
    (bytes, serde_json::to_vec_pretty(&value).unwrap(), component)
}

#[tokio::test]
async fn software_records_reopen_exact_documents_without_fabricating_legacy_proofs() {
    let temp=tempfile::tempdir().unwrap();
    let path=temp.path().join("recipes.sqlite");
    let mut saved_records=Vec::new();
    {
        let store=SqliteStore::open(&path).unwrap();
        let recipes=LocalRecipeStore::new(&store,scope("alice")).unwrap();
        for profile in [LocalSelectionProfile::Python,LocalSelectionProfile::Node] {
            let (package,declaration,component)=software_documents(profile);
            let software=RetainedLocalSoftware::parse(package,&declaration,profile,id(component),evidence("decision"),observations()).unwrap();
            let saved=recipes.save_retained_software(software.clone()).await.unwrap();
            let before=store.list_records(&scope("alice"),None,10).await.unwrap();
            assert_eq!(recipes.save_retained_software(software).await.unwrap().digest(),saved.digest());
            assert_eq!(store.list_records(&scope("alice"),None,10).await.unwrap(),before);
            let record=before.iter().find(|r| r.data.body["identity"]==json!(saved.package().identity)).unwrap();
            assert_eq!(record.data.version,4);
            assert_eq!(record.data.body["document_kind"],"local_software_declaration");
            assert!(record.data.body.get("manifest_digest").is_none());
            assert!(store.list_effects(&record.key,None,10).await.unwrap().is_empty());
            assert_ne!(saved.digest(),saved.validated_declaration().digest());
            saved_records.push(saved);
        }
    }
    let store=SqliteStore::open_existing(&path).unwrap();
    let recipes=LocalRecipeStore::new(&store,scope("alice")).unwrap();
    for saved in saved_records {
        let reopened=recipes.load_retained_exact(&saved.package().identity,saved.digest()).await.unwrap().unwrap();
        let summary=reopened.summary().unwrap();
        assert_eq!(summary["kind"],"local_software_selection");
        assert_eq!(summary["declaration_digest"],json!(saved.validated_declaration().digest()));
        assert!(summary.get("manifest_digest").is_none());
        assert!(summary.get("configuration_digest").is_none());
        let RetainedLocalRecipe::Software(actual)=reopened else { panic!("software record"); };
        assert_eq!(actual.package_json(),saved.package_json());
        assert_eq!(actual.declaration_json(),saved.declaration_json());
        assert_eq!(actual.verification(),saved.verification());
        assert!(recipes.load_selection_exact(&saved.package().identity,saved.digest()).await.is_err());
        for other in [scope("bob"),StoreScope {realm:"other".into(),owner:"alice".into()}] {
            assert!(LocalRecipeStore::new(&store,other).unwrap().load_retained_exact(
                &saved.package().identity,saved.digest()).await.unwrap().is_none());
        }
        let mut changed=saved.declaration_json().to_vec(); changed.push(b'\n');
        let replacement=RetainedLocalSoftware::parse(saved.package_json(),&changed,saved.profile(),saved.component().clone(),
            saved.decision().clone(),saved.verification().to_vec()).unwrap();
        assert_ne!(replacement.digest(),saved.digest());
        assert!(matches!(recipes.save_retained_software(replacement).await,Err(LocalRecipeError::ImmutableConflict)));
    }
    assert_eq!(recipes.list_retained().await.unwrap().verified.len(),2);
    assert!(recipes.list_retained().await.unwrap().unverified.is_empty());
}

#[tokio::test]
async fn software_reopen_rechecks_document_kind_digest_scope_and_exact_retention() {
    for change in ["kind","dual-digest","digest","version","null-source","declaration","evidence"] {
        let temp=tempfile::tempdir().unwrap(); let path=temp.path().join("recipes.sqlite");
        let (package,declaration,component)=software_documents(LocalSelectionProfile::Python);
        let software=RetainedLocalSoftware::parse(package,&declaration,LocalSelectionProfile::Python,
            id(component),evidence("decision"),observations()).unwrap();
        {
            let store=SqliteStore::open(&path).unwrap();
            LocalRecipeStore::new(&store,scope("alice")).unwrap().save_retained_software(software.clone()).await.unwrap();
        }
        tamper(&path,|data| match change {
            "kind" => data.body["document_kind"]=json!("activation_manifest"),
            "dual-digest" => data.body["manifest_digest"]=json!("c".repeat(64)),
            "digest" => data.body["declaration_digest"]=json!("c".repeat(64)),
            "version" => data.version=2,
            "null-source" => data.body["plugin_source_json"]=Value::Null,
            "declaration" => data.body["declaration_json"]=json!("{}"),
            "evidence" => data.body["decision"]["record"]=json!("changed"),
            _ => unreachable!(),
        },true);
        let store=SqliteStore::open_existing(&path).unwrap();
        let recipes=LocalRecipeStore::new(&store,scope("alice")).unwrap();
        assert!(recipes.load_retained_exact(&software.package().identity,software.digest()).await.is_err(),"{change}");
        let listing=recipes.list_retained().await.unwrap();
        if change=="evidence" {
            assert_eq!(listing.verified.len(),1);
            assert_ne!(listing.verified[0].digest(),software.digest());
        } else { assert!(listing.verified.is_empty(),"{change}"); }
    }
}

#[tokio::test]
async fn software_unknown_save_outcome_requires_readback_without_blind_retry() {
    let temp=tempfile::tempdir().unwrap(); let path=temp.path().join("recipes.sqlite");
    let (package,declaration,component)=software_documents(LocalSelectionProfile::Python);
    let software=RetainedLocalSoftware::parse(package,&declaration,LocalSelectionProfile::Python,
        id(component),evidence("decision"),observations()).unwrap();
    {
        let faulty=FaultyStore {inner:SqliteStore::open(&path).unwrap(),fault:Fault::AfterCommit};
        assert!(matches!(LocalRecipeStore::new(&faulty,scope("alice")).unwrap()
            .save_retained_software(software.clone()).await,Err(LocalRecipeError::Unconfirmed(_))));
    }
    let store=SqliteStore::open_existing(&path).unwrap();
    let before=store.list_records(&scope("alice"),None,10).await.unwrap();
    let recipes=LocalRecipeStore::new(&store,scope("alice")).unwrap();
    let RetainedLocalRecipe::Software(reopened)=recipes.load_retained_exact(&software.package().identity,
        software.digest()).await.unwrap().unwrap() else { panic!("software record"); };
    assert_eq!(reopened.declaration_json(),declaration);
    assert_eq!(store.list_records(&scope("alice"),None,10).await.unwrap(),before);
    assert_eq!(before.len(),1);
    assert!(store.list_effects(&before[0].key,None,10).await.unwrap().is_empty());
    assert!(matches!(recipes.save_selection(PYTHON_PACKAGE,PYTHON_SELECTION,LocalSelectionProfile::Python,
        id(component),evidence("decision"),observations()).await,Err(LocalRecipeError::ImmutableConflict)));
}
