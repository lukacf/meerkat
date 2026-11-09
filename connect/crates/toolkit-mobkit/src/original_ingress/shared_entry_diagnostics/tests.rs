//! Source-owner diagnostics, not installed/native workflow qualification.
use super::*;
use crate::original_ingress::tests::{claims, initialized, principal, source};
use crate::original_ingress::{RetainedToolkitOriginal, ToolkitOriginalOwner};
use futures::FutureExt;
use std::{os::unix::fs::PermissionsExt, panic::AssertUnwindSafe, sync::Arc};
use toolkit_store::{AdmissionRequest, RecordData, StoreKey, StoreScope, ToolkitStore};

struct Fixture {
    _dir: tempfile::TempDir,
    owner: Arc<ToolkitOriginalOwner>,
    store: Arc<dyn ToolkitStore>,
    scope: StoreScope,
    root: meerkat_authorization_contracts::grant::GrantLineageRef,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let owner = initialized(&dir.path().join("grants"));
        let root = owner
            .grants
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &principal("admin"),
                meerkat_authorization_contracts::evidence::EvidenceId::new("diagnostic-root")
                    .unwrap(),
                principal("admin"),
                None,
                meerkat_authorization_contracts::constraints::ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        let store: Arc<dyn ToolkitStore> =
            Arc::new(toolkit_store::SqliteStore::open(dir.path().join("sources.sqlite")).unwrap());
        let scope = StoreScope {
            realm: "test-realm".into(),
            owner: "diagnostic-source".into(),
        };
        owner.bind_canonical_source(&store, &scope).unwrap();
        Self {
            _dir: dir,
            owner,
            store,
            scope,
            root,
        }
    }
    fn admit(&self, label: &str) -> Arc<RetainedToolkitOriginal> {
        let association = claims(label, &self.root);
        let original = self
            .owner
            .enroll_new_authenticated(
                association.clone(),
                source(&association),
                &principal("user"),
                &principal("frontend"),
                &principal("admin"),
                meerkat_authorization_contracts::evidence::EvidenceId::new(label).unwrap(),
                meerkat_authorization_contracts::constraints::ExecutionRestrictions::unrestricted(),
                |_| Ok(None),
            )
            .unwrap();
        self.owner
            .record_candidate(&original)
            .unwrap()
            .admit()
            .unwrap();
        original
    }
    async fn row(&self, label: &str) -> Arc<toolkit_store::StoredRecord> {
        Arc::new(
            self.store
                .admit(AdmissionRequest {
                    key: StoreKey {
                        realm: self.scope.realm.clone(),
                        owner: self.scope.owner.clone(),
                        id: label.into(),
                    },
                    admission_key: label.into(),
                    data: RecordData {
                        schema: "test.followup-diagnostic".into(),
                        version: 1,
                        body: serde_json::json!({}),
                    },
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap()
                .record()
                .clone(),
        )
    }
}
fn correlation(label: &str, association: &InputAuthorityAssociation) -> Correlation {
    let args =
        serde_json::value::RawValue::from_string("{\"secret\":\"must-not-log\"}".into()).unwrap();
    Correlation::new(
        meerkat_core::ToolCallView {
            id: label,
            name: "setup_job",
            args: &args,
        },
        &StoreKey {
            realm: "test-realm".into(),
            owner: "captured-acquisition".into(),
            id: label.into(),
        },
        &crate::private_originals::PrivateOriginalRef {
            route: toolkit_package::Id::new(label).unwrap(),
            original: meerkat_authorization_contracts::evidence::EvidenceId::new(label).unwrap(),
            event_sha256: "e".repeat(64),
        },
        association,
    )
    .unwrap()
}
fn refusal(events: &[serde_json::Value], cause: &str, raw: &str) {
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["stage"], "followup.parent_current");
    assert_eq!(events[0]["parent_cause"], cause);
    assert_eq!(events[0]["raw_error"], raw);
    assert_eq!(events[0]["publish_callback_entered"], false);
    assert_eq!(
        events[0]["parent_binding"],
        events[0]["observed_parent_binding"]
    );
}

#[tokio::test]
async fn real_mutex_contention_and_poison_keep_unavailable_and_distinct_causes() {
    for poison in [false, true] {
        let f = Fixture::new();
        let a = f.admit("a");
        if poison {
            assert!(
                std::panic::catch_unwind(AssertUnwindSafe(|| {
                    let _guard = f.owner.originals.lock().unwrap();
                    panic!("synthetic existing mutex owner unwind");
                }))
                .is_err()
            );
        }
        let guard = if poison {
            None
        } else {
            Some(f.owner.originals.lock().unwrap())
        };
        let called = Cell::new(0);
        let (result, events) = capture(
            correlation("a", a.association()),
            f.owner.retain_followup_source(a.association(), || async {
                called.set(called.get() + 1);
                Ok(())
            }),
        )
        .await;
        drop(guard);
        assert_eq!(result, Err(OriginalWriterError::Unavailable));
        assert_eq!(called.get(), 0);
        refusal(
            &events,
            if poison {
                "originals_poisoned"
            } else {
                "originals_would_block"
            },
            "unavailable",
        );
        assert_eq!(f.owner.originals.is_poisoned(), poison);
    }
}

#[tokio::test]
async fn real_cancel_pending_then_cancelled_a_keeps_b_separate() {
    let f = Fixture::new();
    let a = f.admit("a");
    let b = f.admit("b");
    let row = f.row("cancel-a").await;
    let c = a.association().candidate();
    // Real owner transition to Pending via an indeterminate persistence result.
    assert_eq!(
        f.owner
            .record_operation_cancelled(
                &c.ingress_namespace.realm,
                &c.original_work,
                |_, _| async { Ok(row.clone()) },
                |_| async { Err(OriginalWriterError::Unavailable) }
            )
            .await,
        Err(OriginalWriterError::Unavailable)
    );
    let called = Cell::new(0);
    let (result, pending) = capture(
        correlation("a", a.association()),
        f.owner.retain_followup_source(a.association(), || async {
            called.set(called.get() + 1);
            Ok(())
        }),
    )
    .await;
    assert_eq!(result, Err(OriginalWriterError::Unavailable));
    refusal(&pending, "cancellation_pending", "unavailable");
    f.owner
        .record_operation_cancelled(
            &c.ingress_namespace.realm,
            &c.original_work,
            |_, _| async { Ok(row.clone()) },
            |row| async { Ok(row) },
        )
        .await
        .unwrap();
    let (result, cancelled) = capture(
        correlation("a", a.association()),
        f.owner.retain_followup_source(a.association(), || async {
            called.set(called.get() + 1);
            Ok(())
        }),
    )
    .await;
    assert_eq!(result, Err(OriginalWriterError::OperationCancelled));
    refusal(&cancelled, "cancelled", "operation_cancelled");
    assert_eq!(called.get(), 0);
    let ca = correlation("a", a.association());
    let cb = correlation("b", b.association());
    assert_ne!(ca.acquisition, cb.acquisition);
    assert_ne!(ca.parent, cb.parent);
    assert_ne!(ca.parent_binding, cb.parent_binding);
    let (result, events) = capture(
        cb,
        f.owner.retain_followup_source(b.association(), || async {
            called.set(called.get() + 1);
            Ok(())
        }),
    )
    .await;
    assert_eq!(result, Ok(()));
    assert_eq!(called.get(), 1);
    assert!(events.is_empty());
}

#[tokio::test]
async fn real_first_binding_conflict_stays_conflict_without_callback() {
    let f = Fixture::new();
    let first = claims("same-original", &f.root);
    let row = f.row("first-binding").await;
    {
        let _writer = f.owner.source_writer().await;
        f.owner
            .retain_first_binding_write(
                f.store.as_ref(),
                &f.scope,
                &row.key,
                row.clone(),
                &first,
                None,
            )
            .unwrap();
    }
    let mut candidate = first.candidate().clone();
    candidate.target.context =
        meerkat_authorization_contracts::evidence::EvidenceId::new("changed-context").unwrap();
    let changed = InputAuthorityAssociation::new(candidate).unwrap();
    let called = Cell::new(false);
    let (result, events) = capture(
        correlation("same-original", &changed),
        f.owner.retain_followup_source(&changed, || async {
            called.set(true);
            Ok(())
        }),
    )
    .await;
    assert_eq!(result, Err(OriginalWriterError::Conflict));
    assert!(!called.get());
    refusal(&events, "first_binding_conflict", "conflict");
    assert_eq!(f.owner.originals.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn scoped_identity_restores_after_nested_return_unwind_and_drop() {
    let f = Fixture::new();
    let a = claims("a", &f.root);
    let b = claims("b", &f.root);
    let ca = correlation("a", &a);
    let cb = correlation("b", &b);
    let expected = ca.call.clone();
    let (_, events) = capture(ca, async {
        scope(Some(cb.clone()), async {
            assert_ne!(
                CURRENT.with(|c| c.correlation.as_ref().unwrap().call.clone()),
                expected
            );
        })
        .await;
        assert_eq!(
            CURRENT.with(|c| c.correlation.as_ref().unwrap().call.clone()),
            expected
        );
        assert!(
            AssertUnwindSafe(scope(Some(cb.clone()), async {
                panic!("scoped unwind");
            }))
            .catch_unwind()
            .await
            .is_err()
        );
        assert_eq!(
            CURRENT.with(|c| c.correlation.as_ref().unwrap().call.clone()),
            expected
        );
        let mut pending = Box::pin(scope(Some(cb), std::future::pending::<()>()));
        assert!(pending.as_mut().now_or_never().is_none());
        drop(pending);
        assert_eq!(
            CURRENT.with(|c| c.correlation.as_ref().unwrap().call.clone()),
            expected
        );
        emit("test.restored", None, None);
    })
    .await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["call"], expected);
    assert!(CURRENT.try_with(|_| ()).is_err());
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains("must-not-log"));
    assert!(!serialized.contains("captured-acquisition"));
    assert_eq!(
        digest("test", &[b"ab", b"c"]),
        "c46080dc6e5702646016b54f46dc21df9a6109e422dc12a5db4163c3da6e950b"
    );
    assert_ne!(
        digest("test", &[b"ab", b"c"]),
        digest("test", &[b"a", b"bc"])
    );
    assert_ne!(digest("test", &[b"x"]), digest("other", &[b"x"]));
}

#[tokio::test]
async fn missing_context_masks_ambient_identity_and_restores_it() {
    let f = Fixture::new();
    let a = claims("a", &f.root);
    let correlation = correlation("a", &a);
    let expected = correlation.call.clone();
    let (_, events) = capture(correlation, async {
        scope(None, async {
            assert!(CURRENT.with(|current| current.correlation.is_none()));
            begin_followup(&a);
            check_parent(|| parent_refusal(ParentCause::Cancelled));
            emit(
                "test.must_be_suppressed",
                Some("operation_cancelled"),
                Some("denied"),
            );
            assert!(CURRENT.with(|current| current.events.borrow().is_empty()));
        })
        .await;
        assert_eq!(
            CURRENT.with(|current| current.correlation.as_ref().unwrap().call.clone()),
            expected
        );
        let mut pending = Box::pin(scope(None, std::future::pending::<()>()));
        assert!(pending.as_mut().now_or_never().is_none());
        drop(pending);
        assert_eq!(
            CURRENT.with(|current| current.correlation.as_ref().unwrap().call.clone()),
            expected
        );
        emit("test.after_suppression", None, None);
    })
    .await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["stage"], "test.after_suppression");
    assert_eq!(events[0]["call"], expected);
    assert!(events[0]["parent_cause"].is_null());
    assert!(CURRENT.try_with(|_| ()).is_err());
}
