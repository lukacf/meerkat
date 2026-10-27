//! Source guard for the identity-first lean deletions.
//!
//! Core owns each member's identity, generation, fence and session, and Meta
//! writes no Core identity intent row. So the bridge keeps no actuation
//! custody verdict of its own, the health monitor repairs nothing when a
//! member's event stream is lost, the session-owned retire reads no error
//! text, and the delivery repair spawns nothing fresh in place of an absent
//! session. The names those mechanisms used must not come back into the
//! crate source.

#![allow(clippy::panic)]

const SOURCES: &[(&str, &str)] = &[
    ("src/lib.rs", include_str!("../src/lib.rs")),
    (
        "src/identity_first/mod.rs",
        include_str!("../src/identity_first/mod.rs"),
    ),
    (
        "src/identity_first/bridge.rs",
        include_str!("../src/identity_first/bridge.rs"),
    ),
    (
        "src/identity_first/runtime.rs",
        include_str!("../src/identity_first/runtime.rs"),
    ),
    (
        "src/unified_runtime/mod.rs",
        include_str!("../src/unified_runtime/mod.rs"),
    ),
    (
        "src/mob_handle_runtime.rs",
        include_str!("../src/mob_handle_runtime.rs"),
    ),
];

const DELETED: &[&str] = &[
    // The bridge's own actuation custody, read from the identity intent row.
    "CollisionCustody",
    "collision_custody",
    "identity_actuation_custody",
    "await_convergence_and_attach",
    "CONVERGENCE_HANDOFF_BUDGET",
    // The health monitor's repair trigger on a lost event stream.
    "trigger_identity_stream_repair",
    "mark_active_runtime_broken",
    // The error-text tolerance on the session-owned retire path.
    "is_recoverable_session_owned_retire_cleanup_error",
    "is_session_owned_archive_absent_cleanup_error",
    // The delivery repair's fresh spawn in place of an absent session.
    "RepairResumeFailure",
    "DurableSnapshotMissing",
];

#[test]
fn the_deleted_identity_first_actuators_stay_deleted() {
    let found: Vec<String> = SOURCES
        .iter()
        .flat_map(|(path, source)| {
            DELETED
                .iter()
                .filter(move |name| source.contains(**name))
                .map(move |name| format!("{path}: {name}"))
        })
        .collect();
    assert!(
        found.is_empty(),
        "deleted identity-first actuators are back in the crate source: {found:#?}"
    );
}
