//! Real source-owner controls; no model, native operation or provider execution.
use super::*;
use crate::original_ingress::tests::{claims, initialized, principal, source};
use crate::private_originals::{
    PrivateOriginals,
    collector_setup::{check_grant, tests as enrollment},
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};
use toolkit_store::{StoreScope, ToolkitStore};

struct Fixture {
    _dir: tempfile::TempDir,
    owner: Arc<ToolkitOriginalOwner>,
    store: Arc<dyn ToolkitStore>,
    originals: PrivateOriginals,
    record: CollectorRecord,
    key: toolkit_store::StoreKey,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let owner = initialized(&dir.path().join("grants"));
        let store: Arc<dyn ToolkitStore> =
            Arc::new(toolkit_store::SqliteStore::open(dir.path().join("source.sqlite3")).unwrap());
        let scope = StoreScope {
            realm: "work".into(),
            owner: "application".into(),
        };
        let originals = PrivateOriginals::prepare(scope.clone(), store.clone(), owner.clone())
            .await
            .unwrap();
        owner
            .select_installed_pair_recovery(store.as_ref(), &scope)
            .unwrap();
        let record = enrollment::record(&owner, &principal("collector"));
        originals
            .persist_collector_setup(record.clone())
            .await
            .unwrap();
        let key = toolkit_store::StoreKey {
            realm: scope.realm,
            owner: scope.owner,
            id: record.id.as_str().into(),
        };
        Self {
            _dir: dir,
            owner,
            store,
            originals,
            record,
            key,
        }
    }
    fn association(&self) -> InputAuthorityAssociation {
        InputAuthorityAssociation::new(self.record.association.clone()).unwrap()
    }
}

// The holder, reader and release coordinator are separate OS threads. The
// reader independently observes WouldBlock while the actual mutex is held.
// Release and both joins precede assertions, including the old early-refusal
// case. This is a fixed fixture bound, not a production wait/retry policy.
fn contended<T: Send, R: Send>(mutex: &Mutex<T>, read: impl FnOnce() -> R + Send) -> R {
    const BOUND: Duration = Duration::from_secs(2);
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let (locked, entered, waited, result, released, holder, reader) = std::thread::scope(|scope| {
        let holder = scope.spawn(move || {
            let guard = mutex.lock().unwrap();
            let announced = locked_tx.send(()).is_ok();
            let released = release_rx.recv_timeout(BOUND).is_ok();
            drop(guard);
            (announced, released)
        });
        let locked = locked_rx.recv_timeout(BOUND);
        let reader = scope.spawn(move || {
            let blocked = matches!(mutex.try_lock(), Err(std::sync::TryLockError::WouldBlock));
            let announced = entered_tx.send(blocked).is_ok();
            let delivered = result_tx.send(read()).is_ok();
            (announced, delivered)
        });
        let entered = entered_rx.recv_timeout(BOUND);
        let early = result_rx.recv_timeout(Duration::from_millis(50));
        let waited = matches!(&early, Err(RecvTimeoutError::Timeout));
        let released = release_tx.send(());
        let result = match early {
            Err(RecvTimeoutError::Timeout) => result_rx.recv_timeout(BOUND),
            result => result,
        };
        (
            locked,
            entered,
            waited,
            result,
            released,
            holder.join(),
            reader.join(),
        )
    });
    assert!(locked.is_ok());
    assert!(
        matches!(entered, Ok(true)),
        "the reader must observe actual contention"
    );
    assert!(released.is_ok());
    assert!(matches!(holder, Ok((true, true))));
    assert!(matches!(reader, Ok((true, true))));
    assert!(
        waited,
        "an in-memory holder must not become a policy refusal"
    );
    result.unwrap_or_else(|_| panic!("source reader did not return after release"))
}

#[tokio::test]
async fn collector_authorization_reads_wait_for_each_actual_collection() {
    let f = Fixture::new().await;
    let before = f.store.get(&f.key).await.unwrap().unwrap();
    let association = f.association();
    for collectors in [false, true] {
        for path in ["record", "enrollment", "disposition"] {
            let read = || match path {
                "record" => f
                    .owner
                    .current_collector_record(&association)
                    .map(|row| row == Some(f.record.clone())),
                "enrollment" => f
                    .owner
                    .enrolled_service_original(
                        &f.record.association.ingress_namespace,
                        &f.record.association.original_work,
                    )
                    .map(|row| row.is_some_and(|row| row.association() == &association)),
                "disposition" => f
                    .owner
                    .original_disposition(
                        &f.record.association.ingress_namespace,
                        &f.record.association.original_work,
                    )
                    .map(|value| matches!(value, OriginalDisposition::Active)),
                _ => unreachable!(),
            };
            let result = if collectors {
                contended(&f.owner.collectors, read)
            } else {
                contended(&f.owner.originals, read)
            };
            assert!(matches!(result, Ok(true)));
        }
    }
    assert!(f.store.get(&f.key).await.unwrap().unwrap() == before);
    assert!(check_grant(&f.owner, &f.record).is_ok());
}

#[tokio::test]
async fn ordinary_original_keeps_precedence_after_contended_collector_probe() {
    let f = Fixture::new().await;
    let grant = f
        .owner
        .grants()
        .issue_root(
            &meerkat_core::AdmissionGuard::none(),
            &principal("admin"),
            EvidenceId::new("ordinary-base").unwrap(),
            principal("admin"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    let mut candidate = claims(f.record.id.as_str(), &grant).candidate().clone();
    candidate.ingress_namespace.realm = f.record.association.ingress_namespace.realm.clone();
    let association = InputAuthorityAssociation::new(candidate).unwrap();
    let original = f
        .owner
        .enroll_new_authenticated(
            association.clone(),
            source(&association),
            &principal("user"),
            &principal("frontend"),
            &principal("admin"),
            EvidenceId::new("ordinary-child").unwrap(),
            ExecutionRestrictions::unrestricted(),
            |_| Ok(None),
        )
        .unwrap();
    let actual = original.association();
    let before = f.store.get(&f.key).await.unwrap().unwrap();
    assert!(matches!(
        contended(&f.owner.originals, || f
            .owner
            .current_collector_record(actual)),
        Ok(None)
    ));
    let c = actual.candidate();
    assert!(matches!(
        contended(&f.owner.originals, || f
            .owner
            .original_disposition(&c.ingress_namespace, &c.original_work)),
        Ok(OriginalDisposition::Active)
    ));
    assert!(f.store.get(&f.key).await.unwrap().unwrap() == before);
}

#[tokio::test]
async fn collector_authorization_reads_keep_real_mutex_poison_unavailable() {
    for collectors in [false, true] {
        let f = Fixture::new().await;
        let before = f.store.get(&f.key).await.unwrap().unwrap();
        let joined = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    if collectors {
                        let _guard = f.owner.collectors.lock().unwrap();
                        panic!("synthetic collector owner unwind");
                    } else {
                        let _guard = f.owner.originals.lock().unwrap();
                        panic!("synthetic original owner unwind");
                    }
                })
                .join()
        });
        assert!(joined.is_err());
        assert!(if collectors {
            f.owner.collectors.is_poisoned()
        } else {
            f.owner.originals.is_poisoned()
        });
        assert!(matches!(
            f.owner.current_collector_record(&f.association()),
            Err(ControllerReadinessFailure::PolicyUnavailable)
        ));
        assert!(matches!(
            f.owner.enrolled_service_original(
                &f.record.association.ingress_namespace,
                &f.record.association.original_work
            ),
            Err(ControllerReadinessFailure::PolicyUnavailable)
        ));
        assert!(matches!(
            f.owner.original_disposition(
                &f.record.association.ingress_namespace,
                &f.record.association.original_work
            ),
            Err(ControllerReadinessFailure::PolicyUnavailable)
        ));
        assert!(f.store.get(&f.key).await.unwrap().unwrap() == before);
    }
}

#[tokio::test]
async fn collector_reads_preserve_exact_identity_pending_fence_and_revocation() {
    let f = Fixture::new().await;
    let before = f.store.get(&f.key).await.unwrap().unwrap();
    let mut changed = f.record.association.clone();
    changed.target.context_generation += 1;
    assert!(matches!(
        f.owner
            .current_collector_record(&InputAuthorityAssociation::new(changed).unwrap()),
        Err(ControllerReadinessFailure::PolicyUnavailable)
    ));
    let mut foreign = f.record.association.ingress_namespace.clone();
    foreign.occurrence_scope = EvidenceId::new("foreign-occurrence").unwrap();
    assert!(
        f.owner
            .enrolled_service_original(&foreign, &f.record.association.original_work)
            .unwrap()
            .is_none()
    );
    {
        let _writer = f.owner.source_writer().await;
        f.owner.fence_collector_original(&f.key).unwrap();
    }
    assert!(matches!(
        f.owner.current_collector_record(&f.association()),
        Err(ControllerReadinessFailure::PolicyUnavailable)
    ));
    assert!(matches!(
        f.owner.original_disposition(
            &f.record.association.ingress_namespace,
            &f.record.association.original_work
        ),
        Err(ControllerReadinessFailure::PolicyUnavailable)
    ));
    assert!(f.store.get(&f.key).await.unwrap().unwrap() == before);
    f.originals
        .revoke_collector_setup(&f.record.id)
        .await
        .unwrap();
    assert!(
        f.owner
            .current_collector_record(&f.association())
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        f.owner
            .original_disposition(
                &f.record.association.ingress_namespace,
                &f.record.association.original_work
            )
            .unwrap(),
        OriginalDisposition::OperationCancelled { .. }
    ));
    assert!(
        check_grant(&f.owner, &f.record).is_ok(),
        "source revocation is not grant mutation"
    );
}
