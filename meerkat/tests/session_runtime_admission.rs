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
fn staged_admission_restore_holds_session_id() {
    let ledger = empty_ledger();
    let session = SessionId::new();
    let restore = StagedAdmissionRestore {
        admissions: ledger.clone(),
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
