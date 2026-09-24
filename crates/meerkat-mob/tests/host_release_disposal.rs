//! Multi-host mobs phase 3 — the wire→machine disposal fold (design-revival
//! R T-3 == design-fold-ins T-U4): ONE owner, exhaustive, no wildcard.
//!
//! R T-4 (local truthful disposal for host-owned adopted sessions —
//! `dispose_archive_session` feeding `ObserveMemberRetirementArchived
//! {RuntimeReleasedOnlyHostOwned}`) is an IN-CRATE row: the observation
//! producer and the adopted-session construction are `pub(crate)`
//! (meerkat-mob/src/runtime/tests.rs holds the existing twin at the
//! working-tree fix site) — owned by the A2 lane, referenced here.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_mob::machines::mob_machine::MemberSessionDisposal as MachineDisposal;
use meerkat_mob::runtime::bridge_protocol::{
    MemberSessionDisposal as WireDisposal, RuntimeReleaseCause, member_session_disposal_from_wire,
};

// ===========================================================================
// R T-3 — total fold table (§19.L4): AlreadyArchived folds to Archived; the
// RuntimeReleasedOnly causes map 1:1
// ===========================================================================

#[test]
fn disposal_fold_is_total_and_folds_already_archived() {
    // The table, row by row.
    assert_eq!(
        member_session_disposal_from_wire(&WireDisposal::Archived),
        MachineDisposal::Archived
    );
    assert_eq!(
        member_session_disposal_from_wire(&WireDisposal::AlreadyArchived),
        MachineDisposal::Archived,
        "§19.L4: both mean the durable terminal holds — no information loss on replay \
         (AlreadyArchived is therefore never replayed)"
    );
    assert_eq!(
        member_session_disposal_from_wire(&WireDisposal::RuntimeReleasedOnly {
            cause: RuntimeReleaseCause::HostOwnedSession
        }),
        MachineDisposal::RuntimeReleasedOnlyHostOwned
    );
    assert_eq!(
        member_session_disposal_from_wire(&WireDisposal::RuntimeReleasedOnly {
            cause: RuntimeReleaseCause::NoDurableSessions
        }),
        MachineDisposal::RuntimeReleasedOnlyNoDurableSessions
    );

    // Exhaustiveness pin: the compile-time half is the fold's wildcard-free
    // match (a new wire variant breaks the build there). The runtime half:
    // every constructible wire value folds — enumerate the closed cause set.
    for cause in [
        RuntimeReleaseCause::HostOwnedSession,
        RuntimeReleaseCause::NoDurableSessions,
    ] {
        let folded =
            member_session_disposal_from_wire(&WireDisposal::RuntimeReleasedOnly { cause });
        assert!(
            matches!(
                folded,
                MachineDisposal::RuntimeReleasedOnlyHostOwned
                    | MachineDisposal::RuntimeReleasedOnlyNoDurableSessions
            ),
            "RuntimeReleasedOnly causes never fold to a success class, got {folded:?}"
        );
    }
}

// ===========================================================================
// Fold ↔ wire round-trip discipline: the machine never stores AlreadyArchived
// ===========================================================================

#[test]
fn machine_vocabulary_has_no_already_archived_seat() {
    // The machine enum is the three-variant flat form (mob_machine.rs) — a
    // compile-time pin that the AlreadyArchived distinction lives ONLY on
    // the wire. Matching all machine variants with no AlreadyArchived arm IS
    // the assertion; a vocabulary change breaks this match.
    let all = [
        MachineDisposal::Archived,
        MachineDisposal::RuntimeReleasedOnlyHostOwned,
        MachineDisposal::RuntimeReleasedOnlyNoDurableSessions,
    ];
    for disposal in all {
        match disposal {
            MachineDisposal::Archived
            | MachineDisposal::RuntimeReleasedOnlyHostOwned
            | MachineDisposal::RuntimeReleasedOnlyNoDurableSessions => {}
        }
    }
}
