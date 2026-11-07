//! Passive provenance and real claim checks. No session actor, recovery call,
//! consent request or synthetic reservation is created by these controls.
use super::*;
use meerkat_runtime::{
    MeerkatMachine,
    store::{InMemoryRuntimeStore, RuntimeStore, SqliteRuntimeStore},
};

fn native(root: &Path) -> (Arc<MeerkatMachine>, Arc<dyn RuntimeStore>) {
    fs::create_dir_all(root.join("hosting")).unwrap();
    fs::create_dir_all(root.join("delivery")).unwrap();
    let store: Arc<dyn RuntimeStore> = Arc::new(
        SqliteRuntimeStore::new_head_canonical(root.join("runtime.sqlite3"))
            .unwrap()
            .with_hosting_paths(HostingPaths {
                hosting_lock_dir: root.join("hosting"),
                cold_delivery_lock: root.join("delivery/cold.lock"),
                database: None,
            }),
    );
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store.clone()).unwrap());
    assert!(
        machine.hosting_authority().capability().is_cross_process(),
        "the fixture must actually obtain OS hosting support"
    );
    (machine, store)
}

#[test]
fn consent_provenance_process_local_remains_selected_but_cannot_bind_recovery_claim() {
    let temp = private_root();
    let root = temp.path().canonicalize().unwrap();
    let owner = ApplicationConsentOwner::create(&root, identity()).unwrap();
    let machine =
        MeerkatMachine::persistent_without_blobs(Arc::new(InMemoryRuntimeStore::new())).unwrap();
    let session = meerkat_core::SessionId::new();
    let provenance = owner.resource_provenance(&machine, &session).unwrap();
    assert_eq!(provenance.issuer(), owner.selection());
    assert_eq!(provenance.identity(), owner.identity());
    assert_eq!(provenance.session(), &session);
    assert!(provenance.domain().is_none());
    provenance
        .check_current(&owner, &machine, &session)
        .unwrap();
    let claim = machine.grant_session_hosting(&session).unwrap();
    assert!(!claim.is_cross_process());
    let selected = artifact(
        &root.join("resource.json"),
        b"ordinary selected association",
    );
    assert!(matches!(
        provenance.bind_claim(&owner, &machine, &session, selected, &claim),
        Err(ConsentOwnerError::WrongClaim)
    ));
    assert!(owner.store.load_all().unwrap().is_empty());
}

#[tokio::test]
async fn consent_provenance_reopen_requires_same_issuer_domain_session_and_real_claim_owner() {
    let temp = private_root();
    let root = temp.path().canonicalize().unwrap();
    let (machine, store) = native(&root);
    let session = meerkat_core::SessionId::new();
    let owner = ApplicationConsentOwner::create(&root, identity()).unwrap();
    let provenance = owner.resource_provenance(&machine, &session).unwrap();
    let issuer = owner.selection().clone();
    let selected = artifact(&root.join("resource.json"), &encode(&provenance).unwrap());
    let before = read_artifact(&selected).unwrap();
    let claim = machine.grant_session_hosting(&session).unwrap();
    let first = provenance
        .bind_claim(&owner, &machine, &session, selected.clone(), &claim)
        .unwrap();
    assert_eq!(first.scope().domain, *provenance.domain().unwrap());
    assert_eq!(first.scope().session_id, session);
    assert!(
        matches!(claim.reserve_owner_loss(), OwnerLossAttachment::Reserved(_)),
        "binding neither consumes nor completes the native reservation"
    );
    let other = meerkat_core::SessionId::new();
    let other_claim = machine.grant_session_hosting(&other).unwrap();
    assert!(matches!(
        provenance.bind_claim(&owner, &machine, &other, selected.clone(), &other_claim),
        Err(ConsentOwnerError::WrongClaim)
    ));
    assert!(matches!(
        provenance.bind_claim(&owner, &machine, &session, selected.clone(), &other_claim),
        Err(ConsentOwnerError::WrongClaim)
    ));
    let another = ApplicationConsentOwner::create(&root, identity()).unwrap();
    assert!(matches!(
        provenance.check_current(&another, &machine, &session),
        Err(ConsentOwnerError::InvalidSelection)
    ));
    let mut other_identity = identity();
    other_identity.deployment = Id::new("other").unwrap();
    let foreign = ApplicationConsentOwner::create(&root, other_identity).unwrap();
    assert!(matches!(
        provenance.check_current(&foreign, &machine, &session),
        Err(ConsentOwnerError::WrongApplication)
    ));
    let other_root = root.join("other-native");
    fs::create_dir(&other_root).unwrap();
    let (foreign_machine, _) = native(&other_root);
    assert!(matches!(
        provenance.check_current(&owner, &foreign_machine, &session),
        Err(ConsentOwnerError::WrongClaim)
    ));
    // A second machine on the identical actual store shares domain provenance,
    // but cannot use the first machine's live claim as its own authority.
    let second = MeerkatMachine::persistent_without_blobs(store).unwrap();
    provenance.check_current(&owner, &second, &session).unwrap();
    assert!(matches!(
        provenance.bind_claim(&owner, &second, &session, selected.clone(), &claim),
        Err(ConsentOwnerError::WrongClaim)
    ));
    assert!(
        machine
            .current_session_registration_witness(&session)
            .await
            .is_none()
    );
    assert!(
        second
            .current_session_registration_witness(&session)
            .await
            .is_none()
    );
    assert!(owner.store.load_all().unwrap().is_empty());
    drop(owner);
    let reopened = ApplicationConsentOwner::reopen(issuer, &identity()).unwrap();
    let decoded: ConsentResourceProvenance = decode(&read_artifact(&selected).unwrap()).unwrap();
    assert_eq!(
        decoded
            .bind_claim(&reopened, &machine, &session, selected.clone(), &claim)
            .unwrap(),
        first
    );
    assert_eq!(read_artifact(&selected).unwrap(), before);
    let manifest = reopened.selection().artifact().path.clone();
    let bytes = fs::read(&manifest).unwrap();
    fs::write(&manifest, b"changed").unwrap();
    assert!(
        decoded
            .check_current(&reopened, &machine, &session)
            .is_err()
    );
    fs::write(&manifest, bytes).unwrap();
    decoded
        .check_current(&reopened, &machine, &session)
        .unwrap();
    assert!(reopened.store.load_all().unwrap().is_empty());
}
