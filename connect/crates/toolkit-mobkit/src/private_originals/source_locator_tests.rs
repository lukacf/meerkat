use super::*;
use crate::original_ingress::tests::{initialized, private_event, private_route};
use std::os::unix::fs::PermissionsExt;
use toolkit_store::{SqliteStore, StoredEffect, StoredEvent};

async fn source(directory: &std::path::Path) -> Arc<PrivateOriginals> {
    let owner = initialized(&directory.join("grants"));
    let store = Arc::new(SqliteStore::open(directory.join("source.sqlite")).unwrap());
    let scope = StoreScope {
        realm: "test-realm".into(),
        owner: "originals".into(),
    };
    let mut routes = vec![private_route("route")];
    let mut alias = routes[0].clone();
    alias.id = Id::new("alias").unwrap();
    routes.push(alias);
    Arc::new(
        PrivateOriginals::new(scope, store, owner, routes)
            .await
            .unwrap(),
    )
}

async fn snapshot(
    source: &PrivateOriginals,
) -> (Vec<StoredRecord>, Vec<StoredEvent>, Vec<StoredEffect>) {
    let records = source
        .store
        .list_records(&source.scope, None, 16)
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    let events = source
        .store
        .read_events(&records[0].key, 0, 16)
        .await
        .unwrap();
    let effects = source
        .store
        .list_effects(&records[0].key, None, 16)
        .await
        .unwrap();
    (records, events, effects)
}

#[tokio::test]
async fn source_record_locator_preserves_exact_alias_and_scope_without_writes() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let source = source(directory.path()).await;
    let event = private_event("locator");
    let reference = source.retain_event(event.clone()).await.unwrap();
    let mut alias_event = event.clone();
    alias_event.route = Id::new("alias").unwrap();
    let alias = source.retain_event(alias_event).await.unwrap();
    assert!(reference != alias);
    let before = snapshot(&source).await;
    let key = source.source_record_key(&reference).await.unwrap();
    assert_eq!(key, before.0[0].key);
    assert_eq!(source.source_record_key(&reference).await.unwrap(), key);
    assert_eq!(source.source_record_key(&alias).await.unwrap(), key);

    let mut mismatch = reference.clone();
    mismatch.event_sha256 = "f".repeat(64);
    assert_eq!(
        source.source_record_key(&mismatch).await,
        Err(PrivateOriginalError::Conflict)
    );
    let mut missing = reference.clone();
    missing.original = EvidenceId::new("missing").unwrap();
    assert_eq!(
        source.source_record_key(&missing).await,
        Err(PrivateOriginalError::Unavailable)
    );
    let mut unknown_route = reference.clone();
    unknown_route.route = Id::new("unknown-route").unwrap();
    assert_eq!(
        source.source_record_key(&unknown_route).await,
        Err(PrivateOriginalError::Denied)
    );

    // This other owner retains historical events only. Separate owners here
    // test locator scope, not cross-application authority or native admission.
    let foreign_owner = initialized(&directory.path().join("foreign-grants"));
    let foreign = PrivateOriginals::new(
        StoreScope {
            realm: source.scope.realm.clone(),
            owner: "other-originals".into(),
        },
        source.store.clone(),
        foreign_owner,
        source.routes().unwrap().to_vec(),
    )
    .await
    .unwrap();
    assert_eq!(
        foreign.source_record_key(&reference).await,
        Err(PrivateOriginalError::Unavailable)
    );
    // References carry no owner identity. Independently retaining the same
    // exact event makes this other scope's own historical locator valid.
    let foreign_reference = foreign.retain_event(event).await.unwrap();
    assert!(foreign_reference == reference);
    let foreign_before = snapshot(&foreign).await;
    let foreign_key = foreign.source_record_key(&reference).await.unwrap();
    assert_eq!(foreign_key, foreign_before.0[0].key);
    assert_ne!(foreign_key, key);
    assert_eq!(foreign_key.scope(), foreign.scope);
    assert_eq!(
        foreign.source_record_key(&mismatch).await,
        Err(PrivateOriginalError::Conflict)
    );
    assert_eq!(snapshot(&foreign).await, foreign_before);
    assert_eq!(snapshot(&source).await, before);
    assert!(source.binding_evidence(&reference).await.unwrap().is_none());
}

#[tokio::test]
async fn source_record_locator_reads_cancelled_history_without_reopening_authority() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let source = source(directory.path()).await;
    let reference = source
        .retain_event(private_event("cancelled-locator"))
        .await
        .unwrap();
    let key = source.source_record_key(&reference).await.unwrap();
    source.record_operation_cancelled(&reference).await.unwrap();
    let cancelled = snapshot(&source).await;
    let record = source.decode_record(&cancelled.0[0], &reference).unwrap();
    assert!(record.operation_cancelled);
    assert!(record.final_association.is_none());
    assert_eq!(source.source_record_key(&reference).await.unwrap(), key);
    assert_eq!(source.source_record_key(&reference).await.unwrap(), key);
    assert_eq!(snapshot(&source).await, cancelled);
    assert!(source.binding_evidence(&reference).await.unwrap().is_none());
    assert!(matches!(
        source.revoke_original_operation_grant(&reference),
        Err(PrivateOriginalError::Unavailable)
    ));
}
