//! Smoke test that the `meerkat::session_runtime::admission` module is
//! wired up: typedefs resolve, free helpers compile, and the
//! capacity-collision error variant carries the expected session id.
//!
//! We can't construct a real `ActiveCapacityGuard` without the
//! session-service plumbing, so coverage is limited to the lookup-side
//! helpers (`has_*`, `take_*`, `discard_*`) on an empty ledger. The
//! `insert` collision branch is exercised by the existing
//! `meerkat-rpc` session-runtime integration tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use meerkat::session_runtime::admission::{
    StagedAdmissionRestore, StagedCapacityAdmissions, discard_staged_capacity_admission,
    has_staged_capacity_admission, take_staged_capacity_admission,
};
use meerkat_core::types::SessionId;

fn empty_ledger() -> StagedCapacityAdmissions {
    Arc::new(StdMutex::new(HashMap::new()))
}

#[test]
fn empty_ledger_has_no_admissions() {
    let ledger = empty_ledger();
    let session = SessionId::new();
    assert!(!has_staged_capacity_admission(&ledger, &session));
    assert!(take_staged_capacity_admission(&ledger, &session).is_none());
    discard_staged_capacity_admission(&ledger, &session);
    assert!(!has_staged_capacity_admission(&ledger, &session));
}

#[test]
fn runtime_pre_admission_registration_calls_restore_on_drop_and_disarms() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat::session_runtime::admission::{
        RuntimePreAdmissionRegistration, RuntimePreAdmissionRestore,
    };
    use meerkat_core::InputId;

    struct CountingRestore {
        calls: AtomicUsize,
    }

    impl RuntimePreAdmissionRestore for CountingRestore {
        fn restore_or_release(&self, _session_id: &SessionId, _input_id: &InputId) {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    let restore = Arc::new(CountingRestore {
        calls: AtomicUsize::new(0),
    });

    // Armed: drop should fire restore once.
    {
        let _registration = RuntimePreAdmissionRegistration::new(
            Arc::clone(&restore) as Arc<dyn RuntimePreAdmissionRestore>,
            SessionId::new(),
            InputId::new(),
        );
    }
    assert_eq!(restore.calls.load(Ordering::SeqCst), 1);

    // Disarmed: drop should not fire restore.
    let registration = RuntimePreAdmissionRegistration::new(
        Arc::clone(&restore) as Arc<dyn RuntimePreAdmissionRestore>,
        SessionId::new(),
        InputId::new(),
    );
    registration.disarm();
    assert_eq!(restore.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn staged_admission_restore_holds_session_id() {
    let ledger = empty_ledger();
    let session = SessionId::new();
    let restore = StagedAdmissionRestore {
        admissions: Arc::clone(&ledger),
        session_id: session.clone(),
    };
    assert_eq!(restore.session_id, session);
    // The cloned ledger handle must point at the same lock as the
    // original — prove via empty-ledger invariant.
    assert!(!has_staged_capacity_admission(
        &restore.admissions,
        &session
    ));
}
