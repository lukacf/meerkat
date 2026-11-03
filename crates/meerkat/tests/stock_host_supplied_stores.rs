//! A trusted host's own stores governed by a stock realm
//! (`prepare_stock_host_composition`).
//!
//! The host's stores live at their own paths, apart from the realm's: its
//! runtime database (`host/runtime.db`) is not the realm's own runtime
//! database, so custody that only compared paths would not exclude anyone.
//! The host's one machine holds both the realm's administrative custody and
//! its own store's governed custody: while it lives, the realm's
//! administrative commands, its stock opener and any other host of the realm
//! are refused, and a held custody refuses the host before any governed
//! owner is read. The host selects its posture first; every selection other
//! than the realm's stock composition is refused typed. The bundle the host
//! then constructs keeps the host's own stores and its one approval issuer,
//! durable for a persistent host, with its durable decisions under the
//! host's store path.
#![cfg(all(feature = "session-store", feature = "local-authorization", unix))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;

use meerkat::governed_realm::stock::caller::{
    StockCallerAccess, StockCallerRequest, StockLoginLifetime,
};
use meerkat::governed_realm::stock::issuance::{
    DEFAULT_STOCK_AGENT, StockGrantError, StockGrantRequest, StockGrantTarget, grant_stock_agent,
};
use meerkat::governed_realm::stock::{
    StockGovernance, StockGovernanceError, StockPrincipals, govern_stock_realm,
};
use meerkat::governed_realm::stock_host::{
    StockHostCustody, StockHostError, prepare_stock_host_composition,
};
use meerkat::governed_realm::{
    RealmGovernance, open_governed_realm_persistence_with_provider, open_stock_realm_persistence_as,
};
use meerkat::storage_provider::RealmStoreSet;
use meerkat::{ApprovalIssuerSelection, ApprovalIssuerSelectionError, DiskStorageProvider};
use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionKind, DecisionOperationId, DecisionPrincipalKey,
    DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionSubjectKey, Sha256Digest,
};
use meerkat_core::connection::RealmId;
use meerkat_core::{
    ApprovalListFilter, DurabilityDeclaration, DurabilityResolution, OperationAuthorizationError,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_runtime::MeerkatMachine;
use meerkat_runtime::decision::{DecisionError, DecisionWriteMode};
use meerkat_runtime::store::RuntimeStoreExecutionCustodyError;
use meerkat_store::RealmBackend;
use meerkat_store::approval_decision_store::APPROVAL_DECISIONS_FILE;

const REALM: &str = "stock-host";
const OTHER_REALM: &str = "stock-host-ordinary";

fn realm(id: &str) -> RealmId {
    RealmId::parse(id).expect("realm id")
}

/// The host admits every caller; the custody tests bind none.
struct AdmitAll;

impl StockCallerAccess for AdmitAll {
    fn authorize_caller(
        &self,
        _request: &StockCallerRequest<'_>,
    ) -> Result<StockLoginLifetime, OperationAuthorizationError> {
        Ok(StockLoginLifetime::UntilRetired)
    }
}

/// The host's stock composition of `id`: fresh for each preparation.
fn stock_for(id: &str) -> Arc<StockGovernance> {
    let principals = StockPrincipals::for_local_operator(&realm(id)).expect("local operator");
    StockGovernance::host_authenticated_for_agent(
        realm(id),
        principals,
        DEFAULT_STOCK_AGENT,
        Arc::new(AdmitAll),
    )
    .expect("the host's stock composition")
}

fn catalog_grant() -> StockGrantRequest {
    StockGrantRequest {
        agent: DEFAULT_STOCK_AGENT.to_string(),
        target: StockGrantTarget::Catalog,
        expires_in_ms: None,
    }
}

/// Materialize `id` as an ordinary SQLite realm, then drop its open.
async fn materialize(root: &Path, id: &str) {
    drop(
        open_governed_realm_persistence_with_provider(
            &DiskStorageProvider,
            root,
            id,
            Some(RealmBackend::Sqlite),
            None,
            None,
            RealmGovernance::Ungoverned,
        )
        .await
        .expect("the realm materializes"),
    );
}

/// Materialize and govern the realm as the local operator, and issue and
/// select the default agent's catalog grant through stock issuance.
async fn provision(root: &Path) {
    materialize(root, REALM).await;
    let principals = StockPrincipals::for_local_operator(&realm(REALM)).expect("local operator");
    govern_stock_realm(root, REALM, &principals)
        .await
        .expect("the operator governs the realm");
    grant_stock_agent(root, REALM, &principals, &catalog_grant())
        .await
        .expect("the operator issues and selects the catalog grant");
}

/// An administrative command of the realm: a fresh catalog grant.
async fn administer(root: &Path) -> Result<(), StockGrantError> {
    let principals = StockPrincipals::for_local_operator(&realm(REALM)).expect("local operator");
    grant_stock_agent(root, REALM, &principals, &catalog_grant())
        .await
        .map(drop)
}

/// The host's own stores under `dir`: its session store and its separate
/// runtime database, as a host keeps them, apart from the realm's.
fn host_stores(dir: &Path) -> RealmStoreSet {
    std::fs::create_dir_all(dir).expect("host store directory");
    let durable =
        |domain: &str| DurabilityDeclaration::durable(domain, DurabilityResolution::Persistent);
    let ephemeral = |domain: &str| {
        DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
    };
    RealmStoreSet {
        session_store: Arc::new(
            meerkat_store::SqliteSessionStore::open(dir.join("sessions.sqlite3"))
                .expect("host session store"),
        ),
        runtime_store: Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(dir.join("runtime.db"))
                .expect("host runtime store"),
        ),
        schedule_store: Arc::new(meerkat_schedule::MemoryScheduleStore::new()),
        workgraph_store: Arc::new(meerkat_workgraph::MemoryWorkGraphStore::new()),
        job_store: Arc::new(
            meerkat_jobs::SqliteDetachedJobStore::open(dir.join("jobs.sqlite3"))
                .expect("host job store"),
        ),
        blob_store: Arc::new(meerkat_store::FsBlobStore::new(dir.join("blobs"))),
        artifact_store: Arc::new(meerkat_store::FsArtifactStore::new(dir.join("artifacts"))),
        store_path: dir.to_path_buf(),
        projection_root: None,
        durability: vec![
            durable("sessions"),
            durable("runtime"),
            ephemeral("schedule"),
            ephemeral("workgraph"),
            durable("jobs"),
            durable("blobs"),
            durable("artifacts"),
        ],
    }
}

/// An explicitly ephemeral host: every store process-local and declared so.
fn ephemeral_host_stores(dir: &Path) -> RealmStoreSet {
    let ephemeral = |domain: &str| {
        DurabilityDeclaration::durable(domain, DurabilityResolution::DeclaredEphemeral)
    };
    RealmStoreSet {
        session_store: Arc::new(meerkat_store::MemoryStore::new()),
        runtime_store: Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new()),
        schedule_store: Arc::new(meerkat_schedule::MemoryScheduleStore::new()),
        workgraph_store: Arc::new(meerkat_workgraph::MemoryWorkGraphStore::new()),
        job_store: Arc::new(meerkat_jobs::MemoryDetachedJobStore::new()),
        blob_store: Arc::new(meerkat_store::MemoryBlobStore::new()),
        artifact_store: Arc::new(meerkat_store::MemoryArtifactStore::new()),
        store_path: dir.to_path_buf(),
        projection_root: None,
        durability: [
            "sessions",
            "runtime",
            "schedule",
            "workgraph",
            "jobs",
            "blobs",
            "artifacts",
        ]
        .iter()
        .map(|domain| ephemeral(domain))
        .collect(),
    }
}

fn custody(root: &Path) -> StockHostCustody {
    StockHostCustody::new(root, REALM).expect("the association")
}

fn custody_busy(error: &StockHostError) -> bool {
    matches!(
        error,
        StockHostError::RealmCustody {
            source: RuntimeStoreExecutionCustodyError::Busy,
            ..
        }
    )
}

/// While the host's machine lives, it holds the realm: the realm's
/// administrative commands, its stock opener and a second host with its own
/// stores elsewhere are refused. Once the host releases it, the realm is
/// administered again.
#[tokio::test]
async fn the_host_machine_holds_the_realms_administrative_custody() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;

    let prepared = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock_for(REALM)),
        host_stores(&temp.path().join("host")),
    )
    .await
    .expect("the host's composition is prepared");

    assert!(
        matches!(
            administer(&root).await,
            Err(StockGrantError::Refused(StockGovernanceError::Persistence(
                _
            )))
        ),
        "an administrative command is refused while the host holds the realm"
    );
    assert!(
        open_stock_realm_persistence_as(
            &DiskStorageProvider,
            &root,
            REALM,
            None,
            None,
            None,
            DEFAULT_STOCK_AGENT,
        )
        .await
        .is_err(),
        "the stock opener is refused while the host holds the realm"
    );
    let second = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock_for(REALM)),
        host_stores(&temp.path().join("second-host")),
    )
    .await
    .expect_err("a second host of the realm is refused");
    assert!(custody_busy(&second), "{second:?}");

    drop(prepared);
    administer(&root)
        .await
        .expect("the realm is administered once the host released it");
}

/// A held administrative custody refuses the host promptly, before any
/// governed owner is read; released, the host prepares.
#[tokio::test]
async fn a_held_administrative_custody_refuses_the_host_before_any_owner_is_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    let held = open_governed_realm_persistence_with_provider(
        &DiskStorageProvider,
        &root,
        REALM,
        None,
        None,
        None,
        RealmGovernance::GovernedWithoutOwners,
    )
    .await
    .expect("an administrative custody of the realm");

    let stock = stock_for(REALM);
    let refused = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(Arc::clone(&stock)),
        host_stores(&temp.path().join("host")),
    )
    .await
    .expect_err("the host is refused while the realm is held");
    assert!(custody_busy(&refused), "{refused:?}");

    drop(held);
    // No governed owner was read for the refused host: the same
    // composition is still fresh, and prepares once the realm is released.
    let prepared = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(Arc::clone(&stock)),
        host_stores(&temp.path().join("host")),
    )
    .await
    .expect("the host prepares once the realm is released");
    assert!(Arc::ptr_eq(prepared.stock_governance(), &stock));
}

/// The host's own runtime store already has a governed owner: the host is
/// refused, and the realm custody it took first is released with the
/// refusal, so the realm is administered at once.
#[tokio::test]
async fn a_governed_owner_of_the_host_store_refuses_the_host_and_releases_the_realm() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    let stores = host_stores(&temp.path().join("host"));
    let other_owner =
        MeerkatMachine::persistent_governed_staged(Arc::clone(&stores.runtime_store), None)
            .expect("another governed owner of the host store");

    let refused = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock_for(REALM)),
        stores,
    )
    .await
    .expect_err("the host's store is already governed");
    assert!(
        matches!(refused, StockHostError::Persistence(_)),
        "{refused:?}"
    );
    administer(&root)
        .await
        .expect("the refused host kept no realm custody");
    drop(other_owner);
}

/// The host selects its posture before anything is prepared: no
/// governance, governance without owners, a composition of another realm
/// and an ungoverned realm are each refused typed, and none of them takes
/// any custody.
#[tokio::test]
async fn the_host_selects_its_posture_before_anything_is_prepared() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    materialize(&root, OTHER_REALM).await;
    let stores = || host_stores(&temp.path().join("host"));

    let ungoverned =
        prepare_stock_host_composition(custody(&root), RealmGovernance::Ungoverned, stores())
            .await
            .expect_err("an ordinary selection prepares nothing");
    assert!(
        matches!(ungoverned, StockHostError::Ungoverned { .. }),
        "{ungoverned:?}"
    );
    let without_owners = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::GovernedWithoutOwners,
        stores(),
    )
    .await
    .expect_err("governance without owners prepares nothing");
    assert!(
        matches!(without_owners, StockHostError::GovernedWithoutOwners { .. }),
        "{without_owners:?}"
    );
    let mismatched = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock_for(OTHER_REALM)),
        stores(),
    )
    .await
    .expect_err("another realm's composition is refused");
    assert!(
        matches!(mismatched, StockHostError::RealmMismatch { .. }),
        "{mismatched:?}"
    );
    let not_governed = prepare_stock_host_composition(
        StockHostCustody::new(&root, OTHER_REALM).expect("the association"),
        RealmGovernance::Stock(stock_for(OTHER_REALM)),
        stores(),
    )
    .await
    .expect_err("an ungoverned realm never runs a stock host");
    assert!(
        matches!(not_governed, StockHostError::NotGoverned { .. }),
        "{not_governed:?}"
    );

    administer(&root)
        .await
        .expect("no refused selection took the realm's custody");
}

/// Each preparation takes a fresh composition: one already composed for a
/// machine is refused, so no earlier selection is reused.
#[tokio::test]
async fn a_composition_is_prepared_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    let stock = stock_for(REALM);
    drop(
        prepare_stock_host_composition(
            custody(&root),
            RealmGovernance::Stock(Arc::clone(&stock)),
            host_stores(&temp.path().join("host")),
        )
        .await
        .expect("the first preparation"),
    );
    let reused = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock),
        host_stores(&temp.path().join("host")),
    )
    .await
    .expect_err("a composed composition is not prepared again");
    assert!(
        matches!(reused, StockHostError::CompositionReused { .. }),
        "{reused:?}"
    );
}

/// The host's bundle keeps the host's own stores and carries its one
/// durable approval issuer, opened from its selection; while the bundle's
/// machine lives the realm stays held, and dropping it releases the realm.
#[tokio::test]
async fn the_bundle_keeps_the_hosts_stores_and_its_one_durable_issuer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    let host = temp.path().join("host");
    let stores = host_stores(&host);
    let (job_store, artifact_store, runtime_store) = (
        Arc::clone(&stores.job_store),
        Arc::clone(&stores.artifact_store),
        Arc::clone(&stores.runtime_store),
    );
    let stock = stock_for(REALM);
    let prepared = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(Arc::clone(&stock)),
        stores,
    )
    .await
    .expect("the host's composition is prepared");
    let (pin, bundle) = prepared
        .construct(ApprovalIssuerSelection::Persistent {
            store_path: host.clone(),
        })
        .expect("the host's bundle");

    assert_eq!(pin.realm().as_str(), REALM);
    assert!(
        Arc::ptr_eq(&bundle.job_store(), &job_store),
        "the host's jobs"
    );
    assert!(
        Arc::ptr_eq(&bundle.artifact_store(), &artifact_store),
        "the host's artifacts"
    );
    assert!(
        Arc::ptr_eq(&bundle.runtime_store(), &runtime_store),
        "the host's runtime store"
    );
    assert!(
        bundle
            .stock_governance()
            .is_some_and(|carried| Arc::ptr_eq(carried, &stock)),
        "the bundle carries the prepared stock composition"
    );
    let issuer = bundle.approval_issuer();
    assert!(
        issuer.is_persistent(),
        "a persistent host's approvals are durable"
    );
    assert!(
        issuer.list(ApprovalListFilter::default()).is_ok(),
        "the issuer is available"
    );
    assert!(bundle.clone().approval_issuer().is_persistent());

    assert!(
        administer(&root).await.is_err(),
        "the bundle's machine holds the realm"
    );
    drop(issuer);
    drop(bundle);
    administer(&root)
        .await
        .expect("dropping the bundle released the realm");
}

/// A handle sharing the machine's custody (its provider-auth pair) keeps the
/// realm held after the bundle is dropped; its last holder releases it.
#[tokio::test]
async fn a_handle_sharing_the_machines_custody_keeps_the_realm_held() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    let host = temp.path().join("host");
    let prepared = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock_for(REALM)),
        host_stores(&host),
    )
    .await
    .expect("the host's composition is prepared");
    let (_, bundle) = prepared
        .construct(ApprovalIssuerSelection::Persistent { store_path: host })
        .expect("the host's bundle");
    let auth = bundle.runtime_adapter().provider_auth_runtime_authority();
    drop(bundle);
    assert!(
        administer(&root).await.is_err(),
        "the provider-auth pair still shares the realm's custody"
    );
    drop(auth);
    administer(&root)
        .await
        .expect("the last holder released the realm");
}

/// A host whose runtime store is the realm's own runtime database, by its
/// own path or by an alias, is refused typed: the host's machine would
/// admit itself. The realm is released with the refusal.
#[tokio::test]
async fn a_host_store_on_the_realms_own_database_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    let database = meerkat_store::realm_paths_in(&root, REALM).sessions_sqlite_path;
    let alias = temp.path().join("alias.sqlite3");
    std::os::unix::fs::symlink(&database, &alias).expect("an alias of the realm's database");
    for path in [database, alias] {
        let mut stores = host_stores(&temp.path().join("host"));
        let session_store = Arc::new(
            meerkat_store::SqliteSessionStore::open(path.clone()).expect("the realm's sessions"),
        );
        stores.runtime_store = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(path)
                .expect("the realm's runtime database"),
        );
        stores.session_store = session_store;
        let refused = prepare_stock_host_composition(
            custody(&root),
            RealmGovernance::Stock(stock_for(REALM)),
            stores,
        )
        .await
        .expect_err("the host's store is the realm's own database");
        assert!(
            matches!(refused, StockHostError::HostStoreIsRealmStore { .. }),
            "{refused:?}"
        );
        administer(&root)
            .await
            .expect("the refused host released the realm");
    }
}

/// A persistent host's approvals are durable: without a store path it has no
/// selection at all, and an ephemeral selection is refused, releasing the
/// realm. An explicitly ephemeral host keeps process-local approvals.
#[tokio::test]
async fn a_persistent_host_needs_a_durable_approval_selection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    assert_eq!(
        ApprovalIssuerSelection::for_host(true, None),
        Err(ApprovalIssuerSelectionError::PersistentWithoutStorePath),
        "a persistent host without a store path has no selection"
    );

    let prepared = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock_for(REALM)),
        host_stores(&temp.path().join("host")),
    )
    .await
    .expect("the persistent host's composition is prepared");
    let Err(refused) = prepared.construct(ApprovalIssuerSelection::Ephemeral) else {
        panic!("a persistent host's approvals are never ephemeral");
    };
    assert!(
        matches!(refused, StockHostError::EphemeralApprovals { .. }),
        "{refused:?}"
    );
    administer(&root)
        .await
        .expect("the refused host released the realm");

    let prepared = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock_for(REALM)),
        ephemeral_host_stores(&temp.path().join("ephemeral-host")),
    )
    .await
    .expect("the explicitly ephemeral host's composition is prepared");
    let selection =
        ApprovalIssuerSelection::for_host(false, None).expect("an ephemeral host's selection");
    let (_, bundle) = prepared
        .construct(selection)
        .expect("an explicitly ephemeral host keeps process-local approvals");
    assert!(!bundle.approval_issuer().is_persistent());
}

/// One durable decision of the host.
fn hold_request() -> DecisionHoldRequest {
    let requester = PrincipalRef::in_domain(
        PrincipalKind::PersonalAgent,
        "builder",
        TrustDomainId::new("example.org").expect("trust domain"),
    )
    .expect("principal");
    DecisionHoldRequest::new(
        DecisionKind::new("release").expect("kind"),
        DecisionSubjectKey::new("artifact:build-7").expect("subject key"),
        DecisionOperationId::new("operation-7").expect("operation id"),
        Sha256Digest::of(br#"{"build":7}"#),
        DecisionRequestBody::from_value(&serde_json::json!({"build": 7})).expect("request body"),
        DecisionPrincipalKey::of(&requester).expect("principal key"),
        ApproverId::new("team:owners").expect("approver id"),
        u64::MAX / 2,
        DecisionRendering::new("Release build 7.").expect("rendering"),
        DecisionRenderer::new("render.v1").expect("renderer"),
    )
}

/// Whether a file named `name` exists anywhere under `dir`.
fn tree_holds(dir: &Path, name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            tree_holds(&path, name)
        } else {
            entry.file_name() == name
        }
    })
}

/// A persistent host's durable decisions live under its own store path,
/// beside its approvals, never under the realm's root; the bundle's owner
/// narrows to a candidate launch one way.
#[tokio::test]
async fn host_supplied_stores_keep_decisions_under_the_store_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("realms");
    provision(&root).await;
    let host = temp.path().join("host");
    let prepared = prepare_stock_host_composition(
        custody(&root),
        RealmGovernance::Stock(stock_for(REALM)),
        host_stores(&host),
    )
    .await
    .expect("the host's composition is prepared");
    let (_pin, bundle) = prepared
        .construct(ApprovalIssuerSelection::Persistent {
            store_path: host.clone(),
        })
        .expect("the host's bundle");

    let decisions = bundle.decision_owner();
    assert!(decisions.is_persistent());
    assert_eq!(decisions.write_mode(), DecisionWriteMode::Admitted);
    let file = host.join(APPROVAL_DECISIONS_FILE);
    assert!(!file.exists(), "nothing is created before the first hold");
    let held = decisions.hold(hold_request(), true).expect("hold");
    assert!(
        file.is_file(),
        "the decision lives under the host's store path"
    );
    assert!(
        !tree_holds(&root, APPROVAL_DECISIONS_FILE),
        "nothing under the realm's root"
    );
    assert_eq!(
        bundle
            .decision_owner()
            .get(&held.decision_id)
            .expect("one owner across clones"),
        held
    );

    let candidate = bundle.with_candidate_launch_decisions();
    assert_eq!(
        candidate.decision_owner().write_mode(),
        DecisionWriteMode::CandidateLaunch
    );
    assert!(matches!(
        candidate.decision_owner().hold(hold_request(), true),
        Err(DecisionError::CandidateLaunch)
    ));
    assert_eq!(
        candidate
            .decision_owner()
            .get(&held.decision_id)
            .expect("a candidate reads"),
        held
    );
}
