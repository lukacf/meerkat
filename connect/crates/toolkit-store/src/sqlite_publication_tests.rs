//! Actual SQLite transaction/custody controls. The local observer records only
//! guard lifetime; it makes no Toolkit or native authorization decision.
use super::*;
use std::sync::{
    Condvar,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Publication {
    active: AtomicBool,
    changes: AtomicUsize,
    released: tokio::sync::Notify,
}
struct Guard<'a>(&'a Publication);
impl RecordChangeGuard for Guard<'_> {}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        assert!(self.0.active.swap(false, Ordering::SeqCst));
        self.0.released.notify_one();
    }
}
impl RecordChangePublication for Publication {
    fn begin_record_change(&self) -> StoreResult<Box<dyn RecordChangeGuard + '_>> {
        assert!(!self.active.swap(true, Ordering::SeqCst));
        self.changes.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Guard(self)))
    }
}
fn request() -> AdmissionRequest {
    AdmissionRequest {
        key: StoreKey {
            realm: "realm".into(),
            owner: "owner".into(),
            id: "record".into(),
        },
        admission_key: "record".into(),
        data: RecordData {
            schema: "test.publication".into(),
            version: 1,
            body: serde_json::json!({ "value": 1 }),
        },
        events: vec![],
        effects: vec![],
    }
}
fn transition(prior: &StoredRecord) -> Transition {
    Transition {
        key: prior.key.clone(),
        expected_revision: prior.revision,
        expected_digest: Some(prior.content_digest.clone()),
        claim: None,
        data: RecordData {
            body: serde_json::json!({ "value": 2 }),
            ..prior.data.clone()
        },
        events: vec![],
        effects: vec![],
    }
}

#[test]
fn installed_publication_custody_excludes_handles_and_inode_aliases_until_last_clone() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let first = SqliteStore::open(&path).unwrap();
    let second = SqliteStore::open_existing(&path).unwrap();
    let publication: Arc<dyn RecordChangePublication> = Arc::new(Publication::default());
    assert!(matches!(
        SqliteStore::open_existing_published(&path, publication.clone()),
        Err(StoreError::OwnerUnavailable)
    ));
    drop(first);
    drop(second);
    let owner = SqliteStore::open_existing_published(&path, publication.clone()).unwrap();
    assert!(owner.record_publication_is(&publication));
    let other: Arc<dyn RecordChangePublication> = Arc::new(Publication::default());
    assert!(!owner.record_publication_is(&other));
    assert!(matches!(
        SqliteStore::open_existing(&path),
        Err(StoreError::OwnerUnavailable)
    ));
    #[cfg(unix)]
    {
        let alias = directory.path().join("alias.sqlite");
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(matches!(
            SqliteStore::open_existing(&alias),
            Err(StoreError::OwnerUnavailable)
        ));
        let symbolic = directory.path().join("symbolic.sqlite");
        std::os::unix::fs::symlink(&path, &symbolic).unwrap();
        assert!(matches!(
            SqliteStore::open_existing(&symbolic),
            Err(StoreError::OwnerUnavailable)
        ));
        std::fs::remove_file(alias).unwrap();
        std::fs::remove_file(symbolic).unwrap();
    }
    let retained = owner.clone();
    drop(owner);
    assert!(matches!(
        SqliteStore::open_existing(&path),
        Err(StoreError::OwnerUnavailable)
    ));
    drop(retained);
    assert!(SqliteStore::open_existing(&path).is_ok());
}

struct Release(Arc<(Mutex<bool>, Condvar)>);
impl Drop for Release {
    fn drop(&mut self) {
        let (lock, ready) = &*self.0;
        *lock.lock().unwrap() = true;
        ready.notify_all();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandoned_sqlite_write_retains_publication_and_physical_owner_through_commit_or_rollback()
{
    for rollback in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let initial = SqliteStore::open(&path).unwrap();
        let prior = initial.admit(request()).await.unwrap().record().clone();
        drop(initial);
        let publication = Arc::new(Publication::default());
        let owner = SqliteStore::open_existing_published(&path, publication.clone()).unwrap();
        let block = Arc::new((Mutex::new(false), Condvar::new()));
        // Always release the detached blocking worker, including after an assertion panic.
        let release = Release(block.clone());
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let entered = Mutex::new(Some(entered));
        let observed = publication.clone();
        owner
            .with(move |connection, _| {
                connection.create_scalar_function(
                    "hold_update",
                    0,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                    move |_| {
                        assert!(observed.active.load(Ordering::SeqCst));
                        if let Some(entered) = entered.lock().unwrap().take() {
                            let _ = entered.send(());
                        }
                        let (lock, ready) = &*block;
                        let mut released = lock.lock().unwrap();
                        while !*released {
                            released = ready.wait(released).unwrap();
                        }
                        assert!(observed.active.load(Ordering::SeqCst));
                        Ok(1_i64)
                    },
                )?;
                connection.execute_batch(if rollback {
                    "CREATE TRIGGER hold_write BEFORE UPDATE ON toolkit_records BEGIN
                 SELECT hold_update(); SELECT RAISE(ABORT, 'rollback control'); END;"
                } else {
                    "CREATE TRIGGER hold_write BEFORE UPDATE ON toolkit_records BEGIN
                 SELECT hold_update(); END;"
                })?;
                Ok(())
            })
            .await
            .unwrap();
        let writer = owner.clone();
        let change = transition(&prior);
        let caller = tokio::spawn(async move { writer.compare_and_swap(change).await });
        tokio::time::timeout(Duration::from_secs(10), waiting)
            .await
            .unwrap()
            .unwrap();
        assert!(publication.active.load(Ordering::SeqCst));
        // The real WAL reader sees the old row until the blocked transaction ends.
        let observer = Connection::open(&path).unwrap();
        assert_eq!(
            read_record(&observer, &prior.key).unwrap(),
            Some(prior.clone())
        );
        drop(observer);
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        drop(owner);
        assert!(
            matches!(
                SqliteStore::open_existing(&path),
                Err(StoreError::OwnerUnavailable)
            ),
            "the detached worker still owns the same connection and file"
        );
        drop(release);
        tokio::time::timeout(Duration::from_secs(10), publication.released.notified())
            .await
            .unwrap();
        let reopened = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match SqliteStore::open_existing(&path) {
                    Ok(store) => break store,
                    Err(StoreError::OwnerUnavailable) => tokio::task::yield_now().await,
                    Err(error) => panic!("unexpected reopen failure: {error}"),
                }
            }
        })
        .await
        .unwrap();
        assert!(!publication.active.load(Ordering::SeqCst));
        assert_eq!(publication.changes.load(Ordering::SeqCst), 1);
        let actual = reopened.get(&prior.key).await.unwrap().unwrap();
        if rollback {
            assert_eq!(actual, prior);
        } else {
            assert_eq!(actual.revision, prior.revision + 1);
            assert_eq!(actual.data.body["value"], 2);
        }
    }
}

#[tokio::test]
async fn conflicting_cas_does_not_publish_but_failed_actual_update_invalidates_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let initial = SqliteStore::open(&path).unwrap();
    let prior = initial.admit(request()).await.unwrap().record().clone();
    drop(initial);
    let publication = Arc::new(Publication::default());
    let owner = SqliteStore::open_existing_published(&path, publication.clone()).unwrap();
    let mut conflict = transition(&prior);
    conflict.expected_revision += 1;
    assert!(matches!(
        owner.compare_and_swap(conflict).await,
        Err(StoreError::Conflict)
    ));
    assert_eq!(publication.changes.load(Ordering::SeqCst), 0);
    owner
        .with(|connection, _| {
            connection.execute_batch(
                "CREATE TRIGGER reject_update BEFORE UPDATE ON toolkit_records
            BEGIN SELECT RAISE(ABORT, 'rollback control'); END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(owner.compare_and_swap(transition(&prior)).await.is_err());
    assert_eq!(publication.changes.load(Ordering::SeqCst), 1);
    assert!(!publication.active.load(Ordering::SeqCst));
    assert_eq!(owner.get(&prior.key).await.unwrap(), Some(prior));
}
